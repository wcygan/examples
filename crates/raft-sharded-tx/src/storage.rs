use crate::events::{NodeEvent, emit};
use crate::model::{ApplyResult, Command, Decision, ShardView, TxOutcome, TypeConfig};
use openraft::storage::{
    LogFlushed, RaftLogReader, RaftLogStorage, RaftSnapshotBuilder, RaftStateMachine, Snapshot,
    SnapshotMeta,
};
use openraft::{
    Entry, EntryPayload, ErrorSubject, ErrorVerb, LogId, LogState, StorageError, StoredMembership,
    Vote,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Machine {
    shard: String,
    balances: BTreeMap<String, i64>,
    prepared: BTreeMap<String, (String, i64)>,
    outcomes: BTreeMap<String, TxOutcome>,
}
impl Machine {
    fn new(shard: String) -> Self {
        let (account, balance) = if shard == "source" {
            ("source-account", 100)
        } else {
            ("destination-account", 20)
        };
        Self {
            shard,
            balances: BTreeMap::from([(account.to_owned(), balance)]),
            prepared: BTreeMap::new(),
            outcomes: BTreeMap::new(),
        }
    }
    fn view(&self) -> ShardView {
        ShardView {
            shard: self.shard.clone(),
            balances: self.balances.clone(),
            prepared: self
                .prepared
                .iter()
                .map(|(tx, (account, _))| (tx.clone(), account.clone()))
                .collect(),
            outcomes: self.outcomes.clone(),
        }
    }
    fn apply(&mut self, cmd: &Command) -> ApplyResult {
        match cmd {
            Command::Prepare {
                tx_id,
                account,
                delta,
            } => {
                match self.outcomes.get(tx_id) {
                    Some(TxOutcome::Abort) => {
                        return ApplyResult::Prepared {
                            yes: false,
                            reason: Some("aborted tombstone".into()),
                        };
                    }
                    Some(TxOutcome::Commit) => {
                        return ApplyResult::Prepared {
                            yes: true,
                            reason: Some("already committed".into()),
                        };
                    }
                    Some(TxOutcome::Prepared) => {
                        return ApplyResult::Prepared {
                            yes: true,
                            reason: None,
                        };
                    }
                    None => {}
                }
                if let Some((existing, amount)) = self.prepared.get(tx_id) {
                    return if existing == account && amount == delta {
                        ApplyResult::Prepared {
                            yes: true,
                            reason: None,
                        }
                    } else {
                        ApplyResult::Prepared {
                            yes: false,
                            reason: Some("conflicting duplicate prepare".into()),
                        }
                    };
                }
                if self.prepared.values().any(|(locked, _)| locked == account) {
                    return ApplyResult::Prepared {
                        yes: false,
                        reason: Some("account already prepared by another transaction".into()),
                    };
                }
                let Some(balance) = self.balances.get(account) else {
                    return ApplyResult::Prepared {
                        yes: false,
                        reason: Some("unknown account".into()),
                    };
                };
                if balance.checked_add(*delta).is_none_or(|next| next < 0) {
                    return ApplyResult::Prepared {
                        yes: false,
                        reason: Some("insufficient funds or balance overflow".into()),
                    };
                }
                self.prepared
                    .insert(tx_id.clone(), (account.clone(), *delta));
                self.outcomes.insert(tx_id.clone(), TxOutcome::Prepared);
                ApplyResult::Prepared {
                    yes: true,
                    reason: None,
                }
            }
            Command::Decide {
                tx_id,
                decision: Decision::Commit,
            } => {
                match self.outcomes.get(tx_id) {
                    Some(TxOutcome::Abort) => {
                        return ApplyResult::Rejected {
                            reason: "abort tombstone prevents commit".into(),
                        };
                    }
                    Some(TxOutcome::Commit) => return ApplyResult::Committed,
                    _ => {}
                }
                let Some((account, delta)) = self.prepared.remove(tx_id) else {
                    return ApplyResult::Rejected {
                        reason: "commit requires a replicated prepared intent".into(),
                    };
                };
                *self.balances.entry(account).or_default() += delta;
                self.outcomes.insert(tx_id.clone(), TxOutcome::Commit);
                ApplyResult::Committed
            }
            Command::Decide {
                tx_id,
                decision: Decision::Abort,
            } => {
                if self.outcomes.get(tx_id) == Some(&TxOutcome::Commit) {
                    return ApplyResult::Rejected {
                        reason: "commit decision is immutable".into(),
                    };
                }
                self.prepared.remove(tx_id);
                self.outcomes.insert(tx_id.clone(), TxOutcome::Abort);
                ApplyResult::Aborted
            }
        }
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<RwLock<Inner>>,
    node_id: u64,
    shard: String,
}
struct Inner {
    vote: Option<Vote<u64>>,
    committed: Option<LogId<u64>>,
    log: BTreeMap<u64, Entry<TypeConfig>>,
    last_purged: Option<LogId<u64>>,
    machine: Machine,
    last_applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, openraft::BasicNode>,
    snapshot: Option<(SnapshotMeta<u64, openraft::BasicNode>, Vec<u8>)>,
}
impl Store {
    pub fn new(node_id: u64, shard: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                vote: None,
                committed: None,
                log: BTreeMap::new(),
                last_purged: None,
                machine: Machine::new(shard.clone()),
                last_applied: None,
                membership: StoredMembership::default(),
                snapshot: None,
            })),
            node_id,
            shard,
        }
    }
    pub async fn view(&self) -> ShardView {
        self.inner.read().await.machine.view()
    }
    pub async fn status(&self, tx_id: &str) -> (Option<TxOutcome>, bool) {
        let g = self.inner.read().await;
        (
            g.machine.outcomes.get(tx_id).cloned(),
            g.machine.prepared.contains_key(tx_id),
        )
    }
    fn io_error(e: impl std::error::Error + 'static) -> StorageError<u64> {
        StorageError::from_io_error(
            ErrorSubject::StateMachine,
            ErrorVerb::Write,
            std::io::Error::other(e.to_string()),
        )
    }
    fn emit_apply(&self, tx: &str, index: u64, result: &ApplyResult) {
        emit(NodeEvent {
            kind: "state_machine_applied".into(),
            node_id: self.node_id,
            shard: self.shard.clone(),
            peer_id: None,
            tx_id: Some(tx.into()),
            log_index: Some(index),
            detail: format!("{result:?}"),
        });
    }
}

