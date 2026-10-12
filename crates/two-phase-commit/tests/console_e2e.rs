//! Process-level checks of the public console API and SSE replay contract.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use two_phase_commit::events::{EventKind, ProtocolEvent, ProtocolMessage};
use two_phase_commit::{CoordinatorState, Decision, ParticipantState, Vote};

const TIMEOUT: Duration = Duration::from_secs(5);

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
    participant_count: usize,
    status: String,
    events: Vec<ProtocolEvent>,
}

impl Server {
    fn start() -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_two-phase-commit"))
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

fn sse_events(response: &Response) -> Vec<ProtocolEvent> {
    assert_eq!(response.status, 200);
    assert!(response
        .headers
        .to_ascii_lowercase()
        .contains("content-type: text/event-stream"));
    response
        .body
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(|frame| {
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            let id: u64 = frame
                .lines()
                .find_map(|line| line.strip_prefix("id: "))
                .expect("SSE event cursor")
                .parse()
                .unwrap();
            let event: ProtocolEvent = serde_json::from_str(data).unwrap();
            assert_eq!(id, event.sequence);
            Some(event)
        })
        .collect()
}

fn assert_replay(server: &Server, run: &Run) {
    let path = format!("/api/runs/{}/events", run.id);
    let complete = sse_events(&server.request("GET", &path, "", None));
    assert_eq!(
        serde_json::to_value(&complete).unwrap(),
        serde_json::to_value(&run.events).unwrap()
    );
    // Reconnect at a mid-run boundary, preserving exact snapshots and all fields.
    let cursor = run.events.len() / 2;
    let resumed = sse_events(&server.request("GET", &path, "", Some(cursor as u64)));
    let joined: Vec<_> = complete[..cursor].iter().chain(&resumed).collect();
    assert_eq!(
        serde_json::to_value(joined).unwrap(),
        serde_json::to_value(&run.events).unwrap()
    );
    let end = server.request("GET", &path, "", Some(run.events.last().unwrap().sequence));
    assert!(sse_events(&end).is_empty());
}

fn consume_receipt(pending: &mut Vec<&ProtocolEvent>, event: &ProtocolEvent) {
    let sent = pending
        .iter()
        .position(|sent| {
            sent.message_id == event.message_id
                && sent.message == event.message
                && sent.actor == event.peer.as_deref().unwrap()
                && sent.peer.as_deref() == Some(event.actor.as_str())
        })
        .expect("receipt must match an actual, unconsumed send");
    assert!(match event.kind {
        EventKind::Prepared => pending[sent].kind == EventKind::PrepareSent,
        EventKind::VoteReceived => pending[sent].kind == EventKind::VoteSent,
        EventKind::DecisionApplied => matches!(
            pending[sent].kind,
            EventKind::DecisionSent | EventKind::DuplicateDelivery
        ),
        _ => unreachable!(),
    });
    pending.remove(sent);
}

fn assert_protocol_trace(events: &[ProtocolEvent]) {
    let mut pending = Vec::<&ProtocolEvent>::new();
    let mut votes = BTreeMap::new();
    let mut previous = events[0].snapshot.clone();
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
        assert_ne!(event.kind, EventKind::Failed);
        match event.kind {
            EventKind::PrepareSent
            | EventKind::VoteSent
            | EventKind::DecisionSent
            | EventKind::DuplicateDelivery => {
                assert!(
                    event.message_id.is_some() && event.message.is_some() && event.peer.is_some()
                );
                if event.kind == EventKind::PrepareSent {
                    assert_eq!(event.message, Some(ProtocolMessage::Prepare));
                    assert_eq!(event.actor, "coordinator");
                } else if matches!(
                    event.kind,
                    EventKind::DecisionSent | EventKind::DuplicateDelivery
                ) {
                    assert!(matches!(event.message, Some(ProtocolMessage::Decision(_))));
                    assert_eq!(event.actor, "coordinator");
                }
                if event.kind == EventKind::VoteSent {
                    assert!(
                        votes
                            .insert(event.actor.clone(), event.message.unwrap())
                            .is_none(),
                        "participant voted twice"
                    );
                    let node = event
                        .snapshot
                        .nodes
                        .iter()
                        .find(|n| n.id == event.actor)
                        .unwrap();
                    assert_eq!(event.message, node.vote.map(ProtocolMessage::Vote));
                }
                pending.push(event);
            }
            EventKind::Prepared | EventKind::VoteReceived | EventKind::DecisionApplied => {
                consume_receipt(&mut pending, event);
            }
            EventKind::FaultInjected => {
                // A withholding marker is not a send. Remove an intercepted vote.
                if matches!(event.message, Some(ProtocolMessage::Vote(_))) {
                    let sent = pending
                        .iter()
                        .position(|sent| sent.message_id == event.message_id)
                        .unwrap();
                    assert_eq!(event.actor, "coordinator");
                    assert_eq!(event.peer.as_deref(), Some(pending[sent].actor.as_str()));
                    assert_eq!(event.message, pending[sent].message);
                    pending.remove(sent);
                }
            }
            _ => assert!(
                event.message.is_none() && event.message_id.is_none() && event.peer.is_none()
            ),
        }
        for node in &event.snapshot.nodes {
            let before = previous.nodes.iter().find(|n| n.id == node.id).unwrap();
            if node.balance != before.balance {
                assert_eq!(event.kind, EventKind::DecisionApplied);
                assert_eq!(event.actor, node.id);
                assert_eq!(
                    event.message,
                    Some(ProtocolMessage::Decision(Decision::Commit))
                );
                assert_eq!(before.state, ParticipantState::Prepared);
            }
        }
        previous = event.snapshot.clone();
    }
    assert!(
        pending.is_empty(),
        "every send must be delivered or explicitly withheld"
    );
    assert_eq!(votes.len(), events[0].snapshot.nodes.len());
}

