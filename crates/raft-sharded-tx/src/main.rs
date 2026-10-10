//! Child process for one in-memory OpenRaft node. The supervisor talks to it over
//! a small length-prefixed JSON protocol; OpenRaft peer traffic uses the same listener.
use openraft::error::{NetworkError, RPCError, RaftError};
use openraft::{
    BasicNode, Config, Raft,
    network::{RPCOption, RaftNetwork, RaftNetworkFactory},
    raft::{
        AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest,
        InstallSnapshotResponse, VoteRequest, VoteResponse,
    },
};
use raft_sharded_tx::{
    client::{Admin, AdminReply, RpcRequest as Request, RpcResponse as Response},
    events::{NodeEvent, emit},
    model::TypeConfig,
    node::MetricView,
    storage::Store,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::RwLock,
};

const MAX_FRAME: usize = 16 * 1024 * 1024;
type NodeRaft = Raft<TypeConfig>;

#[derive(Clone)]
struct NetworkFactory {
    nodes: Arc<RwLock<BTreeMap<u64, String>>>,
}
struct Network {
    addr: String,
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) != Some("node") {
        eprintln!("usage: raft-sharded-tx node <id> <shard>");
        std::process::exit(2);
    }
    let id: u64 = args.get(1).ok_or("missing node id")?.parse()?;
    let shard = args.get(2).ok_or("missing shard")?.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    println!("READY {} {} {}", id, shard, addr);
    let nodes = Arc::new(RwLock::new(BTreeMap::new()));
    let config = Arc::new(
        Config {
            heartbeat_interval: 100,
            election_timeout_min: 350,
            election_timeout_max: 700,
            ..Default::default()
        }
        .validate()?,
    );
    let store = Store::new(id, shard.clone());
    let raft = Arc::new(
        Raft::new(
            id,
            config,
            NetworkFactory {
                nodes: nodes.clone(),
            },
            store.clone(),
            store.clone(),
        )
        .await?,
    );
    loop {
        let (mut stream, _) = listener.accept().await?;
        let raft = raft.clone();
        let nodes = nodes.clone();
        let store = store.clone();
        let shard = shard.clone();
        tokio::spawn(async move {
            match tokio::time::timeout(
                Duration::from_secs(5),
                handle(&mut stream, raft, nodes, store, id, shard),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("node request failed: {e}"),
                Err(_) => eprintln!("node request exceeded 5-second bound"),
            }
        });
    }
}

async fn handle(
    stream: &mut TcpStream,
    raft: Arc<NodeRaft>,
    nodes: Arc<RwLock<BTreeMap<u64, String>>>,
    store: Store,
    id: u64,
    shard: String,
) -> io::Result<()> {
    let req: Request = read_frame(stream).await?;
    match req {
        Request::Append(rpc) => {
            emit(NodeEvent {
                kind: "append_entries_received".into(),
                node_id: id,
                shard: shard.clone(),
                peer_id: None,
                tx_id: None,
                log_index: rpc.prev_log_id.as_ref().map(|l| l.index),
                detail: String::new(),
            });
            let r = raft.append_entries(rpc).await.map_err(|e| e.to_string());
            write_frame(stream, &Response::Append(r)).await?;
        }
        Request::Vote(rpc) => {
            emit(NodeEvent {
                kind: "vote_request_received".into(),
                node_id: id,
                shard: shard.clone(),
                peer_id: None,
                tx_id: None,
                log_index: None,
                detail: String::new(),
            });
            let r = raft.vote(rpc).await.map_err(|e| e.to_string());
            write_frame(stream, &Response::Vote(r)).await?;
        }
        Request::Snapshot(rpc) => {
            emit(NodeEvent {
                kind: "snapshot_received".into(),
                node_id: id,
                shard: shard.clone(),
                peer_id: None,
                tx_id: None,
                log_index: None,
                detail: String::new(),
            });
            let r = raft.install_snapshot(rpc).await.map_err(|e| e.to_string());
            write_frame(stream, &Response::Snapshot(r)).await?;
        }
        Request::Admin(admin) => {
            let (reply, stop) = admin_call(admin, &raft, &nodes, &store, id, &shard).await;
            write_frame(stream, &Response::Admin(reply)).await?;
            if stop {
                std::process::exit(0);
            }
        }
    }
    Ok(())
}

