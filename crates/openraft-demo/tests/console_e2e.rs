//! Process-level checks of the public console API and SSE replay contract.
use openraft_demo::{audit::Event, cluster::SCENARIOS};
mod support;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

struct Process {
    child: Child,
    output: Option<JoinHandle<()>>,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.output.take() {
            let _ = reader.join();
        }
    }
}
struct Server {
    _process: Process,
    address: SocketAddr,
}
struct Response {
    status: u16,
    headers: String,
    body: String,
}
#[derive(Deserialize)]
struct Run {
    id: u64,
    scenario: String,
    node_count: usize,
    status: String,
    events: Vec<Event>,
}

impl Server {
    fn start() -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_openraft-demo"))
            .args(["--port", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start console binary");
        let mut process = Process {
            child,
            output: None,
        };
        let stdout = process.child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        process.output = Some(thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read startup URL");
            let _ = sender.send(line);
            // Drain protocol output so an unread pipe cannot stall the child.
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        }));
        let line = receiver
            .recv_timeout(TIMEOUT)
            .expect("console startup deadline");
        let address: SocketAddr = line
            .trim()
            .strip_prefix("Console: http://")
            .expect("console URL")
            .parse()
            .unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), 0);
        Self {
            _process: process,
            address,
        }
    }
    fn request(&self, method: &str, path: &str, body: &str, cursor: Option<u64>) -> Response {
        let mut socket = TcpStream::connect_timeout(&self.address, TIMEOUT).unwrap();
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        socket.set_write_timeout(Some(TIMEOUT)).unwrap();
        let cursor = cursor
            .map(|id| format!("Last-Event-ID: {id}\r\n"))
            .unwrap_or_default();
        // HTTP/1.0 closes the response body without a chunked-transfer decoder.
        write!(socket,
            "{method} {path} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{cursor}\r\n{body}",
            self.address, body.len()).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").expect("HTTP response");
        assert!(!headers
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"));
        Response {
            status: headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
            headers: headers.into(),
            body: body.into(),
        }
    }
    fn run(&self, id: u64) -> Run {
        let response = self.request("GET", &format!("/api/runs/{id}"), "", None);
        assert_eq!(response.status, 200);
        serde_json::from_str(&response.body).unwrap()
    }
    fn finish(&self, id: u64) -> Run {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let run = self.run(id);
            if run.status != "running" {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} exceeded its deadline");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn sse(response: &Response) -> Vec<Event> {
    assert_eq!(response.status, 200);
    assert!(response.headers.contains("text/event-stream"));
    response
        .body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|data| serde_json::from_str(data.trim()).unwrap())
        .collect()
}
fn exercise(server: &Server, scenario: &str, count: usize) {
    let body = serde_json::json!({"scenario": scenario, "node_count": count}).to_string();
    let response = server.request("POST", "/api/runs", &body, None);
    assert_eq!(response.status, 201);
    let id = serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["id"]
        .as_u64()
        .unwrap();
    if scenario == "quorum-loss" {
        assert_eq!(server.request("POST", "/api/runs", &body, None).status, 409);
    }
    // Connect during execution. The stream must reach the terminal observation.
    let live = sse(&server.request("GET", &format!("/api/runs/{id}/events"), "", None));
    let run = server.finish(id);
    assert_eq!(run.id, id);
    assert_eq!(run.scenario, scenario);
    assert_eq!(run.node_count, count);
    assert_eq!(
        run.status,
        "finished",
        "{scenario}/{count}: {:?}",
        run.events.last()
    );
    assert!(run.events.iter().all(|e| e.run_id == id));
    assert_eq!(
        serde_json::to_value(&live).unwrap(),
        serde_json::to_value(&run.events).unwrap()
    );
    support::safety(&run.events, count).unwrap_or_else(|e| panic!("{scenario}/{count}: {e}"));
    support::outcome(&run.events, scenario, count);
    // Exercise multiple reconnect positions, including the empty and final prefix.
    for cursor in [
        0,
        run.events.len() as u64 / 2,
        run.events.len() as u64 - 1,
        run.events.len() as u64,
    ] {
        let replay =
            sse(&server.request("GET", &format!("/api/runs/{id}/events"), "", Some(cursor)));
        assert_eq!(
            serde_json::to_value(replay).unwrap(),
            serde_json::to_value(&run.events[cursor as usize..]).unwrap()
        );
    }
}
#[test]
fn every_scenario_and_node_count_preserves_raft_properties_over_http_and_sse() {
    let server = Server::start();
    assert_eq!(server.request("GET", "/", "", None).status, 200);
    let motion = server.request("GET", "/motion.js", "", None);
    assert_eq!(motion.status, 200);
    assert!(motion.body.contains("class Player"));
    assert_eq!(server.request("GET", "/api/runs/999", "", None).status, 404);
    for body in [
        r#"{"scenario":"invalid"}"#,
        r#"{"scenario":"replication","node_count":2}"#,
        r#"{"scenario":"replication","node_count":6}"#,
        r#"{"scenario":"replication","node_count":3.5}"#,
    ] {
        assert_eq!(server.request("POST", "/api/runs", body, None).status, 400);
    }
    // Repeat the exhaustive matrix under naturally varying Tokio schedules.
    for _ in 0..2 {
        for count in 3..=5 {
            for scenario in SCENARIOS {
                exercise(&server, scenario, count);
            }
        }
    }
    let list = server.request("GET", "/api/runs", "", None);
    let runs = serde_json::from_str::<Vec<Run>>(&list.body).unwrap();
    assert_eq!(runs.len(), 24);
    let oldest = server.run(runs.last().unwrap().id);
    support::safety(&oldest.events, oldest.node_count).unwrap();
}

#[test]
fn omitted_node_count_keeps_the_three_node_default() {
    let server = Server::start();
    let response = server.request("POST", "/api/runs", r#"{"scenario":"replication"}"#, None);
    assert_eq!(response.status, 201);
    let id = serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["id"]
        .as_u64()
        .unwrap();
    let run = server.finish(id);
    assert_eq!(run.node_count, 3);
    support::safety(&run.events, 3).unwrap();
    support::outcome(&run.events, "replication", 3);
}
