//! The open RocksDB handle and its column-family layout.
//!
//! One database holds every keyspace as a separate column family: the term
//! dictionary, the nine permuted triple indexes, the named-graph set, and a
//! column family per derived view (objects, graphs, ontology, sequences, jobs).
//! All families share tuned options (Snappy compression and a bloom filter for
//! fast point lookups, which the get-before-put insert path leans on).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, RwLock};

use rocksdb::{
    checkpoint::Checkpoint, BlockBasedOptions, BoundColumnFamily, Cache, DBCompressionType,
    DBWithThreadMode, MultiThreaded, Options, WriteBatch, DEFAULT_COLUMN_FAMILY_NAME,
};
use sbol_db_core::DomainError;

use crate::codec::{Term, TermId};

/// The multi-threaded RocksDB handle. Multi-threaded mode returns column-family
/// handles as `Arc<BoundColumnFamily>`, which clone cleanly into the blocking
/// tasks the async store spawns.
type Inner = DBWithThreadMode<MultiThreaded>;

/// Every column family the store opens. Adding one here is enough for it to be
/// created on open.
pub const COLUMN_FAMILIES: &[&str] = &[
    // Term dictionary: id -> reversible term encoding.
    "id2term",
    // Permuted triple indexes (default graph, then named graphs).
    "dspo",
    "dpos",
    "dosp",
    "spog",
    "posg",
    "ospg",
    "gspo",
    "gpos",
    "gosp",
    // Document-graph registry.
    "graph_meta",
    "graph_hash",
    // Named-graph catalog copied from a reconciled verbatim SynBioHub corpus.
    // The canonical triples remain in the ordinary gspo/gpos/gosp families.
    "verbatim_graph_meta",
    // Universal named-graph catalog. The two legacy graph metadata families
    // above remain readable during migration but product/Admin reads use this.
    "catalog_graph",        // graph id -> NamedGraphRecord JSON
    "catalog_graph_by_iri", // graph IRI -> graph id
    // Derived object view.
    "objects",
    "obj_by_id",
    "obj_by_graph",
    // Ontology.
    "ont",
    "ont_term",
    "ont_term_idx",
    "ont_alias",
    "ont_alias_idx",
    "ont_closure",
    "ont_closure_idx",
    // Sequences + k-mer seed index.
    "seq",
    "seq_kmer",
    "seq_kmer_by_iri",
    // MinHash/LSH similarity sketch index. `seq_sketch` maps iri -> signature
    // bytes; `seq_lsh_band` keys band_hash(8, BE) ++ iri -> () so a band's
    // members are one prefix scan; `seq_lsh_band_by_iri` mirrors
    // iri ++ SEP ++ band_hash(8, BE) -> () so a re-index drops a sequence's old
    // bands.
    "seq_sketch",
    "seq_lsh_band",
    "seq_lsh_band_by_iri",
    // Job queue.
    "job",
    "job_idem",
    "job_attempt",
    "job_log",
    "job_ready",
    // Identity: accounts, their username/email lookup indexes, and API tokens.
    "users",             // user id -> User JSON
    "users_by_username", // username -> user id
    "users_by_email",    // email -> user id
    "api_tokens",        // token hash -> user id
    // SynBioHub query accelerator: derived per-graph indexes (rebuilt lazily
    // when a graph is marked dirty). See `repo::accel`.
    "acc_meta",        // graph + SEP + iri -> MetaRecord JSON
    "acc_meta_by_iri", // iri + SEP + graph -> MetaRecord JSON (global resource pagination)
    // Global occurrence reference counts make universal corpus counts exact
    // without scanning the per-graph metadata families.
    "catalog_resource_refcount", // resource IRI -> u64 LE occurrence count
    "catalog_sequence_refcount", // sequence IRI -> u64 LE occurrence count
    "catalog_class_resource_refcount", // resource IRI + SEP + class IRI -> u64 LE
    "catalog_class_count",       // class IRI -> u64 LE distinct resource count
    "acc_toplevel", // graph + SEP + displayId + SEP + iri -> () (top-levels in sort order)
    "acc_bytype",   // graph + SEP + type + SEP + displayId + SEP + iri -> ()
    "acc_byrole",   // graph + SEP + role + SEP + displayId + SEP + iri -> ()
    "acc_member",   // graph + SEP + collection + SEP + displayId + SEP + iri -> ()
    "acc_rootmember", // graph + SEP + collection + SEP + displayId + SEP + iri -> () (anti-join)
    "acc_member_of", // graph + SEP + member iri -> () (reverse-membership existence)
    "acc_facet",    // graph + SEP + kind + SEP + value -> ()
    "acc_count",    // graph + SEP + scope -> u64 LE (precomputed counts)
    "acc_dirty",    // graph -> () (presence = indexes stale, rebuild on next read)
    // Object PageRank scores backing the native ranked search: iri -> f64 LE.
    "object_pagerank",
    // Sequence cluster assignments backing /similar. `sequence_cluster` maps
    // iri -> cluster_id (i64 BE); `sequence_cluster_by_id` keys
    // cluster_id(BE) + SEP + iri -> () so cluster-mate lookup is a prefix scan.
    "sequence_cluster",
    "sequence_cluster_by_id",
    // Durable instance configuration: key -> ConfigEntry JSON.
    "app_config",
    // Counters and schema version.
    "meta",
];

