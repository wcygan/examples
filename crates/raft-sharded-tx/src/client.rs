//! Bounded framed client protocol used by the supervisor and child node processes.
use crate::{
    model::{ApplyResult, Command, ShardView, TxStatus},
    node::MetricView,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Admin {
    Initialize(Vec<(u64, String)>),
    AddLearner(u64, String),
    ChangeMembership(Vec<u64>),
    Write(Command),
    View,
    Metrics,
    Status(String),
    Shutdown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AdminReply {
    Unit,
    Apply(ApplyResult),
    View(ShardView),
    Metrics(MetricView),
    Status(TxStatus),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RpcRequest {
    Append(openraft::raft::AppendEntriesRequest<crate::model::TypeConfig>),
    Vote(openraft::raft::VoteRequest<u64>),
    Snapshot(openraft::raft::InstallSnapshotRequest<crate::model::TypeConfig>),
    Admin(Admin),
}
#[derive(Debug, Serialize, Deserialize)]
pub enum RpcResponse {
    Append(Result<openraft::raft::AppendEntriesResponse<u64>, String>),
    Vote(Result<openraft::raft::VoteResponse<u64>, String>),
    Snapshot(Result<openraft::raft::InstallSnapshotResponse<u64>, String>),
    Admin(Result<AdminReply, String>),
}

pub async fn request(addr: &str, req: &RpcRequest) -> io::Result<RpcResponse> {
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .map_err(io::Error::other)??;
    write_frame(&mut stream, req).await?;
    tokio::time::timeout(Duration::from_secs(3), read_frame(&mut stream))
        .await
        .map_err(io::Error::other)?
}
pub async fn admin(addr: &str, cmd: Admin) -> io::Result<Result<AdminReply, String>> {
    match request(addr, &RpcRequest::Admin(cmd)).await? {
        RpcResponse::Admin(v) => Ok(v),
        _ => Err(io::Error::other("unexpected response type")),
    }
}
async fn write_frame<T: Serialize>(stream: &mut TcpStream, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await
}
async fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream) -> io::Result<T> {
    let n = stream.read_u32().await? as usize;
    if n > MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    let mut buf = vec![0; n];
    stream.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(io::Error::other)
}
