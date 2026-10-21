pub mod audit;
pub mod cluster;
pub mod console;
mod network;
mod storage;

use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    pub key: String,
    pub value: String,
}

openraft::declare_raft_types!(
    pub TypeConfig: D = Command, R = Option<String>, Node = (),
);
pub type Node = openraft::Raft<TypeConfig>;
