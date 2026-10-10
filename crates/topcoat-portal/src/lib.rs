//! Web process supervisor and durable coordinator-decision journal.
use raft_sharded_tx::{
    client::{Admin, AdminReply, admin},
    cluster::{NodeProcess, initialize_group, leader},
    model::{ApplyResult, Command, Decision, NodeInfo},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, Notify, broadcast};

static RUNTIME: std::sync::OnceLock<Arc<PortalRuntime>> = std::sync::OnceLock::new();
static SHUTDOWN: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();
pub fn set_runtime(runtime: Arc<PortalRuntime>) -> Result<(), Arc<PortalRuntime>> {
    RUNTIME.set(runtime)
}
pub fn runtime() -> &'static Arc<PortalRuntime> {
    RUNTIME.get().expect("portal runtime initialized")
}
pub fn signal_shutdown() {
    SHUTDOWN
        .get_or_init(|| Arc::new(Notify::new()))
        .notify_waiters();
}
pub async fn wait_for_shutdown() {
    SHUTDOWN
        .get_or_init(|| Arc::new(Notify::new()))
        .notified()
        .await;
}

const EVENT_CAPACITY: usize = 512;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalEvent {
    pub id: u64,
    pub kind: String,
    pub tx_id: Option<String>,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalStatus {
    pub source_available: bool,
    pub destination_available: bool,
    pub source_balance: i64,
    pub destination_balance: i64,
    pub source_nodes: Vec<u64>,
    pub destination_nodes: Vec<u64>,
    pub prepared_source: Vec<String>,
    pub prepared_destination: Vec<String>,
    pub decisions: BTreeMap<String, Decision>,
    pub active: BTreeMap<String, i64>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct TransferRequest {
    pub tx_id: Option<String>,
    pub amount: i64,
    #[serde(default)]
    pub fault: Fault,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    #[default]
    None,
    NoVote,
    MissingVote,
    PauseAfterPrepare,
    PartialDecisionDelivery,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub enum TransferResult {
    CommitAcknowledged,
    AbortAcknowledged,
    InDoubt { reason: String },
    DecisionDurableDeliveryUncertain { decision: Decision, reason: String },
    PausedPrepared { tx_id: String },
    PartialDecisionDelivery { tx_id: String, decision: Decision },
}

fn decision_was_applied(
    decision: Decision,
    result: &Result<raft_sharded_tx::model::ApplyResult, String>,
) -> bool {
    matches!(
        (decision, result),
        (
            Decision::Commit,
            Ok(raft_sharded_tx::model::ApplyResult::Committed)
        ) | (
            Decision::Abort,
            Ok(raft_sharded_tx::model::ApplyResult::Aborted)
        )
    )
}
#[derive(Clone, Debug, Serialize)]
pub struct NodeOperationResult {
    pub node_id: Option<u64>,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum JournalRecord {
    Begin { tx_id: String, amount: i64 },
    Decision { tx_id: String, decision: Decision },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalDecision {
    pub tx_id: String,
    pub decision: Decision,
}

struct Journal {
    path: PathBuf,
    begins: BTreeMap<String, i64>,
    decisions: BTreeMap<String, Decision>,
}
impl Journal {
    fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let mut j = Self {
            path: path.clone(),
            begins: BTreeMap::new(),
            decisions: BTreeMap::new(),
        };
        if path.exists() {
            let bytes = fs::read(&path)?;
            let valid_len = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
            if valid_len < bytes.len() {
                let f = OpenOptions::new().write(true).open(&path)?;
                f.set_len(valid_len as u64)?;
                f.sync_all()?;
            }
            let complete = std::str::from_utf8(&bytes[..valid_len]).map_err(io::Error::other)?;
            for line in complete.lines() {
                let r: JournalRecord = serde_json::from_str(line).map_err(io::Error::other)?;
                j.apply(r)?;
            }
        }
        Ok(j)
    }
    fn apply(&mut self, r: JournalRecord) -> io::Result<()> {
        match r {
            JournalRecord::Begin { tx_id, amount } => {
                if let Some(prior) = self.begins.get(&tx_id)
                    && *prior != amount
                {
                    return Err(io::Error::other(
                        "transaction id reused with different amount",
                    ));
                }
                self.begins.insert(tx_id, amount);
            }
            JournalRecord::Decision { tx_id, decision } => {
                if let Some(prior) = self.decisions.get(&tx_id) {
                    if *prior != decision {
                        return Err(io::Error::other("attempted to change immutable decision"));
                    }
                } else {
                    self.decisions.insert(tx_id, decision);
                }
            }
        }
        Ok(())
    }
    fn append(&mut self, r: JournalRecord) -> io::Result<()> {
        match &r {
            JournalRecord::Begin { tx_id, amount } => {
                if self.begins.get(tx_id).is_some_and(|v| v != amount) {
                    return Err(io::Error::other(
                        "transaction id reused with different amount",
                    ));
                }
            }
            JournalRecord::Decision { tx_id, decision } => {
                if self.decisions.get(tx_id).is_some_and(|v| v != decision) {
                    return Err(io::Error::other("attempted to change immutable decision"));
                }
            }
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        serde_json::to_writer(&mut f, &r).map_err(io::Error::other)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        if let Some(p) = self.path.parent() {
            File::open(p)?.sync_all()?;
        }
        self.apply(r)
    }
    fn begin(&mut self, tx: String, amount: i64) -> io::Result<()> {
        if let Some(old) = self.begins.get(&tx) {
            return if *old == amount {
                Ok(())
            } else {
                Err(io::Error::other(
                    "transaction id reused with different amount",
                ))
            };
        }
        self.append(JournalRecord::Begin { tx_id: tx, amount })
    }
    fn decide(&mut self, tx: String, d: Decision) -> io::Result<()> {
        if let Some(old) = self.decisions.get(&tx) {
            return if *old == d {
                Ok(())
            } else {
                Err(io::Error::other("immutable decision conflict"))
            };
        }
        self.append(JournalRecord::Decision {
            tx_id: tx,
            decision: d,
        })
    }
}

pub struct PortalRuntime {
    source: Mutex<Vec<NodeProcess>>,
    destination: Mutex<Vec<NodeProcess>>,
    raft_binary: PathBuf,
    journal: Mutex<Journal>,
    transaction_lock: Mutex<()>,
    events: Mutex<VecDeque<PortalEvent>>,
    event_tx: broadcast::Sender<PortalEvent>,
    next_event: std::sync::atomic::AtomicU64,
    next_node: std::sync::atomic::AtomicU64,
    paused: std::sync::atomic::AtomicBool,
    resume: Notify,
}
impl PortalRuntime {
    pub async fn start(binary: &str, journal_path: impl AsRef<Path>) -> io::Result<Arc<Self>> {
        let (event_tx, _) = broadcast::channel(256);
        let mut source = Vec::new();
        let mut destination = Vec::new();
        for id in 1..=3 {
            source.push(NodeProcess::spawn(binary, id, "source").await?);
        }
        for id in 4..=6 {
            destination.push(NodeProcess::spawn(binary, id, "destination").await?);
        }
        initialize_group(&source).await.map_err(io::Error::other)?;
        initialize_group(&destination)
            .await
            .map_err(io::Error::other)?;
        leader(&source).await.map_err(io::Error::other)?;
        leader(&destination).await.map_err(io::Error::other)?;
        let journal = Journal::open(journal_path)?;
        let rt = Arc::new(Self {
            source: Mutex::new(source),
            destination: Mutex::new(destination),
            raft_binary: PathBuf::from(binary),
            journal: Mutex::new(journal),
            transaction_lock: Mutex::new(()),
            events: Mutex::new(VecDeque::new()),
            event_tx,
            next_event: std::sync::atomic::AtomicU64::new(1),
            next_node: std::sync::atomic::AtomicU64::new(7),
            paused: std::sync::atomic::AtomicBool::new(false),
            resume: Notify::new(),
        });
        {
            let mut nodes = rt.source.lock().await;
            for node in nodes.iter_mut() {
                if let Some(rx) = node.events.take() {
                    rt.watch_node_events(rx);
                }
            }
        }
        {
            let mut nodes = rt.destination.lock().await;
            for node in nodes.iter_mut() {
                if let Some(rx) = node.events.take() {
                    rt.watch_node_events(rx);
                }
            }
        }
        rt.event(
            "cluster_ready",
            None,
            "Two independent 3-node OpenRaft groups are ready",
        )
        .await;
        Ok(rt)
    }
    fn watch_node_events(
        self: &Arc<Self>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<raft_sharded_tx::events::NodeEvent>,
    ) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if e.kind == "state_machine_applied"
                    && let Some(rt) = weak.upgrade()
                {
                    rt.event(
                        "participant_applied",
                        e.tx_id.as_deref(),
                        format!(
                            "{} node {} applied log {}: {}",
                            e.shard,
                            e.node_id,
                            e.log_index
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".into()),
                            e.detail
                        ),
                    )
                    .await;
                }
            }
        });
    }
    async fn event(&self, kind: &str, tx: Option<&str>, detail: impl Into<String>) {
        let e = PortalEvent {
            id: self
                .next_event
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            kind: kind.into(),
            tx_id: tx.map(str::to_owned),
            detail: detail.into(),
        };
        let mut q = self.events.lock().await;
        q.push_back(e.clone());
        while q.len() > EVENT_CAPACITY {
            q.pop_front();
        }
        let _ = self.event_tx.send(e);
    }
    pub async fn subscribe(
        &self,
        last: Option<u64>,
    ) -> (Vec<PortalEvent>, broadcast::Receiver<PortalEvent>, bool) {
        let q = self.events.lock().await;
        let current = self
            .next_event
            .load(std::sync::atomic::Ordering::SeqCst)
            .saturating_sub(1);
        let oldest = q.front().map(|e| e.id).unwrap_or(current + 1);
        let gap = last.is_some_and(|v| v.saturating_add(1) < oldest);
        let replay = if gap {
            Vec::new()
        } else {
            q.iter()
                .filter(|e| last.is_none_or(|v| e.id > v))
                .cloned()
                .collect()
        };
        (replay, self.event_tx.subscribe(), gap)
    }
    pub async fn current_event_id(&self) -> u64 {
        self.next_event
            .load(std::sync::atomic::Ordering::SeqCst)
            .saturating_sub(1)
    }
    pub async fn status(&self) -> PortalStatus {
        let source = view_group(&self.source).await;
        let destination = view_group(&self.destination).await;
        let source_nodes = self.source.lock().await.iter().map(|n| n.id).collect();
        let destination_nodes = self.destination.lock().await.iter().map(|n| n.id).collect();
        let j = self.journal.lock().await;
        PortalStatus {
            source_available: source.is_some(),
            destination_available: destination.is_some(),
            source_balance: source
                .as_ref()
                .and_then(|v| v.balances.get("source-account").copied())
                .unwrap_or_default(),
            destination_balance: destination
                .as_ref()
                .and_then(|v| v.balances.get("destination-account").copied())
                .unwrap_or_default(),
            source_nodes,
            destination_nodes,
            prepared_source: source
                .map(|v| v.prepared.into_keys().collect())
                .unwrap_or_default(),
            prepared_destination: destination
                .map(|v| v.prepared.into_keys().collect())
                .unwrap_or_default(),
            decisions: j.decisions.clone(),
            active: j
                .begins
                .iter()
                .filter(|(k, _)| !j.decisions.contains_key(*k))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
        }
    }
    pub async fn remove_node(&self, id: u64) -> Result<(), String> {
        let _guard = self.transaction_lock.lock().await;
        {
            let j = self.journal.lock().await;
            if j.begins.keys().any(|tx| !j.decisions.contains_key(tx)) {
                return Err("membership change refused while a transaction is unresolved".into());
            }
        }
        let shard = {
            let s = self.source.lock().await;
            if s.iter().any(|n| n.id == id) {
                "source"
            } else {
                "destination"
            }
        };
        let group = if shard == "source" {
            &self.source
        } else {
            &self.destination
        };
        let mut nodes = group.lock().await;
        if nodes.len() <= 3 {
            return Err("this demo keeps at least three voters in each shard".into());
        }
        let Some(index) = nodes.iter().position(|n| n.id == id) else {
            return Err("node id not found".into());
        };
        let current = leader(&nodes).await?;
        let members = nodes
            .iter()
            .filter(|n| n.id != id)
            .map(|n| n.id)
            .collect::<Vec<_>>();
        admin(&current.addr, Admin::ChangeMembership(members))
            .await
            .map_err(|e| e.to_string())??;
        let mut removed = nodes.remove(index);
        removed.kill().await.map_err(|e| e.to_string())?;
        self.event(
            "node_removed",
            None,
            format!("node {id} removed from {shard} through Raft membership change, then stopped"),
        )
        .await;
        Ok(())
    }
    pub async fn crash_follower(&self, shard: &str) -> Result<u64, String> {
        let _guard = self.transaction_lock.lock().await;
        let group = match shard {
            "source" => &self.source,
            "destination" => &self.destination,
            _ => return Err("shard must be source or destination".into()),
        };
        let mut nodes = group.lock().await;
        if nodes.len() < 3 {
            return Err("need at least three configured voters for this fault injection".into());
        }
        let current = leader(&nodes).await?;
        for node in nodes.iter_mut() {
            if node.addr != current.addr && node.is_running().await {
                node.kill().await.map_err(|e| e.to_string())?;
                let id = node.id;
                self.event("node_process_killed",None,format!("actual child process {id} in {shard} was killed without a membership change; remaining two voters should retain quorum")).await;
                return Ok(id);
            }
        }
        Err("no running follower was found".into())
    }
    pub async fn add_node(self: &Arc<Self>, shard: &str) -> Result<u64, String> {
        let _guard = self.transaction_lock.lock().await;
        {
            let j = self.journal.lock().await;
            if j.begins.keys().any(|tx| !j.decisions.contains_key(tx)) {
                return Err("membership change refused while a transaction is unresolved".into());
            }
        }
        let group = match shard {
            "source" => &self.source,
            "destination" => &self.destination,
            _ => return Err("shard must be source or destination".into()),
        };
        let mut nodes = group.lock().await;
        if nodes.len() >= 5 {
            return Err("this demo caps each shard at five voters".into());
        }
        let id = self
            .next_node
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut node = NodeProcess::spawn(
            self.raft_binary.to_str().ok_or("non-UTF8 binary path")?,
            id,
            shard,
        )
        .await
        .map_err(|e| e.to_string())?;
        let current = leader(&nodes)
            .await
            .map_err(|e| format!("pre-add leader lookup failed: {e}"))?;
        admin(&current.addr, Admin::AddLearner(id, node.addr.clone()))
            .await
            .map_err(|e| format!("add learner RPC failed: {e}"))?
            .map_err(|e| format!("add learner rejected: {e}"))?;
        let event_rx = node.events.take();
        nodes.push(node);
        let current = leader(&nodes)
            .await
            .map_err(|e| format!("post-add leader lookup failed: {e}"))?;
        let members = nodes.iter().map(|n| n.id).collect::<Vec<_>>();
        admin(&current.addr, Admin::ChangeMembership(members))
            .await
            .map_err(|e| format!("promotion RPC failed: {e}"))?
            .map_err(|e| format!("promotion rejected: {e}"))?;
        if let Some(rx) = event_rx {
            self.watch_node_events(rx);
        }
        self.event("node_added",None,format!("node {id} joined {shard} as a caught-up learner, then was promoted through Raft membership")).await;
        Ok(id)
    }
    pub async fn resume(&self) {
        self.paused
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.resume.notify_waiters();
    }
    pub async fn stop_all(&self) {
        let mut source = self.source.lock().await;
        for n in source.iter_mut() {
            let _ = n.kill().await;
        }
        drop(source);
        let mut destination = self.destination.lock().await;
        for n in destination.iter_mut() {
            let _ = n.kill().await;
        }
    }
    pub async fn transfer(&self, req: TransferRequest) -> TransferResult {
        let _guard = self.transaction_lock.lock().await;
        let amount = req.amount;
        let tx = req.tx_id.unwrap_or_else(new_tx_id);
        if amount <= 0 || amount > 50 {
            return TransferResult::InDoubt {
                reason: "amount must be between 1 and 50".into(),
            };
        }
        if let Some(d) = self.journal.lock().await.decisions.get(&tx).copied() {
            return self.deliver(&tx, d, false).await;
        }
        if let Err(e) = self.journal.lock().await.begin(tx.clone(), amount) {
            return TransferResult::InDoubt {
                reason: e.to_string(),
            };
        }
        self.event(
            "transaction_started",
            Some(&tx),
            format!("transfer amount {amount}"),
        )
        .await;
        let source_leader = match leader_group(&self.source).await {
            Ok(x) => x,
            Err(e) => return TransferResult::InDoubt { reason: e },
        };
        let dest_leader = match leader_group(&self.destination).await {
            Ok(x) => x,
            Err(e) => return TransferResult::InDoubt { reason: e },
        };
        let source_delta = if matches!(req.fault, Fault::NoVote) {
            -1000
        } else {
            -amount
        };
        let yes_s = match timed_write(
            &source_leader,
            Command::Prepare {
                tx_id: tx.clone(),
                account: "source-account".into(),
                delta: source_delta,
            },
        )
        .await
        {
            Ok(ApplyResult::Prepared { yes, .. }) => yes,
            Err(e) => {
                return TransferResult::InDoubt {
                    reason: format!("source vote uncertain: {e}"),
                };
            }
            _ => false,
        };
        self.event(
            "participant_vote",
            Some(&tx),
            format!("source voted {}", if yes_s { "YES" } else { "NO" }),
        )
        .await;
        if matches!(req.fault, Fault::MissingVote) {
            self.event(
                "injected_missing_vote",
                Some(&tx),
                "destination did not reply; no coordinator decision will be recorded",
            )
            .await;
            return TransferResult::InDoubt{reason:"injected missing destination vote before decision; no timeout was converted to ABORT".into()};
        }
        let yes_d = if matches!(req.fault, Fault::NoVote) {
            false
        } else {
            match timed_write(
                &dest_leader,
                Command::Prepare {
                    tx_id: tx.clone(),
                    account: "destination-account".into(),
                    delta: amount,
                },
            )
            .await
            {
                Ok(ApplyResult::Prepared { yes, .. }) => yes,
                Err(e) => {
                    return TransferResult::InDoubt {
                        reason: format!("destination vote uncertain: {e}"),
                    };
                }
                _ => false,
            }
        };
        self.event(
            "participant_vote",
            Some(&tx),
            format!("destination voted {}", if yes_d { "YES" } else { "NO" }),
        )
        .await;
        if !yes_s || !yes_d {
            if let Err(e) = self
                .journal
                .lock()
                .await
                .decide(tx.clone(), Decision::Abort)
            {
                return TransferResult::InDoubt {
                    reason: e.to_string(),
                };
            }
            self.event(
                "decision_durable",
                Some(&tx),
                "ABORT synced before decision broadcast",
            )
            .await;
            return self
                .deliver(
                    &tx,
                    Decision::Abort,
                    matches!(req.fault, Fault::PartialDecisionDelivery),
                )
                .await;
        }
        self.event(
            "participants_prepared",
            Some(&tx),
            "both shards returned YES",
        )
        .await;
        if matches!(req.fault, Fault::PauseAfterPrepare) {
            self.paused.store(true, std::sync::atomic::Ordering::SeqCst);
            self.event(
                "coordinator_paused",
                Some(&tx),
                "paused before durable decision; prepared intents remain locked",
            )
            .await;
            while self.paused.load(std::sync::atomic::Ordering::SeqCst) {
                self.resume.notified().await;
            }
        }
        if let Err(e) = self
            .journal
            .lock()
            .await
            .decide(tx.clone(), Decision::Commit)
        {
            return TransferResult::InDoubt {
                reason: e.to_string(),
            };
        }
        self.event(
            "decision_durable",
            Some(&tx),
            "COMMIT synced before decision broadcast",
        )
        .await;
        self.deliver(
            &tx,
            Decision::Commit,
            matches!(req.fault, Fault::PartialDecisionDelivery),
        )
        .await
    }
    async fn deliver(&self, tx: &str, d: Decision, partial: bool) -> TransferResult {
        let s = leader_group(&self.source).await;
        let dest = leader_group(&self.destination).await;
        if let Err(e) = s {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: e,
            };
        }
        if let Err(e) = dest {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: e,
            };
        }
        let s = s.unwrap();
        let dest = dest.unwrap();
        let cmd = |decision| Command::Decide {
            tx_id: tx.into(),
            decision,
        };
        let a = timed_write(&s, cmd(d)).await;
        if let Err(e) = a {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: format!("source delivery uncertain: {e}"),
            };
        }
        if !decision_was_applied(d, &a) {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: format!("source rejected or did not apply decision: {a:?}"),
            };
        }
        self.event(
            "decision_delivered",
            Some(tx),
            "source shard applied the synced coordinator decision (participant Raft store is in-memory)",
        )
        .await;
        if partial {
            self.event(
                "delivery_injected_drop",
                Some(tx),
                "destination decision delivery intentionally withheld; click Recover to resend",
            )
            .await;
            return TransferResult::PartialDecisionDelivery {
                tx_id: tx.into(),
                decision: d,
            };
        }
        let b = timed_write(&dest, cmd(d)).await;
        if let Err(e) = b {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: format!("destination delivery uncertain: {e}"),
            };
        }
        if !decision_was_applied(d, &b) {
            return TransferResult::DecisionDurableDeliveryUncertain {
                decision: d,
                reason: format!("destination rejected or did not apply decision: {b:?}"),
            };
        }
        self.event(
            "decision_delivered",
            Some(tx),
            "destination shard applied the synced coordinator decision (participant Raft store is in-memory)",
        )
        .await;
        match d {
            Decision::Commit => TransferResult::CommitAcknowledged,
            Decision::Abort => TransferResult::AbortAcknowledged,
        }
    }
    pub async fn recover(&self, tx: &str) -> TransferResult {
        let _guard = self.transaction_lock.lock().await;
        let d = self.journal.lock().await.decisions.get(tx).copied();
        match d {
            Some(d) => self.deliver(tx, d, false).await,
            None => TransferResult::InDoubt {
                reason: "no durable decision exists; transaction remains unresolved".into(),
            },
        }
    }
    pub async fn journal_decisions(&self) -> Vec<JournalDecision> {
        self.journal
            .lock()
            .await
            .decisions
            .iter()
            .map(|(tx, d)| JournalDecision {
                tx_id: tx.clone(),
                decision: *d,
            })
            .collect()
    }
}
async fn timed_write(
    node: &NodeInfo,
    cmd: Command,
) -> Result<raft_sharded_tx::model::ApplyResult, String> {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        match admin(&node.addr, Admin::Write(cmd))
            .await
            .map_err(|e| e.to_string())?
        {
            Ok(AdminReply::Apply(r)) => Ok(r),
            Ok(r) => Err(format!("unexpected reply {r:?}")),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|_| {
        "Raft write did not receive a quorum acknowledgement before the deadline".to_owned()
    })?
}
async fn leader_group(group: &Mutex<Vec<NodeProcess>>) -> Result<NodeInfo, String> {
    let nodes = group.lock().await;
    leader(&nodes).await
}
async fn view_group(group: &Mutex<Vec<NodeProcess>>) -> Option<raft_sharded_tx::model::ShardView> {
    let nodes = group.lock().await;
    let n = leader(&nodes).await.ok()?;
    match admin(&n.addr, Admin::View).await.ok()? {
        Ok(AdminReply::View(v)) => Some(v),
        _ => None,
    }
}
fn new_tx_id() -> String {
    format!(
        "tx-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decision_log_survives_reopen_and_rejects_reversal() {
        let root = std::env::temp_dir().join(format!(
            "decision-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("decision.jsonl");
        let mut j = Journal::open(&path).unwrap();
        j.begin("tx".into(), 3).unwrap();
        j.decide("tx".into(), Decision::Commit).unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{partial").unwrap();
        f.sync_all().unwrap();
        drop(f);
        let mut reopened = Journal::open(&path).unwrap();
        assert_eq!(reopened.decisions.get("tx"), Some(&Decision::Commit));
        assert!(reopened.decide("tx".into(), Decision::Abort).is_err());
        let _ = fs::remove_dir_all(root);
    }
    #[tokio::test]
    async fn event_replay_detects_a_ring_buffer_gap() {
        let root = std::env::temp_dir().join(format!(
            "event-gap-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (event_tx, _) = broadcast::channel(1);
        let rt = PortalRuntime {
            source: Mutex::new(Vec::new()),
            destination: Mutex::new(Vec::new()),
            raft_binary: PathBuf::new(),
            journal: Mutex::new(Journal::open(root.join("journal.jsonl")).unwrap()),
            transaction_lock: Mutex::new(()),
            events: Mutex::new(VecDeque::new()),
            event_tx,
            next_event: std::sync::atomic::AtomicU64::new(1),
            next_node: std::sync::atomic::AtomicU64::new(7),
            paused: std::sync::atomic::AtomicBool::new(false),
            resume: Notify::new(),
        };
        for n in 0..513 {
            rt.event("test", None, n.to_string()).await;
        }
        let (replay, _, gap) = rt.subscribe(Some(0)).await;
        assert!(gap);
        assert!(replay.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
