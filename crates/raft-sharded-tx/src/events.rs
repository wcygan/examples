use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeEvent {
    pub kind: String,
    pub node_id: u64,
    pub shard: String,
    pub peer_id: Option<u64>,
    pub tx_id: Option<String>,
    pub log_index: Option<u64>,
    pub detail: String,
}

pub fn emit(event: NodeEvent) {
    let line = format!(
        "EVENT {}\n",
        serde_json::to_string(&event).unwrap_or_else(|_| "{}".into())
    );
    let mut stdout = std::io::stdout().lock();
    let _ = std::io::Write::write_all(&mut stdout, line.as_bytes());
}
