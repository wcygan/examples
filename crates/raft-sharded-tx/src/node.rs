use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricView {
    pub node_id: u64,
    pub shard: String,
    pub state: String,
    pub current_leader: Option<u64>,
    pub last_log_index: Option<u64>,
    pub last_applied: Option<u64>,
}
