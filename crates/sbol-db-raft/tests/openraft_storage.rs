use openraft::testing::{StoreBuilder, Suite};
use openraft::{StorageError, StorageIOError};
use sbol_db_raft::{NodeId, NodeIdentity, RocksLogStore, RocksStateMachine, TypeConfig};
use tempfile::{tempdir, TempDir};

struct RocksStoreBuilder;

impl StoreBuilder<TypeConfig, RocksLogStore, RocksStateMachine, TempDir> for RocksStoreBuilder {
    async fn build(
        &self,
    ) -> Result<(TempDir, RocksLogStore, RocksStateMachine), StorageError<NodeId>> {
        let root = tempdir().map_err(|error| StorageIOError::write(&error))?;
        let cluster_id = uuid::Uuid::from_u128(1);
        let log = RocksLogStore::open(
            root.path().join("raft-log"),
            NodeIdentity {
                cluster_id,
                node_id: 1,
            },
        )
        .map_err(|error| StorageIOError::write_logs(&error))?;
        let state = RocksStateMachine::open(
            root.path().join("state"),
            root.path().join("snapshots"),
            cluster_id,
        )?;
        Ok((root, log, state))
    }
}

#[test]
fn openraft_storage_conformance_suite() {
    Suite::<TypeConfig, RocksLogStore, RocksStateMachine, RocksStoreBuilder, TempDir>::test_all(
        RocksStoreBuilder,
    )
    .unwrap();
}
