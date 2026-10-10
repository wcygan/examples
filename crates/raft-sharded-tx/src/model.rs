use openraft::BasicNode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Command {
    Prepare {
        tx_id: String,
        account: String,
        delta: i64,
    },
    Decide {
        tx_id: String,
        decision: Decision,
    },
}

impl Command {
    pub fn tx_id(&self) -> &str {
        match self {
            Self::Prepare { tx_id, .. } | Self::Decide { tx_id, .. } => tx_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Decision {
    Commit,
    Abort,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApplyResult {
    Noop,
    Prepared { yes: bool, reason: Option<String> },
    Committed,
    Aborted,
    Rejected { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TxOutcome {
    Prepared,
    Commit,
    Abort,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShardView {
    pub shard: String,
    pub balances: BTreeMap<String, i64>,
    pub prepared: BTreeMap<String, String>,
    pub outcomes: BTreeMap<String, TxOutcome>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxStatus {
    pub tx_id: String,
    pub outcome: Option<TxOutcome>,
    pub prepared: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub addr: String,
}
impl From<NodeInfo> for BasicNode {
    fn from(n: NodeInfo) -> Self {
        BasicNode::new(n.addr)
    }
}

openraft::declare_raft_types!(
    pub TypeConfig:
        D = Command,
        R = ApplyResult,
        NodeId = u64,
        Node = BasicNode,
        SnapshotData = std::io::Cursor<Vec<u8>>,
        AsyncRuntime = openraft::TokioRuntime,
);
