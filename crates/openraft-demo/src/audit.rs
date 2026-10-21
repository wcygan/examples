//! Independent append-only observations; Raft's replicated log may change.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeState {
    pub role: String,
    pub term: u64,
    pub leader: Option<u64>,
    pub available: bool,
    pub isolated: bool,
    pub log: BTreeMap<u64, Value>,
    pub committed: Option<u64>,
    pub applied: Option<u64>,
    pub values: BTreeMap<String, String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Started,
    VoteSaved,
    LogAppended,
    LogTruncated,
    LogPurged,
    Committed,
    Applied,
    SnapshotBuilt,
    SnapshotInstalled,
    Metrics,
    Sent,
    Received,
    ResponseSent,
    ResponseReceived,
    Dropped,
    Fault,
    ClientWrite,
    ClientAcknowledged,
    ClientTimeout,
    Finished,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub run_id: u64,
    pub sequence: u64,
    pub elapsed_ms: u128,
    pub actor: u64,
    pub peer: Option<u64>,
    pub message_id: Option<u64>,
    pub kind: Kind,
    pub description: String,
    pub payload: Value,
    pub nodes: BTreeMap<u64, NodeState>,
}
struct Journal {
    run_id: u64,
    started: Instant,
    events: Vec<Event>,
    nodes: BTreeMap<u64, NodeState>,
    next_message: u64,
    sealed: bool,
}
#[derive(Clone)]
pub struct Audit(Arc<Mutex<Journal>>);
impl Default for Audit {
    fn default() -> Self {
        Self::new(0)
    }
}
impl Audit {
    pub fn new(run_id: u64) -> Self {
        Self::with_nodes(run_id, 3).expect("valid default node count")
    }
    pub fn with_nodes(run_id: u64, node_count: usize) -> Result<Self, String> {
        if !(3..=5).contains(&node_count) {
            return Err("Choose 3–5 Raft nodes".into());
        }
        let nodes = (1..=node_count as u64)
            .map(|id| {
                (
                    id,
                    NodeState {
                        role: "Learner".into(),
                        available: true,
                        ..NodeState::default()
                    },
                )
            })
            .collect();
        Ok(Self(Arc::new(Mutex::new(Journal {
            run_id,
            started: Instant::now(),
            events: vec![],
            nodes,
            next_message: 0,
            sealed: false,
        }))))
    }
    pub fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap().events.clone()
    }
    pub fn event_after(&self, cursor: u64) -> Option<Event> {
        self.0
            .lock()
            .unwrap()
            .events
            .get(usize::try_from(cursor).ok()?)
            .cloned()
    }
    pub fn nodes(&self) -> BTreeMap<u64, NodeState> {
        self.0.lock().unwrap().nodes.clone()
    }
    pub fn emit(
        &self,
        actor: u64,
        kind: Kind,
        description: impl Into<String>,
        payload: Value,
        update: impl FnOnce(&mut NodeState),
    ) {
        self.record(actor, kind, description.into(), payload, None, update);
    }
    fn record(
        &self,
        actor: u64,
        kind: Kind,
        description: String,
        payload: Value,
        correlation: Option<(u64, u64)>,
        update: impl FnOnce(&mut NodeState),
    ) {
        let mut journal = self.0.lock().unwrap();
        if journal.sealed {
            return;
        }
        update(journal.nodes.get_mut(&actor).unwrap());
        let event = Event {
            run_id: journal.run_id,
            sequence: journal.events.len() as u64 + 1,
            elapsed_ms: journal.started.elapsed().as_millis(),
            actor,
            peer: correlation.map(|c| c.0),
            message_id: correlation.map(|c| c.1),
            kind,
            description,
            payload,
            nodes: journal.nodes.clone(),
        };
        journal.events.push(event);
        if matches!(kind, Kind::Finished | Kind::Failed) {
            journal.sealed = true;
        }
    }
    pub fn message(&self) -> u64 {
        let mut journal = self.0.lock().unwrap();
        journal.next_message += 1;
        journal.next_message
    }
    pub fn rpc(&self, actor: u64, peer: u64, id: u64, kind: Kind, label: &str, payload: Value) {
        self.record(actor, kind, label.into(), payload, Some((peer, id)), |_| {});
    }
    pub fn finish(&self, error: Option<&str>) {
        self.emit(
            1,
            if error.is_some() {
                Kind::Failed
            } else {
                Kind::Finished
            },
            error.unwrap_or("Recording complete"),
            Value::Null,
            |_| {},
        );
    }
}
