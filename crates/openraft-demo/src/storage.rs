//! Volatile storage implementing OpenRaft's log and state-machine contracts.
use crate::{
    audit::{Audit, Kind},
    TypeConfig,
};
use openraft::{
    storage::{LogFlushed, LogState, RaftLogStorage, RaftStateMachine},
    Entry, EntryPayload, LogId, OptionalSend, RaftLogReader, RaftSnapshotBuilder, Snapshot,
    SnapshotMeta, StorageError, StorageIOError, StoredMembership, Vote,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::BTreeMap, fmt::Debug, io::Cursor, ops::RangeBounds, sync::Arc};
use tokio::sync::Mutex;

#[derive(Default)]
struct Logs {
    entries: BTreeMap<u64, Entry<TypeConfig>>,
    vote: Option<Vote<u64>>,
    purged: Option<LogId<u64>>,
    committed: Option<LogId<u64>>,
}
#[derive(Clone)]
pub struct LogStore {
    data: Arc<Mutex<Logs>>,
    id: u64,
    audit: Audit,
}
impl LogStore {
    pub fn new(id: u64, audit: Audit) -> Self {
        Self {
            data: Arc::default(),
            id,
            audit,
        }
    }
}
impl RaftLogReader<TypeConfig> for LogStore {
    async fn try_get_log_entries<R: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: R,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        Ok(self
            .data
            .lock()
            .await
            .entries
            .range(range)
            .map(|(_, e)| e.clone())
            .collect())
    }
}
impl RaftLogStorage<TypeConfig> for LogStore {
    type LogReader = Self;
    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<u64>> {
        let data = self.data.lock().await;
        Ok(LogState {
            last_purged_log_id: data.purged,
            last_log_id: data
                .entries
                .last_key_value()
                .map(|(_, e)| e.log_id)
                .or(data.purged),
        })
    }
    async fn get_log_reader(&mut self) -> Self {
        self.clone()
    }
    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        self.data.lock().await.vote = Some(*vote);
        self.audit
            .emit(self.id, Kind::VoteSaved, "Vote saved", json!(vote), |n| {
                n.term = vote.leader_id.term;
            });
        Ok(())
    }
    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        Ok(self.data.lock().await.vote)
    }
    async fn save_committed(&mut self, log: Option<LogId<u64>>) -> Result<(), StorageError<u64>> {
        let mut data = self.data.lock().await;
        if data.committed != log {
            data.committed = log;
            self.audit.emit(
                self.id,
                Kind::Committed,
                "Commit index advanced",
                json!(log),
                |n| {
                    n.committed = log.map(|l| l.index);
                },
            );
        }
        Ok(())
    }
    async fn read_committed(&mut self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        Ok(self.data.lock().await.committed)
    }
    async fn append<I>(
        &mut self,
        entries: I,
        callback: LogFlushed<TypeConfig>,
    ) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut data = self.data.lock().await;
        for entry in entries {
            let payload = json!(entry);
            let index = entry.log_id.index;
            data.entries.insert(index, entry);
            self.audit.emit(
                self.id,
                Kind::LogAppended,
                format!("Append log #{index}"),
                payload.clone(),
                |n| {
                    n.log.insert(index, payload);
                },
            );
        }
        callback.log_io_completed(Ok(()));
        Ok(())
    }
    async fn truncate(&mut self, log: LogId<u64>) -> Result<(), StorageError<u64>> {
        self.data
            .lock()
            .await
            .entries
            .retain(|index, _| *index < log.index);
        self.audit.emit(
            self.id,
            Kind::LogTruncated,
            "Truncate conflicting suffix",
            json!(log),
            |n| n.log.retain(|index, _| *index < log.index),
        );
        Ok(())
    }
    async fn purge(&mut self, log: LogId<u64>) -> Result<(), StorageError<u64>> {
        let mut data = self.data.lock().await;
        data.purged = Some(log);
        data.entries.retain(|index, _| *index > log.index);
        self.audit.emit(
            self.id,
            Kind::LogPurged,
            "Purge snapshotted prefix",
            json!(log),
            |n| n.log.retain(|index, _| *index > log.index),
        );
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct State {
    applied: Option<LogId<u64>>,
    membership: StoredMembership<u64, ()>,
    values: BTreeMap<String, String>,
}
#[derive(Default)]
struct MachineData {
    state: State,
    snapshot: Option<(SnapshotMeta<u64, ()>, Vec<u8>)>,
    serial: u64,
}
#[derive(Clone)]
pub struct Machine {
    data: Arc<Mutex<MachineData>>,
    id: u64,
    audit: Audit,
}
impl Machine {
    pub fn new(id: u64, audit: Audit) -> Self {
        Self {
            data: Arc::default(),
            id,
            audit,
        }
    }
}
impl RaftSnapshotBuilder<TypeConfig> for Machine {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<u64>> {
        let mut data = self.data.lock().await;
        let bytes =
            serde_json::to_vec(&data.state).map_err(|e| StorageIOError::read_state_machine(&e))?;
        data.serial += 1;
        let meta = SnapshotMeta {
            last_log_id: data.state.applied,
            last_membership: data.state.membership.clone(),
            snapshot_id: format!("{}-{}", self.id, data.serial),
        };
        data.snapshot = Some((meta.clone(), bytes.clone()));
        self.audit.emit(
            self.id,
            Kind::SnapshotBuilt,
            "Snapshot built",
            json!(meta),
            |_| {},
        );
        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(bytes)),
        })
    }
}
impl RaftStateMachine<TypeConfig> for Machine {
    type SnapshotBuilder = Self;
    async fn applied_state(
        &mut self,
    ) -> Result<(Option<LogId<u64>>, StoredMembership<u64, ()>), StorageError<u64>> {
        let data = self.data.lock().await;
        Ok((data.state.applied, data.state.membership.clone()))
    }
    async fn apply<I>(&mut self, entries: I) -> Result<Vec<Option<String>>, StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let mut data = self.data.lock().await;
        let mut responses = vec![];
        for entry in entries {
            data.state.applied = Some(entry.log_id);
            let response = match &entry.payload {
                EntryPayload::Normal(command) => data
                    .state
                    .values
                    .insert(command.key.clone(), command.value.clone()),
                EntryPayload::Membership(m) => {
                    data.state.membership = StoredMembership::new(Some(entry.log_id), m.clone());
                    None
                }
                EntryPayload::Blank => None,
            };
            self.audit.emit(
                self.id,
                Kind::Applied,
                format!("Apply log #{}", entry.log_id.index),
                json!(entry),
                |n| {
                    n.applied = Some(entry.log_id.index);
                    n.values = data.state.values.clone();
                },
            );
            responses.push(response);
        }
        Ok(responses)
    }
    async fn get_snapshot_builder(&mut self) -> Self {
        self.clone()
    }
    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(vec![])))
    }
    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, ()>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let bytes = snapshot.into_inner();
        let state: State =
            serde_json::from_slice(&bytes).map_err(|e| StorageIOError::read_state_machine(&e))?;
        let mut data = self.data.lock().await;
        data.state = state;
        data.snapshot = Some((meta.clone(), bytes));
        self.audit.emit(
            self.id,
            Kind::SnapshotInstalled,
            "Snapshot installed",
            json!(meta),
            |n| {
                n.applied = data.state.applied.map(|l| l.index);
                n.values = data.state.values.clone();
            },
        );
        Ok(())
    }
    async fn get_current_snapshot(
        &mut self,
    ) -> Result<Option<Snapshot<TypeConfig>>, StorageError<u64>> {
        Ok(self
            .data
            .lock()
            .await
            .snapshot
            .as_ref()
            .map(|(meta, bytes)| Snapshot {
                meta: meta.clone(),
                snapshot: Box::new(Cursor::new(bytes.clone())),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Builder;
    impl openraft::testing::StoreBuilder<TypeConfig, LogStore, Machine> for Builder {
        async fn build(&self) -> Result<((), LogStore, Machine), StorageError<u64>> {
            let audit = Audit::default();
            Ok(((), LogStore::new(1, audit.clone()), Machine::new(1, audit)))
        }
    }
    #[test]
    fn openraft_storage_contracts() {
        openraft::testing::Suite::test_all(Builder).unwrap();
    }
}
