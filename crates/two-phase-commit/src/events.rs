//! Ordered, session-only observations. These records are not a recovery journal.
use crate::{CoordinatorState, Decision, ParticipantState, Vote};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub state: ParticipantState,
    pub vote: Option<Vote>,
    pub balance: i64,
    pub blocked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub coordinator: CoordinatorState,
    pub coordinator_available: bool,
    pub votes_received: usize,
    pub nodes: Vec<Node>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self::for_participants(["source-account".into(), "destination-account".into()])
    }
}
impl Snapshot {
    pub fn for_participants(ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            coordinator: CoordinatorState::Collecting,
            coordinator_available: true,
            votes_received: 0,
            nodes: ids
                .into_iter()
                .enumerate()
                .map(|(index, id)| Node {
                    id,
                    state: ParticipantState::Working,
                    vote: None,
                    balance: if index == 0 { 100 } else { 20 },
                    blocked: false,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Started,
    PrepareSent,
    Prepared,
    VoteSent,
    VoteReceived,
    DecisionMade,
    DecisionSent,
    DecisionApplied,
    FaultInjected,
    TimeoutObserved,
    DuplicateDelivery,
    Finished,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolMessage {
    Prepare,
    Vote(Vote),
    Decision(Decision),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtocolEvent {
    pub run_id: u64,
    pub sequence: u64,
    pub elapsed_ms: u64,
    pub actor: String,
    pub kind: EventKind,
    pub message_id: Option<String>,
    pub peer: Option<String>,
    pub message: Option<ProtocolMessage>,
    pub description: String,
    pub snapshot: Snapshot,
}

type Sink = Box<dyn Fn(ProtocolEvent) + Send>;
struct Collector {
    run_id: u64,
    sequence: u64,
    start: Instant,
    snapshot: Snapshot,
    sink: Sink,
    pace: Duration,
}

/// Clones share one collector, ordering observations from participant threads.
#[derive(Clone)]
pub struct Observer(Arc<Mutex<Collector>>);
impl Observer {
    pub fn new(run_id: u64, pace: Duration, sink: impl Fn(ProtocolEvent) + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Collector {
            run_id,
            sequence: 0,
            start: Instant::now(),
            snapshot: Snapshot::default(),
            sink: Box::new(sink),
            pace,
        })))
    }
    pub fn quiet() -> Self {
        Self::new(0, Duration::ZERO, |_| {})
    }
    pub fn emit(
        &self,
        kind: EventKind,
        actor: &str,
        description: impl Into<String>,
        message: Option<(&str, &str, ProtocolMessage)>,
        update: impl FnOnce(&mut Snapshot),
    ) {
        let mut collector = self.0.lock().unwrap();
        update(&mut collector.snapshot);
        collector.sequence += 1;
        let event = ProtocolEvent {
            run_id: collector.run_id,
            sequence: collector.sequence,
            elapsed_ms: collector
                .start
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            actor: actor.into(),
            kind,
            description: description.into(),
            message_id: message.map(|(id, _, _)| id.into()),
            peer: message.map(|(_, peer, _)| peer.into()),
            message: message.map(|(_, _, payload)| payload),
            snapshot: collector.snapshot.clone(),
        };
        (collector.sink)(event);
        // Keep the collector locked so pacing cannot reorder threaded observations.
        if !collector.pace.is_zero() {
            std::thread::sleep(collector.pace);
        }
    }
}
