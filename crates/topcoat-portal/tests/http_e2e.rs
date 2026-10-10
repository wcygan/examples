use serde_json::Value;
use std::{
    fs,
    io::BufRead,
    io::BufReader,
    io::Read,
    io::Write,
    net::TcpStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "topcoat-2pc-http-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Portal(Child);
impl Drop for Portal {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn binaries() -> (PathBuf, PathBuf) {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    (debug.join("topcoat-portal"), debug.join("raft-sharded-tx"))
}
fn start(temp: &Temp) -> (Portal, String) {
    let (portal, raft) = binaries();
    let mut child = Command::new(portal)
        .args([
            "--port",
            "0",
            "--journal",
            temp.0.join("decisions.jsonl").to_str().unwrap(),
            "--raft-bin",
            raft.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        let first = lines.next().unwrap().unwrap();
        let _ = tx.send(first);
        while lines.next().is_some() {}
    });
    let line = rx.recv_timeout(Duration::from_secs(15)).unwrap();
    let url = line.strip_prefix("PORTAL_READY ").unwrap().to_owned();
    (Portal(child), url)
}
fn request(url: &str, method: &str, path: &str, body: &str, extra: &str) -> (String, String) {
    let addr = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    write!(stream,"{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    (head.to_owned(), body.to_owned())
}

#[test]
fn actual_topcoat_http_process_serves_ui_transactions_recovery_and_lifecycle() {
    let temp = Temp::new();
    let (_portal, url) = start(&temp);
    let (headers, html) = request(&url, "GET", "/", "", "");
    assert!(headers.contains("200 OK"));
    assert!(headers.to_ascii_lowercase().contains("text/html"));
    assert!(html.contains("Two-shard two-phase commit"));
    assert!(html.contains("Kill a source follower process"));
    let (_, state) = request(&url, "GET", "/api/status", "", "");
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["source_nodes"].as_array().unwrap().len(), 3);
    assert_eq!(state["destination_nodes"].as_array().unwrap().len(), 3);
    let (_, response) = request(
        &url,
        "POST",
        "/api/transfer",
        r#"{"tx_id":"http-commit","amount":4,"fault":"none"}"#,
        "",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["CommitAcknowledged"],
        Value::Null
    );
    let (_, response) = request(
        &url,
        "POST",
        "/api/transfer",
        r#"{"tx_id":"http-partial","amount":3,"fault":"partial_decision_delivery"}"#,
        "",
    );
    assert!(response.contains("PartialDecisionDelivery"));
    let (_, response) = request(
        &url,
        "POST",
        "/api/recover",
        r#"{"tx_id":"http-partial"}"#,
        "",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["CommitAcknowledged"],
        Value::Null
    );
    let (_, response) = request(
        &url,
        "POST",
        "/api/nodes",
        r#"{"op":"add","shard":"source"}"#,
        "",
    );
    let added = serde_json::from_str::<Value>(&response).unwrap()["node_id"]
        .as_u64()
        .unwrap();
    assert_eq!(added, 7);
    let (_, response) = request(
        &url,
        "POST",
        "/api/nodes",
        &format!(r#"{{"op":"remove","node_id":{added}}}"#),
        "",
    );
    assert!(response.contains("removed"));
    let (sse_head, _) = request_sse(&url);
    assert!(sse_head.contains("text/event-stream"));
    let journal = fs::read_to_string(temp.0.join("decisions.jsonl")).unwrap();
    assert!(journal.contains("http-commit"));
    assert!(journal.contains("http-partial"));
    let _ = request(&url, "POST", "/api/shutdown", "", "");
}
fn request_sse(url: &str) -> (String, String) {
    let addr = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET /api/events HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nLast-Event-ID: 0\r\n\r\n"
    )
    .unwrap();
    let mut out = vec![0; 8192];
    let n = stream.read(&mut out).unwrap();
    let data = String::from_utf8_lossy(&out[..n]).into_owned();
    let mut parts = data.splitn(2, "\r\n\r\n");
    (
        parts.next().unwrap_or("").into(),
        parts.next().unwrap_or("").into(),
    )
}
