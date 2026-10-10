//! Process launcher and admin helpers for the six-node local teaching cluster.
use crate::{
    client::{Admin, AdminReply, admin},
    events::NodeEvent,
    model::{Command, NodeInfo},
    node::MetricView,
};
use std::{collections::BTreeMap, io, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command as ProcessCommand},
    sync::{mpsc, oneshot},
};

pub struct NodeProcess {
    pub id: u64,
    pub shard: String,
    pub addr: String,
    pub events: Option<mpsc::UnboundedReceiver<NodeEvent>>,
    child: Child,
}
impl NodeProcess {
    pub async fn spawn(binary: &str, id: u64, shard: &str) -> io::Result<Self> {
        let mut child = ProcessCommand::new(binary)
            .args(["node", &id.to_string(), shard])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout unavailable"))?;
        let (tx, rx) = oneshot::channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(rest) = line.strip_prefix("READY ") {
                    let _ = tx.send(rest.to_owned());
                    break;
                }
                let _ = line;
            }
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json) = line.strip_prefix("EVENT ")
                    && let Ok(event) = serde_json::from_str::<NodeEvent>(json)
                {
                    let _ = event_tx.send(event);
                }
            }
        });
        let ready = tokio::time::timeout(Duration::from_secs(8), rx)
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)?;
        let mut p = ready.split_whitespace();
        let parsed_id = p.next().and_then(|x| x.parse::<u64>().ok());
        let parsed_shard = p.next();
        let addr = p.next();
        if parsed_id != Some(id) || parsed_shard != Some(shard) || addr.is_none() {
            return Err(io::Error::other(format!("bad child READY line: {ready}")));
        }
        Ok(Self {
            id,
            shard: shard.to_owned(),
            addr: addr.unwrap().to_owned(),
            events: Some(event_rx),
            child,
        })
    }
    pub async fn kill(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        self.child.start_kill()?;
        let _ = self.child.wait().await?;
        Ok(())
    }
    pub async fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}
impl Drop for NodeProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.child.start_kill();
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while std::time::Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

pub async fn initialize_group(nodes: &[NodeProcess]) -> Result<(), String> {
    let members = nodes
        .iter()
        .map(|n| (n.id, n.addr.clone()))
        .collect::<Vec<_>>();
    match admin(&nodes[0].addr, Admin::Initialize(members))
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(AdminReply::Unit) => Ok(()),
        Ok(v) => Err(format!("unexpected init reply {v:?}")),
        Err(e) => Err(e),
    }
}
pub async fn leader(nodes: &[NodeProcess]) -> Result<NodeInfo, String> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            for n in nodes {
                if let Ok(Ok(AdminReply::Metrics(m))) = admin(&n.addr, Admin::Metrics).await
                    && m.state == "Leader"
                {
                    return Ok(NodeInfo {
                        addr: n.addr.clone(),
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for Raft leader".to_owned())?
}
pub async fn metrics(nodes: &[NodeProcess]) -> BTreeMap<u64, MetricView> {
    let mut out = BTreeMap::new();
    for n in nodes {
        if let Ok(Ok(AdminReply::Metrics(m))) = admin(&n.addr, Admin::Metrics).await {
            out.insert(n.id, m);
        }
    }
    out
}
pub async fn write(
    leader: &NodeInfo,
    command: Command,
) -> Result<crate::model::ApplyResult, String> {
    match admin(&leader.addr, Admin::Write(command))
        .await
        .map_err(|e| e.to_string())?
    {
        Ok(AdminReply::Apply(v)) => Ok(v),
        Ok(v) => Err(format!("unexpected write reply {v:?}")),
        Err(e) => Err(e),
    }
}