/// Field separator inside composite secondary-index keys. `0x1F` (unit
/// separator) cannot occur in an IRI or a CURIE, so concatenated key parts
/// never collide.
pub const SEP: u8 = 0x1F;

/// Presence means `acc_meta_by_iri` contains a complete reverse projection of
/// `acc_meta`, rather than only rows written since this column family was
/// introduced. Legacy databases acquire the marker after a background rebuild.
pub(crate) const ACC_META_BY_IRI_READY: &[u8] = b"derived:acc_meta_by_iri:v1:ready";

/// An open RocksDB database with the sbol-db column families. Cheaply cloneable
/// (shares one underlying handle); clones move into `spawn_blocking` closures.
#[derive(Clone)]
pub struct Db {
    inner: Arc<Inner>,
    terms: Arc<TermDict>,
    read_only: bool,
}

/// Shared id->term cache backing [`Db::resolve_term`]. A term id is a content
/// address (id = hash of the term) and `id2term` is append-only, so a cached
/// id->term mapping is immutable and never needs invalidation. Sharing it across
/// every pattern scan collapses the repeated `id2term` lookups a nested-loop
/// join makes (a join over N members revisits the same predicate, type, and
/// member terms N times). Its size tracks the number of distinct terms read, so
/// it stays within the term dictionary's footprint; a server over a very large
/// corpus could cap it with an LRU.
#[derive(Default)]
struct TermDict {
    map: RwLock<HashMap<TermId, Term>>,
}

fn cf_options(cache: &Cache) -> Options {
    let mut opts = Options::default();
    opts.set_compression_type(DBCompressionType::Snappy);
    let mut block = BlockBasedOptions::default();
    block.set_block_cache(cache);
    block.set_bloom_filter(10.0, false);
    block.set_cache_index_and_filter_blocks(true);
    opts.set_block_based_table_factory(&block);
    opts
}

impl Db {
    /// Open (creating if absent) the database at `path` with every column
    /// family present.
    pub fn open(path: &Path) -> Result<Self, DomainError> {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.increase_parallelism(parallelism);
        db_opts.set_max_background_jobs(parallelism.clamp(2, 8));
        db_opts.set_bytes_per_sync(1 << 20);

        let cache = Cache::new_lru_cache(256 << 20);
        let cfs = COLUMN_FAMILIES
            .iter()
            .map(|name| rocksdb::ColumnFamilyDescriptor::new(*name, cf_options(&cache)));

        let inner = Inner::open_cf_descriptors(&db_opts, path, cfs)
            .map_err(|e| DomainError::Database(format!("opening rocksdb at {path:?}: {e}")))?;
        Ok(Self {
            inner: Arc::new(inner),
            terms: Arc::new(TermDict::default()),
            read_only: false,
        })
    }

    /// Open an existing database without permitting any mutations. Backup
    /// verification uses this to prove a decrypted checkpoint is structurally
    /// complete without modifying the candidate restore generation.
    pub fn open_read_only(path: &Path) -> Result<Self, DomainError> {
        let actual = Inner::list_cf(&Options::default(), path).map_err(db_err)?;
        let actual: BTreeSet<_> = actual.into_iter().collect();
        let expected: BTreeSet<_> = std::iter::once(DEFAULT_COLUMN_FAMILY_NAME.to_owned())
            .chain(COLUMN_FAMILIES.iter().map(|name| (*name).to_owned()))
            .collect();
        if actual != expected {
            let missing: Vec<_> = expected.difference(&actual).cloned().collect();
            let unexpected: Vec<_> = actual.difference(&expected).cloned().collect();
            return Err(DomainError::Database(format!(
                "RocksDB checkpoint column-family mismatch; missing={missing:?}, unexpected={unexpected:?}"
            )));
        }
        let inner = Inner::open_cf_for_read_only(
            &Options::default(),
            path,
            COLUMN_FAMILIES.iter().copied(),
            false,
        )
        .map_err(db_err)?;
        Ok(Self {
            inner: Arc::new(inner),
            terms: Arc::new(TermDict::default()),
            read_only: true,
        })
    }