#[derive(Serialize, Deserialize)]
struct SnapshotData {
    machine: Machine,
    last_applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, openraft::BasicNode>,
}

impl RaftLogReader<TypeConfig> for Store {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + Send>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        Ok(self
            .inner
            .read()
            .await
            .log
            .range(range)
            .map(|(_, e)| e.clone())
            .collect())
    }
}
impl RaftLogStorage<TypeConfig> for Store {
    type LogReader = Store;
    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<u64>> {
        let g = self.inner.read().await;
        Ok(LogState {
            last_purged_log_id: g.last_purged,
            last_log_id: g
                .log
                .last_key_value()
                .map(|(_, e)| e.log_id)
                .or(g.last_purged),
        })
    }
    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }
    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        self.inner.write().await.vote = Some(*vote);
        Ok(())
    }
    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        Ok(self.inner.read().await.vote)
    }
    async fn save_committed(
        &mut self,
        committed: Option<LogId<u64>>,
    ) -> Result<(), StorageError<u64>> {
        self.inner.write().await.committed = committed;
        Ok(())
    }
    async fn read_committed(&mut self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        Ok(self.inner.read().await.committed)
    }
    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + Send,
        I::IntoIter: Send,
    {
        let mut g = self.inner.write().await;
        for entry in entries {
            g.log.insert(entry.log_id.index, entry);
        }
        callback.log_io_completed(Ok(()));
        Ok(())
    }
    async fn truncate(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        self.inner.write().await.log.split_off(&log_id.index);
        Ok(())
    }
    async fn purge(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut g = self.inner.write().await;
        g.log.retain(|idx, _| *idx > log_id.index);
        g.last_purged = Some(log_id);
        Ok(())
    }
}

impl RaftStateMachine<TypeConfig> for Store {
    type SnapshotBuilder = Store;
    async fn applied_state(
        &mut self,
    ) -> Result<
        (
            Option<LogId<u64>>,
            StoredMembership<u64, openraft::BasicNode>,
        ),
        StorageError<u64>,
    > {
        let g = self.inner.read().await;
        Ok((g.last_applied, g.membership.clone()))
    }
    async fn apply<I>(&mut self, entries: I) -> Result<Vec<ApplyResult>, StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + Send,
        I::IntoIter: Send,
    {
        let mut results = Vec::new();
        for entry in entries {
            let index = entry.log_id.index;
            let tx_id = match &entry.payload {
                EntryPayload::Normal(cmd) => {
                    let tx = cmd.tx_id().to_owned();
                    let result = self.inner.write().await.machine.apply(cmd);
                    self.emit_apply(&tx, index, &result);
                    results.push(result);
                    Some(tx)
                }
                EntryPayload::Membership(membership) => {
                    let mut g = self.inner.write().await;
                    g.membership = StoredMembership::new(Some(entry.log_id), membership.clone());
                    results.push(ApplyResult::Noop);
                    None
                }
                EntryPayload::Blank => {
                    results.push(ApplyResult::Noop);
                    None
                }
            };
            let _ = tx_id;
            self.inner.write().await.last_applied = Some(entry.log_id);
        }
        Ok(results)
    }
    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        self.clone()
    }
    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }
    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, openraft::BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let data: SnapshotData =
            serde_json::from_slice(&snapshot.into_inner()).map_err(Self::io_error)?;
        let mut g = self.inner.write().await;
        g.machine = data.machine;
        g.last_applied = data.last_applied;
        g.membership = data.membership;
        g.snapshot = Some((
            meta.clone(),
            serde_json::to_vec(&SnapshotData {
                machine: g.machine.clone(),
                last_applied: g.last_applied,
                membership: g.membership.clone(),
            })
            .map_err(Self::io_error)?,
        ));
        Ok(())
    }
    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<u64>> {
        let g = self.inner.read().await;
        let Some((meta, bytes)) = &g.snapshot else {
            return Ok(None);
        };
        Ok(Some(Snapshot {
            meta: meta.clone(),
            snapshot: Box::new(Cursor::new(bytes.clone())),
        }))
    }
}
impl RaftSnapshotBuilder<TypeConfig> for Store {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<u64>> {
        let mut g = self.inner.write().await;
        let data = SnapshotData {
            machine: g.machine.clone(),
            last_applied: g.last_applied,
            membership: g.membership.clone(),
        };
        let bytes = serde_json::to_vec(&data).map_err(Self::io_error)?;
        let meta = SnapshotMeta {
            last_log_id: g.last_applied,
            last_membership: g.membership.clone(),
            snapshot_id: format!(
                "{}-{}",
                self.node_id,
                g.last_applied.as_ref().map(|l| l.index).unwrap_or(0)
            ),
        };
        g.snapshot = Some((meta.clone(), bytes.clone()));
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(bytes)),
        })
    }
}
