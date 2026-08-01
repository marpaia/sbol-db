use std::fmt::Debug;
use std::io;
use std::ops::RangeBounds;
use std::path::Path;
use std::sync::Arc;

use openraft::storage::{LogFlushed, RaftLogStorage};
use openraft::{
    Entry, LogId, LogState, OptionalSend, RaftLogReader, StorageError, StorageIOError, Vote,
};
use rocksdb::{
    ColumnFamily, ColumnFamilyDescriptor, Direction, IteratorMode, Options, WriteBatch,
    WriteOptions, DB,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{NodeId, TypeConfig};

type StorageResult<T> = Result<T, StorageError<NodeId>>;

const META_COLUMN_FAMILY: &str = "meta";
const LOG_COLUMN_FAMILY: &str = "logs";
const LAST_PURGED_KEY: &[u8] = b"last_purged_log_id";
const VOTE_KEY: &[u8] = b"vote";
const NODE_IDENTITY_KEY: &[u8] = b"node_identity";

/// Stable identity bound to one Raft log directory.
///
/// A replacement process may reopen the directory only with the same cluster
/// and node ids. This prevents accidentally starting copied or empty storage as
/// an existing voter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub cluster_id: Uuid,
    pub node_id: NodeId,
}

/// RocksDB-backed durable storage for the Raft log and consensus metadata.
///
/// This database must live at a different path from the application state
/// machine database. Every safety-critical write uses `sync = true`, so an
/// acknowledged vote or log append has reached stable storage before OpenRaft
/// is told that the operation completed.
#[derive(Clone)]
pub struct RocksLogStore {
    db: Arc<DB>,
}

impl RocksLogStore {
    pub fn open(path: impl AsRef<Path>, identity: NodeIdentity) -> io::Result<Self> {
        let mut options = Options::default();
        options.create_if_missing(true);
        options.create_missing_column_families(true);

        let column_families = [
            ColumnFamilyDescriptor::new(META_COLUMN_FAMILY, Options::default()),
            ColumnFamilyDescriptor::new(LOG_COLUMN_FAMILY, Options::default()),
        ];
        let db = DB::open_cf_descriptors(&options, path, column_families).map_err(db_error)?;

        let store = Self { db: Arc::new(db) };
        store.bind_identity(identity)?;
        Ok(store)
    }

    fn meta(&self) -> &ColumnFamily {
        self.db
            .cf_handle(META_COLUMN_FAMILY)
            .expect("Raft meta column family must exist")
    }

    fn logs(&self) -> &ColumnFamily {
        self.db
            .cf_handle(LOG_COLUMN_FAMILY)
            .expect("Raft logs column family must exist")
    }

    fn read_json<T: DeserializeOwned>(&self, key: &[u8]) -> io::Result<Option<T>> {
        self.db
            .get_cf(self.meta(), key)
            .map_err(db_error)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(invalid_data))
            .transpose()
    }

    fn write_sync(&self, batch: WriteBatch) -> io::Result<()> {
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db.write_opt(batch, &options).map_err(db_error)
    }

    fn bind_identity(&self, requested: NodeIdentity) -> io::Result<()> {
        match self.read_json::<NodeIdentity>(NODE_IDENTITY_KEY)? {
            Some(stored) if stored != requested => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Raft log belongs to cluster {} node {}, not cluster {} node {}",
                    stored.cluster_id, stored.node_id, requested.cluster_id, requested.node_id
                ),
            )),
            Some(_) => Ok(()),
            None => {
                let mut batch = WriteBatch::default();
                batch.put_cf(
                    self.meta(),
                    NODE_IDENTITY_KEY,
                    serde_json::to_vec(&requested).map_err(invalid_data)?,
                );
                self.write_sync(batch)
            }
        }
    }

    fn append_entries_sync<I>(&self, entries: I) -> io::Result<()>
    where
        I: IntoIterator<Item = Entry<TypeConfig>>,
    {
        let mut batch = WriteBatch::default();
        for entry in entries {
            batch.put_cf(
                self.logs(),
                encode_index(entry.log_id.index),
                serde_json::to_vec(&entry).map_err(invalid_data)?,
            );
        }
        self.write_sync(batch)
    }
}