async fn admin_call(
    admin: Admin,
    raft: &NodeRaft,
    nodes: &Arc<RwLock<BTreeMap<u64, String>>>,
    store: &Store,
    id: u64,
    shard: &str,
) -> (Result<AdminReply, String>, bool) {
    let stop = matches!(admin, Admin::Shutdown);
    let result = match admin {
        Admin::Initialize(members) => {
            *nodes.write().await = members.iter().cloned().collect();
            let members = members
                .into_iter()
                .map(|(id, addr)| (id, BasicNode::new(addr)))
                .collect::<BTreeMap<_, _>>();
            raft.initialize(members)
                .await
                .map(|_| AdminReply::Unit)
                .map_err(|e| e.to_string())
        }
        Admin::AddLearner(new_id, addr) => raft
            .add_learner(new_id, BasicNode::new(addr), true)
            .await
            .map(|_| AdminReply::Unit)
            .map_err(|e| e.to_string()),
        Admin::ChangeMembership(ids) => raft
            .change_membership(ids.into_iter(), false)
            .await
            .map(|_| AdminReply::Unit)
            .map_err(|e| e.to_string()),
        Admin::Write(cmd) => raft
            .client_write(cmd)
            .await
            .map(|r| AdminReply::Apply(r.data))
            .map_err(|e| e.to_string()),
        Admin::View => match raft.ensure_linearizable().await {
            Ok(_) => Ok(AdminReply::View(store.view().await)),
            Err(e) => Err(e.to_string()),
        },
        Admin::Metrics => {
            let m = raft.metrics().borrow().clone();
            Ok(AdminReply::Metrics(MetricView {
                node_id: id,
                shard: shard.to_owned(),
                state: format!("{:?}", m.state),
                current_leader: m.current_leader,
                last_log_index: m.last_log_index,
                last_applied: m.last_applied.map(|l| l.index),
            }))
        }
        Admin::Status(tx) => {
            let (outcome, prepared) = store.status(&tx).await;
            Ok(AdminReply::Status(raft_sharded_tx::model::TxStatus {
                tx_id: tx,
                outcome,
                prepared,
            }))
        }
        Admin::Shutdown => Ok(AdminReply::Unit),
    };
    (result, stop)
}

async fn send<T: Serialize, R: DeserializeOwned>(addr: &str, req: &T) -> io::Result<R> {
    let mut stream = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr))
        .await
        .map_err(io::Error::other)??;
    write_frame(&mut stream, req).await?;
    tokio::time::timeout(Duration::from_secs(3), read_frame(&mut stream))
        .await
        .map_err(io::Error::other)?
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
fn rpc_error<E: std::error::Error>(e: io::Error) -> RPCError<u64, BasicNode, E> {
    RPCError::Network(NetworkError::new(&e))
}

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = Network;
    async fn new_client(&mut self, target: u64, node: &BasicNode) -> Self::Network {
        self.nodes.write().await.insert(target, node.addr.clone());
        Network {
            addr: node.addr.clone(),
        }
    }
}
impl RaftNetwork<TypeConfig> for Network {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let r: Response = send(&self.addr, &Request::Append(rpc))
            .await
            .map_err(rpc_error)?;
        match r {
            Response::Append(Ok(v)) => Ok(v),
            Response::Append(Err(e)) => Err(rpc_error(io::Error::other(e))),
            _ => Err(rpc_error(io::Error::other("unexpected response"))),
        }
    }
    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _option: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, BasicNode, RaftError<u64>>> {
        let r: Response = send(&self.addr, &Request::Vote(rpc))
            .await
            .map_err(rpc_error)?;
        match r {
            Response::Vote(Ok(v)) => Ok(v),
            Response::Vote(Err(e)) => Err(rpc_error(io::Error::other(e))),
            _ => Err(rpc_error(io::Error::other("unexpected response"))),
        }
    }
    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<u64>,
        RPCError<u64, BasicNode, RaftError<u64, openraft::error::InstallSnapshotError>>,
    > {
        let r: Response = send(&self.addr, &Request::Snapshot(rpc))
            .await
            .map_err(rpc_error)?;
        match r {
            Response::Snapshot(Ok(v)) => Ok(v),
            Response::Snapshot(Err(e)) => {
                Err(RPCError::Network(NetworkError::new(&io::Error::other(e))))
            }
            _ => Err(RPCError::Network(NetworkError::new(&io::Error::other(
                "unexpected response",
            )))),
        }
    }
}