fn assert_boundary_counts(name: &str, events: &[ProtocolEvent]) {
    let count = events[0].snapshot.nodes.len();
    let blocked = matches!(name, "missing-vote" | "coordinator-loss");
    let applied = match name {
        "missing-vote" | "coordinator-loss" => 0,
        "partial-delivery" => count * 2 - 1,
        "recovery" => count * 2,
        _ => count,
    };
    for (kind, count) in [
        (EventKind::Started, 1),
        (EventKind::PrepareSent, count),
        (EventKind::Prepared, count),
        (EventKind::VoteSent, count),
        (
            EventKind::VoteReceived,
            if name == "coordinator-loss" {
                1
            } else if blocked {
                count - 1
            } else {
                count
            },
        ),
        (EventKind::DecisionMade, usize::from(!blocked)),
        (EventKind::DecisionSent, if blocked { 0 } else { count }),
        (EventKind::DecisionApplied, applied),
        (EventKind::TimeoutObserved, usize::from(blocked)),
        (EventKind::Finished, 1),
    ] {
        assert_eq!(
            events.iter().filter(|e| e.kind == kind).count(),
            count,
            "{name}: incorrect {kind:?} boundary count"
        );
    }
}

fn assert_faults(name: &str, events: &[ProtocolEvent]) {
    let count = events[0].snapshot.nodes.len();
    let faults: Vec<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::FaultInjected)
        .collect();
    let duplicates: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == EventKind::DuplicateDelivery)
        .collect();
    assert_eq!(
        duplicates.len(),
        match name {
            "partial-delivery" => count - 1,
            "recovery" => count,
            _ => 0,
        }
    );
    for (index, duplicate) in duplicates {
        let receipt = &events[index + 1];
        assert_eq!(receipt.kind, EventKind::DecisionApplied);
        assert_eq!(receipt.message_id, duplicate.message_id);
        assert_eq!(
            serde_json::to_value(&receipt.snapshot).unwrap(),
            serde_json::to_value(&duplicate.snapshot).unwrap()
        );
    }
    match name {
        "missing-vote" | "coordinator-loss" => {
            assert_eq!(
                faults.len(),
                if name == "missing-vote" { 1 } else { count - 1 }
            );
            assert_eq!(faults[0].message, Some(ProtocolMessage::Vote(Vote::Yes)));
            assert!(events.iter().any(|e| e.kind == EventKind::TimeoutObserved));
            assert!(!events.iter().any(|e| matches!(
                e.kind,
                EventKind::DecisionMade | EventKind::DecisionSent | EventKind::DecisionApplied
            )));
        }
        "partial-delivery" => {
            assert_eq!(faults.len(), 1);
            let withheld = faults[0];
            assert_eq!(
                withheld.message,
                Some(ProtocolMessage::Decision(Decision::Commit))
            );
            assert_eq!(
                withheld.snapshot.coordinator,
                CoordinatorState::Decided(Decision::Commit)
            );
            assert_eq!(
                withheld.snapshot.nodes[0].state,
                ParticipantState::Committed
            );
            assert_eq!(
                withheld.snapshot.nodes[count - 1].state,
                ParticipantState::Prepared
            );
            assert!(withheld.snapshot.nodes[count - 1].blocked);
            assert_eq!(withheld.snapshot.nodes[count - 1].balance, 20);
            assert!(!events.iter().any(|e| e.sequence < withheld.sequence
                && e.kind == EventKind::DecisionSent
                && e.message_id == withheld.message_id));
            assert!(events.iter().any(|e| e.sequence > withheld.sequence
                && e.kind == EventKind::DecisionApplied
                && e.message_id == withheld.message_id));
        }
        _ => assert!(faults.is_empty()),
    }
}

