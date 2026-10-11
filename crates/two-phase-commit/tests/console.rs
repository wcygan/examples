use futures_util::StreamExt;
use std::time::{Duration, Instant};
use topcoat::router::{request::Request, Body, Router, StatusCode};
use two_phase_commit::console::{router, Console};

async fn call(
    router: &Router,
    method: &str,
    uri: &str,
    body: &str,
    cursor: Option<u64>,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(cursor) = cursor {
        request = request.header("Last-Event-ID", cursor.to_string());
    }
    let response = router
        .handle(request.body(Body::from(body.to_owned())).unwrap())
        .await;
    let status = response.status();
    let mut stream = response.into_body().into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.unwrap());
    }
    (status, String::from_utf8(bytes).unwrap())
}
async fn finish(console: &Console, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while console.run(id).unwrap().status == "running" {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
#[tokio::test]
async fn validates_requests_and_retains_history() {
    let console = Console::default();
    let router = router(console.clone());
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/runs",
            r#"{"scenario":"invalid"}"#,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "POST", "/api/runs", "{", None).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&router, "GET", "/api/runs/999", "", None).await.0,
        StatusCode::NOT_FOUND
    );
    let id = console.start("commit", Duration::from_millis(5)).unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/runs",
            r#"{"scenario":"commit"}"#,
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    finish(&console, id).await;
    let second = console.start("no-vote", Duration::ZERO).unwrap();
    finish(&console, second).await;
    let (_, body) = call(&router, "GET", "/api/runs", "", None).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(console.run(id).unwrap().events.len() > 10);
    assert!(Console::default().runs().is_empty());
}
#[tokio::test]
async fn sse_reconnect_replays_cursor_then_follows_live_without_gaps() {
    let console = Console::default();
    let router = router(console.clone());
    let id = console
        .start("partial-delivery", Duration::from_millis(5))
        .unwrap();
    let (_, body) = tokio::time::timeout(
        Duration::from_secs(3),
        call(
            &router,
            "GET",
            &format!("/api/runs/{id}/events"),
            "",
            Some(3),
        ),
    )
    .await
    .unwrap();
    finish(&console, id).await;
    let sequences: Vec<u64> = body
        .lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .map(|id| id.parse().unwrap())
        .collect();
    let count = console.run(id).unwrap().events.len() as u64;
    assert_eq!(sequences, (4..=count).collect::<Vec<_>>());
    let (_, resumed) = call(
        &router,
        "GET",
        &format!("/api/runs/{id}/events"),
        "",
        Some(count - 2),
    )
    .await;
    let ids: Vec<u64> = resumed
        .lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .map(|id| id.parse().unwrap())
        .collect();
    assert_eq!(ids, vec![count - 1, count]);
    assert_eq!(
        call(&router, "GET", "/api/runs/999/events", "", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn console_binary_serves_on_an_ephemeral_loopback_port() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, Stdio};
    struct Guard(Child);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Guard(
        Command::new(env!("CARGO_BIN_EXE_two-phase-commit-console"))
            .args(["--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        sender.send(line).unwrap();
    });
    let line = receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    reader.join().unwrap();
    let address = line.trim().strip_prefix("Console: http://").unwrap();
    assert!(address.starts_with("127.0.0.1:"));
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("Two-phase commit console"));
}