impl RaftLogReader<TypeConfig> for RocksLogStore {
    async fn try_get_log_entries<RB>(&mut self, range: RB) -> StorageResult<Vec<Entry<TypeConfig>>>
    where
        RB: RangeBounds<u64> + Clone + Debug + OptionalSend,
    {
        let start_index = match range.start_bound() {
            std::ops::Bound::Included(index) => *index,
            std::ops::Bound::Excluded(index) => index.saturating_add(1),
            std::ops::Bound::Unbounded => 0,
        };
        let start = encode_index(start_index);
        let iterator = self
            .db
            .iterator_cf(self.logs(), IteratorMode::From(&start, Direction::Forward));
        let mut entries = Vec::new();

        for item in iterator {
            let (key, value) = item.map_err(|error| StorageIOError::read_logs(&error))?;
            let index = decode_index(&key).map_err(|error| StorageIOError::read_logs(&error))?;
            if !range.contains(&index) {
                break;
            }

            let entry: Entry<TypeConfig> = serde_json::from_slice(&value)
                .map_err(|error| StorageIOError::read_log_at_index(index, &error))?;
            if entry.log_id.index != index {
                let error = io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "Raft log key index {index} does not match entry index {}",
                        entry.log_id.index
                    ),
                );
                return Err(StorageIOError::read_log_at_index(index, &error).into());
            }
            entries.push(entry);
        }

        Ok(entries)
    }
}

impl RaftLogStorage<TypeConfig> for RocksLogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> StorageResult<LogState<TypeConfig>> {
        let last_purged_log_id = self
            .read_json(LAST_PURGED_KEY)
            .map_err(|error| StorageIOError::read(&error))?;
        let last_entry = self.db.iterator_cf(self.logs(), IteratorMode::End).next();
        let last_log_id = match last_entry {
            Some(item) => {
                let (_, value) = item.map_err(|error| StorageIOError::read_logs(&error))?;
                let entry: Entry<TypeConfig> = serde_json::from_slice(&value)
                    .map_err(|error| StorageIOError::read_logs(&error))?;
                Some(entry.log_id)
            }
            None => last_purged_log_id,
        };

        Ok(LogState {
            last_purged_log_id,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn save_vote(&mut self, vote: &Vote<NodeId>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();
        batch.put_cf(
            self.meta(),
            VOTE_KEY,
            serde_json::to_vec(vote).map_err(|error| StorageIOError::write_vote(&error))?,
        );
        self.write_sync(batch)
            .map_err(|error| StorageIOError::write_vote(&error).into())
    }

    async fn read_vote(&mut self) -> StorageResult<Option<Vote<NodeId>>> {
        self.read_json(VOTE_KEY)
            .map_err(|error| StorageIOError::read_vote(&error).into())
    }

    async fn append<I>(&mut self, entries: I, callback: LogFlushed<TypeConfig>) -> StorageResult<()>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        callback.log_io_completed(self.append_entries_sync(entries));
        Ok(())
    }

    async fn truncate(&mut self, log_id: LogId<NodeId>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();
        batch.delete_range_cf(
            self.logs(),
            encode_index(log_id.index),
            encode_index(u64::MAX),
        );
        batch.delete_cf(self.logs(), encode_index(u64::MAX));
        self.write_sync(batch)
            .map_err(|error| StorageIOError::write_logs(&error).into())
    }

    async fn purge(&mut self, log_id: LogId<NodeId>) -> StorageResult<()> {
        let mut batch = WriteBatch::default();
        batch.put_cf(
            self.meta(),
            LAST_PURGED_KEY,
            serde_json::to_vec(&log_id).map_err(|error| StorageIOError::write_logs(&error))?,
        );
        batch.delete_range_cf(self.logs(), encode_index(0), encode_index(log_id.index));
        batch.delete_cf(self.logs(), encode_index(log_id.index));
        self.write_sync(batch)
            .map_err(|error| StorageIOError::write_logs(&error).into())
    }
}

fn encode_index(index: u64) -> [u8; 8] {
    index.to_be_bytes()
}

fn decode_index(bytes: &[u8]) -> io::Result<u64> {
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid Raft log index key length: {}", bytes.len()),
        )
    })?;
    Ok(u64::from_be_bytes(bytes))
}