fn check_scenario(name: &str, decision: Option<Decision>, state: ParticipantState) {
    let server = Server::start();
    for count in 2..=5 {
        check_run(&server, name, count, decision, state);
    }
}

fn assert_final_state(run: &Run, decision: Option<Decision>, state: ParticipantState) {
    let name = run.scenario.as_str();
    let count = run.participant_count;
    let final_state = &run.events.last().unwrap().snapshot;
    assert_eq!(
        final_state.coordinator,
        decision
            .map(CoordinatorState::Decided)
            .unwrap_or(CoordinatorState::Collecting)
    );
    assert_eq!(
        final_state.coordinator_available,
        name != "coordinator-loss"
    );
    assert_eq!(
        final_state.votes_received,
        if name == "coordinator-loss" {
            1
        } else if decision.is_none() {
            count - 1
        } else {
            count
        }
    );
    assert_eq!(final_state.nodes.len(), count);
    for (index, node) in final_state.nodes.iter().enumerate() {
        assert_eq!(node.id, format!("participant-{}", index + 1));
        assert_eq!(node.state, state);
        let balance = if index == 0 { 100 } else { 20 };
        let change = if decision == Some(Decision::Commit) {
            if index == 0 {
                -10 * (count as i64 - 1)
            } else {
                10
            }
        } else {
            0
        };
        assert_eq!(node.balance, balance + change);
        assert_eq!(node.blocked, decision.is_none());
        assert_eq!(
            node.vote,
            Some(if name == "no-vote" && index == count - 1 {
                Vote::No
            } else {
                Vote::Yes
            })
        );
    }
}

fn check_run(
    server: &Server,
    name: &str,
    count: usize,
    decision: Option<Decision>,
    state: ParticipantState,
) {
    let page = server.request("GET", "/", "", None);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Audit waterfall"));
    let body = serde_json::json!({"scenario": name, "participant_count": count}).to_string();
    let started = server.request("POST", "/api/runs", &body, None);
    assert_eq!(started.status, 201);
    let id = serde_json::from_str::<serde_json::Value>(&started.body).unwrap()["id"]
        .as_u64()
        .unwrap();
    // Subscribe through the real socket before polling the finished state.
    let streamed = sse_events(&server.request("GET", &format!("/api/runs/{id}/events"), "", None));
    let run = server.finish(id);
    assert_eq!(run.id, id);
    assert_eq!(run.scenario, name);
    assert_eq!(run.participant_count, count);
    assert_eq!(run.status, "finished");
    assert_eq!(
        serde_json::to_value(streamed).unwrap(),
        serde_json::to_value(&run.events).unwrap()
    );
    assert_eq!(run.events.first().unwrap().kind, EventKind::Started);
    assert_eq!(run.events.last().unwrap().kind, EventKind::Finished);
    assert!(run.events.iter().all(|e| e.run_id == id));
    assert_protocol_trace(&run.events);
    assert_boundary_counts(name, &run.events);
    assert_faults(name, &run.events);
    assert_final_state(&run, decision, state);
    let list = server.request("GET", "/api/runs", "", None);
    assert_eq!(list.status, 200);
    let history: Vec<Run> = serde_json::from_str(&list.body).unwrap();
    assert_eq!(history.len(), count - 1);
    assert_eq!(history[0].id, id);
    assert_eq!(history[0].participant_count, count);
    assert_eq!(history[0].status, "finished");
    assert!(history[0].events.is_empty());
    assert_replay(server, &run);
    let oldest = server.run(1);
    assert_eq!(oldest.participant_count, 2);
    assert_eq!(oldest.events[0].snapshot.nodes.len(), 2);
}

#[test]
fn commit_over_http_and_sse() {
    check_scenario(
        "commit",
        Some(Decision::Commit),
        ParticipantState::Committed,
    );
}
#[test]
fn no_vote_over_http_and_sse() {
    check_scenario("no-vote", Some(Decision::Abort), ParticipantState::Aborted);
}
#[test]
fn missing_vote_over_http_and_sse() {
    check_scenario("missing-vote", None, ParticipantState::Prepared);
}
#[test]
fn coordinator_loss_over_http_and_sse() {
    check_scenario("coordinator-loss", None, ParticipantState::Prepared);
}
#[test]
fn partial_delivery_over_http_and_sse() {
    check_scenario(
        "partial-delivery",
        Some(Decision::Commit),
        ParticipantState::Committed,
    );
}
#[test]
fn recovery_over_http_and_sse() {
    check_scenario(
        "recovery",
        Some(Decision::Commit),
        ParticipantState::Committed,
    );
}
