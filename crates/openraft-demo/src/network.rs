//! Actual OpenRaft RPCs over a controllable in-process transport.
use crate::{
    audit::{Audit, Kind},
    Node, TypeConfig,
};
use openraft::{
    error::{InstallSnapshotError, RPCError, RaftError, RemoteError, Unreachable},
    network::{Backoff, RPCOption, RaftNetwork, RaftNetworkFactory},
    raft::{
        AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest,
        InstallSnapshotResponse, VoteRequest, VoteResponse,
    },
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
pub struct Transport(Arc<Mutex<Links>>);
#[derive(Default)]
struct Links {
    nodes: BTreeMap<u64, Node>,
    isolated: BTreeSet<u64>,
}
impl Transport {
    pub fn insert(&self, id: u64, node: Node) {
        self.0.lock().unwrap().nodes.insert(id, node);
    }
    pub fn isolate(&self, id: u64, value: bool) {
        let mut links = self.0.lock().unwrap();
        if value {
            links.isolated.insert(id);
        } else {
            links.isolated.remove(&id);
        }
    }
    pub fn remove(&self, id: u64) {
        self.0.lock().unwrap().nodes.remove(&id);
    }
    pub fn clear(&self) {
        self.0.lock().unwrap().nodes.clear();
    }
    fn target(&self, from: u64, to: u64) -> Option<Node> {
        let links = self.0.lock().unwrap();
        if links.isolated.contains(&from) || links.isolated.contains(&to) {
            None
        } else {
            links.nodes.get(&to).cloned()
        }
    }
}
#[derive(Clone)]
pub struct Factory {
    pub from: u64,
    pub transport: Transport,
    pub audit: Audit,
}
pub struct Connection {
    factory: Factory,
    to: u64,
}
impl RaftNetworkFactory<TypeConfig> for Factory {
    type Network = Connection;
    async fn new_client(&mut self, target: u64, _: &()) -> Connection {
        Connection {
            factory: self.clone(),
            to: target,
        }
    }
}
impl Connection {
    // OpenRaft requires this concrete RPC error type at the network boundary.
    #[allow(clippy::result_large_err)]
    fn send<E: std::error::Error>(
        &self,
        label: &str,
        payload: Value,
    ) -> Result<(Node, u64), RPCError<u64, (), E>> {
        let id = self.factory.audit.message();
        self.factory.audit.rpc(
            self.factory.from,
            self.to,
            id,
            Kind::Sent,
            label,
            payload.clone(),
        );
        let Some(target) = self.factory.transport.target(self.factory.from, self.to) else {
            self.factory.audit.rpc(
                self.factory.from,
                self.to,
                id,
                Kind::Dropped,
                "Transport unavailable (simulated)",
                payload,
            );
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other("isolated or stopped"),
            )));
        };
        self.factory.audit.rpc(
            self.to,
            self.factory.from,
            id,
            Kind::Received,
            label,
            payload,
        );
        Ok((target, id))
    }
    // Preserve the OpenRaft RPC error contract without an extra boxing layer.
    #[allow(clippy::result_large_err)]
    fn response<T: serde::Serialize, E: std::error::Error + serde::Serialize>(
        &self,
        id: u64,
        label: &str,
        result: Result<T, E>,
    ) -> Result<T, RPCError<u64, (), E>> {
        let payload = serde_json::to_value(&result).expect("RPC result serializes");
        self.factory.audit.rpc(
            self.to,
            self.factory.from,
            id,
            Kind::ResponseSent,
            label,
            payload.clone(),
        );
        // An isolation introduced while the RPC was processed also blocks its response.
        if self
            .factory
            .transport
            .target(self.to, self.factory.from)
            .is_none()
        {
            self.factory.audit.rpc(
                self.to,
                self.factory.from,
                id,
                Kind::Dropped,
                "Response unavailable (simulated)",
                payload,
            );
            return Err(RPCError::Unreachable(Unreachable::new(
                &std::io::Error::other("response blocked"),
            )));
        }
        self.factory.audit.rpc(
            self.factory.from,
            self.to,
            id,
            Kind::ResponseReceived,
            label,
            payload,
        );
        result.map_err(|error| RPCError::RemoteError(RemoteError::new(self.to, error)))
    }
}
impl RaftNetwork<TypeConfig> for Connection {
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<TypeConfig>,
        _: RPCOption,
    ) -> Result<AppendEntriesResponse<u64>, RPCError<u64, (), RaftError<u64>>> {
        let (node, id) = self.send("AppendEntries", serde_json::to_value(&rpc).unwrap())?;
        let result = node.append_entries(rpc).await;
        self.response(id, "AppendEntries response", result)
    }
    async fn vote(
        &mut self,
        rpc: VoteRequest<u64>,
        _: RPCOption,
    ) -> Result<VoteResponse<u64>, RPCError<u64, (), RaftError<u64>>> {
        let (node, id) = self.send("RequestVote", serde_json::to_value(&rpc).unwrap())?;
        let result = node.vote(rpc).await;
        self.response(id, "RequestVote response", result)
    }
    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<TypeConfig>,
        _: RPCOption,
    ) -> Result<InstallSnapshotResponse<u64>, RPCError<u64, (), RaftError<u64, InstallSnapshotError>>>
    {
        let (node, id) = self.send("InstallSnapshot", serde_json::to_value(&rpc).unwrap())?;
        let result = node.install_snapshot(rpc).await;
        self.response(id, "InstallSnapshot response", result)
    }
    fn backoff(&self) -> Backoff {
        Backoff::new(std::iter::repeat(Duration::from_millis(100)))
    }
}