    /// Create a native RocksDB checkpoint containing every column family. The
    /// checkpoint API takes a consistent engine snapshot and hard-links
    /// immutable SST files when the destination shares a filesystem.
    pub fn create_checkpoint(&self, destination: &Path) -> Result<(), DomainError> {
        self.ensure_writable()?;
        if destination.exists() {
            return Err(DomainError::Io(format!(
                "checkpoint destination already exists: {}",
                destination.display()
            )));
        }
        self.inner.flush_wal(true).map_err(db_err)?;
        Checkpoint::new(self.inner.as_ref())
            .and_then(|checkpoint| checkpoint.create_checkpoint(destination))
            .map_err(db_err)
    }

    fn ensure_writable(&self) -> Result<(), DomainError> {
        if self.read_only {
            Err(DomainError::Database(
                "attempted to mutate a read-only RocksDB checkpoint".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    /// Resolve a term id to its term, caching the decode in the shared
    /// dictionary. Repeated ids across scans (a shared predicate, the rdf:type
    /// objects, a collection IRI revisited per member) are decoded once and then
    /// served from memory instead of a fresh `id2term` get and decode each time.
    pub fn resolve_term(&self, id: &TermId) -> Result<Term, DomainError> {
        if let Some(term) = self.terms.map.read().unwrap().get(id) {
            return Ok(term.clone());
        }
        let bytes = self
            .get_cf("id2term", id)?
            .ok_or_else(|| DomainError::Database("dangling term id".into()))?;
        let term = Term::decode(&bytes)?;
        self.terms.map.write().unwrap().insert(*id, term.clone());
        Ok(term)
    }

    /// Handle for a column family by name. The family always exists (every name
    /// in [`COLUMN_FAMILIES`] is created on open), so a miss is a logic error.
    pub fn cf(&self, name: &str) -> Arc<BoundColumnFamily<'_>> {
        self.inner
            .cf_handle(name)
            .unwrap_or_else(|| panic!("column family `{name}` is not registered"))
    }

    pub fn get_cf(&self, cf: &str, key: &[u8]) -> Result<Option<Vec<u8>>, DomainError> {
        self.inner.get_cf(&self.cf(cf), key).map_err(db_err)
    }

    pub fn put_cf(&self, cf: &str, key: &[u8], value: &[u8]) -> Result<(), DomainError> {
        self.ensure_writable()?;
        self.inner.put_cf(&self.cf(cf), key, value).map_err(db_err)
    }

    pub fn delete_cf(&self, cf: &str, key: &[u8]) -> Result<(), DomainError> {
        self.ensure_writable()?;
        self.inner.delete_cf(&self.cf(cf), key).map_err(db_err)
    }

    pub fn exists_cf(&self, cf: &str, key: &[u8]) -> Result<bool, DomainError> {
        // A definite "no" from the bloom filter avoids the read entirely; only
        // a "maybe" pays for the get.
        if !self.inner.key_may_exist_cf(&self.cf(cf), key) {
            return Ok(false);
        }
        Ok(self.get_cf(cf, key)?.is_some())
    }

    /// Atomically commit a batch of writes across column families.
    pub fn write(&self, batch: WriteBatch) -> Result<(), DomainError> {
        self.ensure_writable()?;
        self.inner.write(batch).map_err(db_err)
    }

    /// Iterate every (key, value) under `prefix` in one column family, in key
    /// order, invoking `f` for each. Stops early when `f` returns `false`.
    pub fn for_each_prefix(
        &self,
        cf: &str,
        prefix: &[u8],
        mut f: impl FnMut(&[u8], &[u8]) -> Result<bool, DomainError>,
    ) -> Result<(), DomainError> {
        let handle = self.cf(cf);
        let mode = rocksdb::IteratorMode::From(prefix, rocksdb::Direction::Forward);
        for item in self.inner.iterator_cf(&handle, mode) {
            let (key, value) = item.map_err(db_err)?;
            if !key.starts_with(prefix) {
                break;
            }
            if !f(&key, &value)? {
                break;
            }
        }
        Ok(())
    }

    /// Iterate a prefix range strictly after an opaque prior key. The cursor
    /// is the exact last key returned by a previous page, so this remains
    /// stable without OFFSET even while the database contains many graphs.
    pub fn for_each_prefix_after(
        &self,
        cf: &str,
        prefix: &[u8],
        after: Option<&[u8]>,
        mut f: impl FnMut(&[u8], &[u8]) -> Result<bool, DomainError>,
    ) -> Result<(), DomainError> {
        let handle = self.cf(cf);
        let start = after.unwrap_or(prefix);
        let mode = rocksdb::IteratorMode::From(start, rocksdb::Direction::Forward);
        for item in self.inner.iterator_cf(&handle, mode) {
            let (key, value) = item.map_err(db_err)?;
            if !key.starts_with(prefix) {
                break;
            }
            if after.is_some_and(|cursor| key.as_ref() == cursor) {
                continue;
            }
            if !f(&key, &value)? {
                break;
            }
        }
        Ok(())
    }

    /// Read an integer-valued RocksDB property (e.g. `rocksdb.estimate-num-keys`)
    /// for one column family. Returns `None` when the property is unset or the
    /// engine cannot compute it.
    pub fn property_int_cf(&self, cf: &str, name: &str) -> Option<u64> {
        self.inner
            .property_int_value_cf(&self.cf(cf), name)
            .ok()
            .flatten()
    }

    /// Every live SST file across all column families, each tagged with its
    /// column family, level, and size. This is the per-level, per-family source
    /// the LSM stats surface aggregates.
    pub fn live_files(&self) -> Result<Vec<rocksdb::LiveFile>, DomainError> {
        self.inner.live_files().map_err(db_err)
    }

    /// Trigger a full manual compaction of every column family. Synchronous and
    /// potentially slow; callers run it on a blocking thread.
    pub fn compact_all(&self) {
        if self.read_only {
            return;
        }
        for cf in COLUMN_FAMILIES {
            self.inner
                .compact_range_cf(&self.cf(cf), None::<&[u8]>, None::<&[u8]>);
        }
    }

    /// Iterate every (key, value) in one column family in key order.
    pub fn for_each(
        &self,
        cf: &str,
        mut f: impl FnMut(&[u8], &[u8]) -> Result<bool, DomainError>,
    ) -> Result<(), DomainError> {
        let handle = self.cf(cf);
        for item in self
            .inner
            .iterator_cf(&handle, rocksdb::IteratorMode::Start)
        {
            let (key, value) = item.map_err(db_err)?;
            if !f(&key, &value)? {
                break;
            }
        }
        Ok(())
    }

    /// Visit each distinct fixed-width leading key component without walking
    /// every key in the component's range. The iterator seeks directly to the
    /// successor prefix, so enumerating named graphs from `gspo` is O(graphs)
    /// rather than O(triples).
    pub fn for_each_distinct_fixed_prefix(
        &self,
        cf: &str,
        prefix_len: usize,
        mut f: impl FnMut(&[u8]) -> Result<bool, DomainError>,
    ) -> Result<(), DomainError> {
        let handle = self.cf(cf);
        let mut iterator = self.inner.raw_iterator_cf(&handle);
        iterator.seek_to_first();
        while iterator.valid() {
            let key = iterator
                .key()
                .ok_or_else(|| DomainError::Database("RocksDB iterator lost its key".into()))?;
            if key.len() < prefix_len {
                return Err(DomainError::Database(format!(
                    "RocksDB key in `{cf}` is shorter than its {prefix_len}-byte prefix"
                )));
            }
            let prefix = &key[..prefix_len];
            if !f(prefix)? {
                break;
            }
            let Some(successor) = prefix_successor(prefix) else {
                break;
            };
            iterator.seek(successor);
        }
        iterator.status().map_err(db_err)
    }
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut next = prefix.to_vec();
    for byte in next.iter_mut().rev() {
        if *byte != u8::MAX {
            *byte += 1;
            return Some(next);
        }
        *byte = 0;
    }
    None
}

pub fn db_err(e: rocksdb::Error) -> DomainError {
    DomainError::Database(e.to_string())
}

/// Build a composite secondary-index key from parts joined by [`SEP`].
pub fn compose(parts: &[&[u8]]) -> Vec<u8> {
    let len = parts.iter().map(|p| p.len()).sum::<usize>() + parts.len().saturating_sub(1);
    let mut out = Vec::with_capacity(len);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(SEP);
        }
        out.extend_from_slice(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_reopens_read_only_with_all_state() {
        let source = tempfile::tempdir().unwrap();
        let checkpoint_parent = tempfile::tempdir().unwrap();
        let checkpoint_path = checkpoint_parent.path().join("checkpoint");
        let db = Db::open(source.path()).unwrap();
        db.put_cf("app_config", b"instance", b"complete-state")
            .unwrap();

        db.create_checkpoint(&checkpoint_path).unwrap();
        let checkpoint = Db::open_read_only(&checkpoint_path).unwrap();

        assert_eq!(
            checkpoint.get_cf("app_config", b"instance").unwrap(),
            Some(b"complete-state".to_vec())
        );
        assert!(checkpoint
            .put_cf("app_config", b"instance", b"mutated")
            .is_err());
    }
}