fn db_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use openraft::{CommittedLeaderId, EntryPayload};
    use tempfile::tempdir;

    use super::*;
    use crate::{CommandEnvelope, ReplicatedCommand};

    fn entry(term: u64, index: u64) -> Entry<TypeConfig> {
        Entry {
            log_id: LogId::new(CommittedLeaderId::new(term, 1), index),
            payload: EntryPayload::Normal(CommandEnvelope::new(
                uuid::Uuid::from_u128(1),
                uuid::Uuid::from_u128(index as u128),
                ReplicatedCommand::Barrier,
            )),
        }
    }

    #[tokio::test]
    async fn vote_survives_reopen() {
        let directory = tempdir().unwrap();
        let identity = NodeIdentity {
            cluster_id: Uuid::from_u128(1),
            node_id: 2,
        };
        let vote = Vote::new(3, 2);
        {
            let mut store = RocksLogStore::open(directory.path(), identity).unwrap();
            store.save_vote(&vote).await.unwrap();
        }

        let mut reopened = RocksLogStore::open(directory.path(), identity).unwrap();
        assert_eq!(reopened.read_vote().await.unwrap(), Some(vote));
    }

    #[tokio::test]
    async fn append_and_read_preserve_log_order() {
        let directory = tempdir().unwrap();
        let mut store = RocksLogStore::open(
            directory.path(),
            NodeIdentity {
                cluster_id: Uuid::from_u128(1),
                node_id: 1,
            },
        )
        .unwrap();
        let entries = vec![entry(1, 1), entry(1, 2), entry(2, 3)];

        store.append_entries_sync(entries.clone()).unwrap();

        assert_eq!(store.try_get_log_entries(1..=3).await.unwrap(), entries);
        let state = store.get_log_state().await.unwrap();
        assert_eq!(state.last_log_id, Some(entries[2].log_id));
    }

    #[tokio::test]
    async fn truncate_and_purge_are_atomic_and_persistent() {
        let directory = tempdir().unwrap();
        let identity = NodeIdentity {
            cluster_id: Uuid::from_u128(1),
            node_id: 1,
        };
        let mut store = RocksLogStore::open(directory.path(), identity).unwrap();
        let entries = vec![entry(1, 1), entry(1, 2), entry(2, 3)];
        store.append_entries_sync(entries).unwrap();

        store
            .truncate(LogId::new(CommittedLeaderId::new(2, 1), 3))
            .await
            .unwrap();
        store
            .purge(LogId::new(CommittedLeaderId::new(1, 1), 1))
            .await
            .unwrap();
        drop(store);

        let mut reopened = RocksLogStore::open(directory.path(), identity).unwrap();
        let state = reopened.get_log_state().await.unwrap();
        assert_eq!(state.last_purged_log_id.map(|id| id.index), Some(1));
        assert_eq!(state.last_log_id.map(|id| id.index), Some(2));
        assert_eq!(reopened.try_get_log_entries(0..).await.unwrap().len(), 1);
    }

    #[test]
    fn log_directory_rejects_a_different_node_identity() {
        let directory = tempdir().unwrap();
        RocksLogStore::open(
            directory.path(),
            NodeIdentity {
                cluster_id: Uuid::from_u128(1),
                node_id: 1,
            },
        )
        .unwrap();

        let error = match RocksLogStore::open(
            directory.path(),
            NodeIdentity {
                cluster_id: Uuid::from_u128(1),
                node_id: 2,
            },
        ) {
            Ok(_) => panic!("different node identity unexpectedly opened the log"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("belongs to cluster"));
    }
}
