//! Scenario harness. All consensus decisions come from OpenRaft.
use crate::{
    audit::{Audit, Kind},
    network::{Factory, Transport},
    storage::{LogStore, Machine},
    Command, Node,
};
use openraft::{Config, ServerState};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::task::JoinHandle;

type Error = Box<dyn std::error::Error + Send + Sync>;
pub const SCENARIOS: [&str; 4] = ["replication", "leader-loss", "quorum-loss", "catch-up"];
struct Cluster {
    nodes: BTreeMap<u64, Node>,
    transport: Transport,
    audit: Audit,
    watchers: Vec<JoinHandle<()>>,
}
impl Cluster {
    async fn new(audit: Audit) -> Result<Self, Error> {
        let config = Arc::new(
            Config {
                cluster_name: "teaching".into(),
                enable_elect: false,
                heartbeat_interval: 100,
                election_timeout_min: 500,
                election_timeout_max: 1000,
                ..Config::default()
            }
            .validate()?,
        );
        let mut cluster = Self {
            nodes: BTreeMap::new(),
            transport: Transport::default(),
            audit,
            watchers: vec![],
        };
        for id in cluster.audit.nodes().keys().copied().collect::<Vec<_>>() {
            let raft = Node::new(
                id,
                config.clone(),
                Factory {
                    from: id,
                    transport: cluster.transport.clone(),
                    audit: cluster.audit.clone(),
                },
                LogStore::new(id, cluster.audit.clone()),
                Machine::new(id, cluster.audit.clone()),
            )
            .await?;
            cluster.watch(id, &raft);
            cluster.transport.insert(id, raft.clone());
            cluster.nodes.insert(id, raft);
        }
        Ok(cluster)
    }
    fn watch(&mut self, id: u64, node: &Node) {
        let mut receiver = node.metrics();
        let audit = self.audit.clone();
        self.watchers.push(tokio::spawn(async move {
            loop {
                let metrics = receiver.borrow_and_update().clone();
                audit.emit(
                    id,
                    Kind::Metrics,
                    format!("{:?} · term {}", metrics.state, metrics.current_term),
                    json!(metrics),
                    |n| {
                        n.role = format!("{:?}", metrics.state);
                        n.term = metrics.current_term;
                        n.leader = metrics.current_leader;
                    },
                );
                if receiver.changed().await.is_err() {
                    break;
                }
            }
        }));
    }
    async fn bootstrap(&self) -> Result<(), Error> {
        self.nodes[&1]
            .initialize(
                self.nodes
                    .keys()
                    .map(|id| (*id, ()))
                    .collect::<BTreeMap<_, _>>(),
            )
            .await?;
        self.elect(1).await
    }
    async fn elect(&self, id: u64) -> Result<(), Error> {
        self.nodes[&id].trigger().elect().await?;
        self.nodes[&id]
            .wait(Some(Duration::from_secs(4)))
            .state(ServerState::Leader, "leader elected")
            .await?;
        Ok(())
    }
    async fn write(&self, id: u64, value: &str) -> Result<(), Error> {
        let command = Command {
            key: "lesson".into(),
            value: value.into(),
        };
        self.audit.emit(
            id,
            Kind::ClientWrite,
            format!("Write lesson = {value}"),
            json!(command),
            |_| {},
        );
        let result = tokio::time::timeout(
            Duration::from_millis(700),
            self.nodes[&id].client_write(command),
        )
        .await;
        match result {
            Ok(Ok(response)) => self.audit.emit(
                id,
                Kind::ClientAcknowledged,
                "Client write acknowledged",
                json!(response),
                |_| {},
            ),
            Ok(Err(error)) => return Err(Box::new(error)),
            Err(_) => self.audit.emit(
                id,
                Kind::ClientTimeout,
                "Client timed out · outcome unknown",
                json!({"timeout_ms": 700}),
                |_| {},
            ),
        }
        Ok(())
    }
    async fn applied(&self, ids: &[u64], value: &str) -> Result<(), Error> {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let nodes = self.audit.nodes();
                if ids
                    .iter()
                    .all(|id| nodes[id].values.get("lesson").is_some_and(|v| v == value))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
    fn isolate(&self, id: u64, value: bool) {
        self.transport.isolate(id, value);
        self.audit.emit(
            id,
            Kind::Fault,
            if value {
                "Isolated (simulated)"
            } else {
                "Network restored"
            },
            json!({"isolated": value}),
            |n| n.isolated = value,
        );
    }
    async fn scenario(&self, scenario: &str) -> Result<(), Error> {
        self.bootstrap().await?;
        self.write(1, "first").await?;
        let all: Vec<_> = self.nodes.keys().copied().collect();
        let followers: Vec<_> = all.iter().copied().filter(|id| *id != 1).collect();
        let last = *all.last().unwrap();
        self.applied(&all, "first").await?;
        match scenario {
            "replication" => {}
            "leader-loss" => {
                self.transport.remove(1);
                self.nodes[&1].shutdown().await?;
                self.audit.emit(
                    1,
                    Kind::Fault,
                    "Leader stopped (simulated)",
                    json!({"stopped": true}),
                    |n| n.available = false,
                );
                // Allow the surviving follower's old leader lease to expire.
                tokio::time::sleep(Duration::from_millis(1100)).await;
                self.elect(2).await?;
                self.write(2, "after election").await?;
                self.applied(&followers, "after election").await?;
            }
            "quorum-loss" => {
                self.isolate(1, true);
                self.write(1, "without quorum").await?;
                if self.audit.nodes().values().any(|n| {
                    n.values
                        .get("lesson")
                        .is_some_and(|v| v == "without quorum")
                }) {
                    return Err("A write applied without quorum".into());
                }
            }
            "catch-up" => {
                self.isolate(last, true);
                self.write(1, "while isolated").await?;
                let connected: Vec<_> = all.iter().copied().filter(|id| *id != last).collect();
                self.applied(&connected, "while isolated").await?;
                self.isolate(last, false);
                self.nodes[&1].trigger().heartbeat().await?;
                self.applied(&all, "while isolated").await?;
            }
            _ => return Err("Unknown scenario".into()),
        }
        Ok(())
    }
    async fn shutdown(mut self) {
        for watcher in &self.watchers {
            watcher.abort();
        }
        for node in self.nodes.values() {
            let _ = node.shutdown().await;
        }
        self.transport.clear();
        self.nodes.clear();
    }
}
impl Drop for Cluster {
    fn drop(&mut self) {
        for watcher in &self.watchers {
            watcher.abort();
        }
        self.transport.clear();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            for node in std::mem::take(&mut self.nodes).into_values() {
                runtime.spawn(async move {
                    let _ = node.shutdown().await;
                });
            }
        }
    }
}
pub async fn run(scenario: &str, audit: Audit) -> Result<(), String> {
    if !SCENARIOS.contains(&scenario) {
        return Err("Unknown scenario".into());
    }
    audit.emit(
        1,
        Kind::Started,
        format!("Start {scenario}"),
        json!({"scenario": scenario, "node_count": audit.nodes().len()}),
        |_| {},
    );
    let cluster = match Cluster::new(audit.clone()).await {
        Ok(cluster) => cluster,
        Err(error) => {
            let error = error.to_string();
            audit.finish(Some(&error));
            return Err(error);
        }
    };
    let result = cluster.scenario(scenario).await.map_err(|e| e.to_string());
    audit.finish(result.as_ref().err().map(String::as_str));
    cluster.shutdown().await;
    result
}
