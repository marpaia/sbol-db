//! Reconciled Postgres-to-RocksDB conversion for production SynBioHub data.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use sbol_db_core::{ApiToken, UserId};
use sbol_db_postgres::{PgConfigStore, PgPool, PgUserStore, TripleRepository};
use sbol_db_rocksdb::{
    AccelCountImport, AccelCountKind, AccelFacetImport, AccelMemberImport, AccelObjectImport,
    RocksdbBulkLoader, RocksdbConfigStore, RocksdbTokenStore, RocksdbUserStore, SketchBandImport,
    SketchImport,
};
use sbol_db_storage::{ClusterId, ConfigStore, FacetKind, MetaRecord, RankRow, UserStore};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::output::print_json;

#[derive(Debug)]
pub struct CopyInputs {
    pub source_url: String,
    pub destination: PathBuf,
    pub chunk_size: usize,
    pub omit_completed_job_history: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct SourceCounts {
    graphs: u64,
    triples: u64,
    accelerator_objects: u64,
    accelerator_members: u64,
    accelerator_facets: u64,
    users: u64,
    api_tokens: u64,
    config_entries: u64,
    pageranks: u64,
    clusters: u64,
    sketches: u64,
    sketch_bands: u64,
}

#[derive(Debug, Serialize)]
struct CopyReport {
    source_run_id: Uuid,
    source_bundle_sha256: String,
    destination: PathBuf,
    status: &'static str,
    counts: SourceCounts,
    historical_jobs_omitted: u64,
    resumable: bool,
}

struct ReadySource {
    run_id: Uuid,
    bundle: String,
    counts: SourceCounts,
    historical_jobs: u64,
}

pub async fn run(inputs: CopyInputs) -> Result<()> {
    if inputs.chunk_size == 0 {
        bail!("--chunk-size must be greater than zero");
    }
    if !matches!(
        inputs
            .source_url
            .split_once("://")
            .map(|(scheme, _)| scheme),
        Some("postgres" | "postgresql")
    ) {
        bail!("copy-postgres-to-rocksdb requires a Postgres --database-url source");
    }

    let pool = sbol_db_postgres::connect(&inputs.source_url)
        .await
        .context("connecting to the Postgres source")?;
    let source = inspect_source(&pool, inputs.omit_completed_job_history).await?;
    let destination_url = format!("rocksdb://{}", inputs.destination.display());
    let db = sbol_db_rocksdb::connect(&destination_url).with_context(|| {
        format!(
            "opening RocksDB destination {}",
            inputs.destination.display()
        )
    })?;
    let loader = RocksdbBulkLoader::new(db.clone());
    let source_identity = format!("{}:{}", source.run_id, source.bundle);
    loader
        .prepare(&source_identity)
        .await
        .context("preparing the RocksDB destination")?;

    copy_identity_and_config(&pool, &db).await?;
    copy_triples(&pool, &loader, inputs.chunk_size, source.counts.triples).await?;
    copy_accelerators(&pool, &loader, inputs.chunk_size).await?;
    copy_search_state(&pool, &loader, inputs.chunk_size).await?;

    let source_after = source_counts(&pool, source.run_id).await?;
    if source_after != source.counts {
        bail!(
            "Postgres source changed during the copy: before={:?}, after={:?}",
            source.counts,
            source_after
        );
    }
    reconcile_destination(&loader, &source.counts).await?;

    let report = CopyReport {
        source_run_id: source.run_id,
        source_bundle_sha256: source.bundle,
        destination: inputs.destination,
        status: "ready",
        counts: source.counts,
        historical_jobs_omitted: source.historical_jobs,
        resumable: true,
    };
    let report_json = serde_json::to_string_pretty(&report)?;
    loader.mark_complete(&report_json).await?;
    print_json(&report)
}

async fn inspect_source(pool: &PgPool, omit_completed_jobs: bool) -> Result<ReadySource> {
    let rows = sqlx::query(
        "SELECT id, source_bundle_sha256 FROM sbh_migration_run \
         WHERE status='ready' ORDER BY completed_at DESC NULLS LAST",
    )
    .fetch_all(pool)
    .await
    .context("reading the production migration ledger")?;
    if rows.len() != 1 {
        bail!(
            "source must contain exactly one ready production migration run (found {})",
            rows.len()
        );
    }
    let run_id: Uuid = rows[0].try_get("id")?;
    let bundle: String = rows[0].try_get("source_bundle_sha256")?;

    let incomplete_graphs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_graph WHERE run_id=$1 AND status<>'verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let incomplete_accel: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbh_migration_accelerator WHERE run_id=$1 AND status<>'verified'",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    let dirty_accel: i64 = sqlx::query_scalar("SELECT count(*) FROM accel_dirty")
        .fetch_one(pool)
        .await?;
    if incomplete_graphs != 0 || incomplete_accel != 0 || dirty_accel != 0 {
        bail!(
            "source is not fully reconciled (graphs={incomplete_graphs}, accelerators={incomplete_accel}, dirty={dirty_accel})"
        );
    }

    let active_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sbol_jobs WHERE status NOT IN ('succeeded','failed','cancelled')",
    )
    .fetch_one(pool)
    .await?;
    if active_jobs != 0 {
        bail!("source has {active_jobs} active jobs; quiesce workers before copying");
    }
    let historical_jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM sbol_jobs")
        .fetch_one(pool)
        .await?;
    if historical_jobs != 0 && !omit_completed_jobs {
        bail!(
            "source has {historical_jobs} terminal job-history rows; rerun with \
             --omit-completed-job-history to acknowledge that operational history is not canonical registry data"
        );
    }

    let unsupported: i64 = sqlx::query_scalar(
        "SELECT \
           (SELECT count(*) FROM sbol_graphs WHERE kind='sbol3') + \
           (SELECT count(*) FROM sbol_objects) + \
           (SELECT count(*) FROM sbol_sequences) + \
           (SELECT count(*) FROM sbol_ontologies)",
    )
    .fetch_one(pool)
    .await?;
    if unsupported != 0 {
        bail!(
            "source contains {unsupported} typed document/object/sequence/ontology rows; \
             this production converter refuses to omit them"
        );
    }

    Ok(ReadySource {
        run_id,
        bundle,
        counts: source_counts(pool, run_id).await?,
        historical_jobs: u64::try_from(historical_jobs)?,
    })
}

async fn source_counts(pool: &PgPool, run_id: Uuid) -> Result<SourceCounts> {
    let row = sqlx::query(
        "SELECT \
           (SELECT count(*) FROM sbh_migration_graph WHERE run_id=$1 AND status='verified') graphs, \
           (SELECT coalesce(sum(loaded_quads),0)::bigint FROM sbh_migration_graph WHERE run_id=$1 AND status='verified') triples, \
           (SELECT count(*) FROM accel_object) accelerator_objects, \
           (SELECT count(*) FROM accel_member) accelerator_members, \
           (SELECT count(*) FROM accel_facet) accelerator_facets, \
           (SELECT count(*) FROM sbh_user) users, \
           (SELECT count(*) FROM sbh_api_token) api_tokens, \
           (SELECT count(*) FROM sbh_app_config) config_entries, \
           (SELECT count(*) FROM object_pagerank) pageranks, \
           (SELECT count(*) FROM sbol_sequence_cluster) clusters, \
           (SELECT count(*) FROM sbol_sequence_sketch) sketches, \
           (SELECT count(*) FROM sbol_sequence_lsh_band) sketch_bands",
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;
    Ok(SourceCounts {
        graphs: row_u64(&row, "graphs")?,
        triples: row_u64(&row, "triples")?,
        accelerator_objects: row_u64(&row, "accelerator_objects")?,
        accelerator_members: row_u64(&row, "accelerator_members")?,
        accelerator_facets: row_u64(&row, "accelerator_facets")?,
        users: row_u64(&row, "users")?,
        api_tokens: row_u64(&row, "api_tokens")?,
        config_entries: row_u64(&row, "config_entries")?,
        pageranks: row_u64(&row, "pageranks")?,
        clusters: row_u64(&row, "clusters")?,
        sketches: row_u64(&row, "sketches")?,
        sketch_bands: row_u64(&row, "sketch_bands")?,
    })
}

async fn copy_identity_and_config(pool: &PgPool, db: &sbol_db_rocksdb::Db) -> Result<()> {
    let users = PgUserStore::new(pool.clone()).list_users().await?;
    RocksdbUserStore::new(db.clone())
        .import_exact(users)
        .await
        .context("copying exact user records")?;
    let config = PgConfigStore::new(pool.clone()).get_all().await?;
    RocksdbConfigStore::new(db.clone())
        .import_exact(config)
        .await
        .context("copying exact configuration records")?;

    let rows = sqlx::query("SELECT token_hash, user_id, created_at FROM sbh_api_token")
        .fetch_all(pool)
        .await?;
    let tokens = rows
        .into_iter()
        .map(|row| {
            Ok(ApiToken {
                token_hash: row.try_get("token_hash")?,
                user_id: UserId(row.try_get("user_id")?),
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    RocksdbTokenStore::new(db.clone())
        .import_exact(tokens)
        .await
        .context("copying exact API-token records")?;
    Ok(())
}

async fn copy_triples(
    pool: &PgPool,
    loader: &RocksdbBulkLoader,
    chunk_size: usize,
    expected: u64,
) -> Result<()> {
    let repository = TripleRepository::new(pool.clone());
    let mut last_id = loader
        .checkpoint("triples")
        .await?
        .map(|value| value.parse::<i64>())
        .transpose()
        .context("invalid triples checkpoint")?
        .unwrap_or(0);
    let mut copied = 0_u64;
    loop {
        let rows = repository
            .scan_all_page(last_id, i64::try_from(chunk_size)?)
            .await?;
        if rows.is_empty() {
            break;
        }
        last_id = rows.last().expect("non-empty page").0;
        let triples = rows.into_iter().map(|(_, triple)| triple).collect();
        copied += u64::try_from(loader.write_triples(triples, last_id.to_string()).await?)?;
        if copied % 1_000_000 < chunk_size as u64 {
            tracing::info!(copied, expected, "copied canonical triples into RocksDB");
        }
    }
    tracing::info!(last_id, "canonical triple copy complete");
    Ok(())
}

async fn copy_accelerators(
    pool: &PgPool,
    loader: &RocksdbBulkLoader,
    chunk_size: usize,
) -> Result<()> {
    let (mut graph, mut iri) = loader
        .checkpoint("accel_objects_v2")
        .await?
        .map(|value| serde_json::from_str::<(String, String)>(&value))
        .transpose()?
        .unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT graph_iri, iri, top_level, meta FROM accel_object \
             WHERE (graph_iri, iri) > ($1, $2) ORDER BY graph_iri, iri LIMIT $3",
        )
        .bind(&graph)
        .bind(&iri)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        graph = rows.last().expect("page").try_get("graph_iri")?;
        iri = rows.last().expect("page").try_get("iri")?;
        let mut page = Vec::with_capacity(rows.len());
        for row in rows {
            let meta: MetaRecord = serde_json::from_str(&row.try_get::<String, _>("meta")?)?;
            let top_level: bool = row.try_get("top_level")?;
            if meta.top_level != top_level {
                bail!(
                    "accelerator metadata/top-level mismatch for {}",
                    row.try_get::<String, _>("iri")?
                );
            }
            page.push(AccelObjectImport {
                graph: row.try_get("graph_iri")?,
                iri: row.try_get("iri")?,
                meta,
            });
        }
        loader
            .write_accel_objects(page, serde_json::to_string(&(graph.clone(), iri.clone()))?)
            .await?;
    }

    let (mut graph, mut collection, mut member) = loader
        .checkpoint("accel_members")
        .await?
        .map(|value| serde_json::from_str::<(String, String, String)>(&value))
        .transpose()?
        .unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT graph_iri, collection_iri, member_iri, sort_key, is_root FROM accel_member \
             WHERE (graph_iri, collection_iri, member_iri) > ($1, $2, $3) \
             ORDER BY graph_iri, collection_iri, member_iri LIMIT $4",
        )
        .bind(&graph)
        .bind(&collection)
        .bind(&member)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("page");
        graph = last.try_get("graph_iri")?;
        collection = last.try_get("collection_iri")?;
        member = last.try_get("member_iri")?;
        let page = rows
            .into_iter()
            .map(|row| {
                Ok(AccelMemberImport {
                    graph: row.try_get("graph_iri")?,
                    collection: row.try_get("collection_iri")?,
                    member: row.try_get("member_iri")?,
                    sort_key: row.try_get("sort_key")?,
                    is_root: row.try_get("is_root")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        loader
            .write_accel_members(
                page,
                serde_json::to_string(&(graph.clone(), collection.clone(), member.clone()))?,
            )
            .await?;
    }

    let rows = sqlx::query(
        "SELECT graph_iri, kind, value, subject_count FROM accel_facet ORDER BY graph_iri, kind, value",
    )
    .fetch_all(pool)
    .await?;
    let facets = rows
        .into_iter()
        .map(|row| {
            let kind = match row.try_get::<i16, _>("kind")? {
                1 => FacetKind::Types,
                2 => FacetKind::Roles,
                3 => FacetKind::Creators,
                other => return Err(sqlx::Error::Protocol(format!("invalid facet kind {other}"))),
            };
            Ok(AccelFacetImport {
                graph: row.try_get("graph_iri")?,
                kind,
                value: row.try_get("value")?,
                subject_count: u64::try_from(row.try_get::<i64, _>("subject_count")?)
                    .map_err(|error| sqlx::Error::Protocol(error.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    loader.write_accel_facets(facets).await?;
    loader
        .write_accel_counts(load_accel_counts(pool).await?)
        .await?;
    tracing::info!("accelerator copy complete");
    Ok(())
}

async fn load_accel_counts(pool: &PgPool) -> Result<Vec<AccelCountImport>> {
    let mut out = Vec::new();
    for row in sqlx::query(
        "SELECT graph_iri, count(*)::bigint n FROM accel_object WHERE top_level GROUP BY graph_iri",
    )
    .fetch_all(pool)
    .await?
    {
        out.push(count_row(&row, AccelCountKind::TopLevel)?);
    }
    append_value_counts(
        pool,
        &mut out,
        "SELECT graph_iri, type_iri value, count(*)::bigint n FROM accel_type GROUP BY graph_iri, type_iri",
        AccelCountKind::Type,
    )
    .await?;
    append_value_counts(
        pool,
        &mut out,
        "SELECT t.graph_iri, t.type_iri value, count(*)::bigint n FROM accel_type t \
         JOIN accel_object o ON o.graph_iri=t.graph_iri AND o.iri=t.iri \
         WHERE o.top_level GROUP BY t.graph_iri, t.type_iri",
        AccelCountKind::TopLevelType,
    )
    .await?;
    append_value_counts(
        pool,
        &mut out,
        "SELECT t.graph_iri, t.type_iri value, count(*)::bigint n FROM accel_type t \
         WHERE NOT EXISTS (SELECT 1 FROM accel_member m WHERE m.graph_iri=t.graph_iri AND m.member_iri=t.iri) \
         GROUP BY t.graph_iri, t.type_iri",
        AccelCountKind::RootType,
    )
    .await?;
    append_value_counts(
        pool,
        &mut out,
        "SELECT t.graph_iri, t.type_iri value, count(*)::bigint n FROM accel_type t \
         JOIN accel_object o ON o.graph_iri=t.graph_iri AND o.iri=t.iri \
         WHERE o.top_level AND NOT EXISTS \
           (SELECT 1 FROM accel_member m WHERE m.graph_iri=t.graph_iri AND m.member_iri=t.iri) \
         GROUP BY t.graph_iri, t.type_iri",
        AccelCountKind::RootTopLevelType,
    )
    .await?;
    append_value_counts(
        pool,
        &mut out,
        "SELECT graph_iri, value, subject_count n FROM accel_facet WHERE kind=2",
        AccelCountKind::Role,
    )
    .await?;
    for row in sqlx::query(
        "SELECT r.graph_iri, t.type_iri, r.role_iri, count(*)::bigint n FROM accel_role r \
         JOIN accel_type t ON t.graph_iri=r.graph_iri AND t.iri=r.iri \
         GROUP BY r.graph_iri, t.type_iri, r.role_iri",
    )
    .fetch_all(pool)
    .await?
    {
        out.push(AccelCountImport {
            graph: row.try_get("graph_iri")?,
            kind: AccelCountKind::TopLevelTypeRole {
                object_type: row.try_get("type_iri")?,
                role: row.try_get("role_iri")?,
            },
            count: row_u64(&row, "n")?,
        });
    }
    for row in sqlx::query(
        "SELECT graph_iri, collection_iri, count(*)::bigint n, \
                count(*) FILTER (WHERE is_root)::bigint root_n \
         FROM accel_member GROUP BY graph_iri, collection_iri",
    )
    .fetch_all(pool)
    .await?
    {
        let graph: String = row.try_get("graph_iri")?;
        let collection: String = row.try_get("collection_iri")?;
        out.push(AccelCountImport {
            graph: graph.clone(),
            kind: AccelCountKind::Member {
                collection: collection.clone(),
                root_only: false,
            },
            count: row_u64(&row, "n")?,
        });
        out.push(AccelCountImport {
            graph,
            kind: AccelCountKind::Member {
                collection,
                root_only: true,
            },
            count: row_u64(&row, "root_n")?,
        });
    }
    Ok(out)
}

async fn append_value_counts(
    pool: &PgPool,
    out: &mut Vec<AccelCountImport>,
    query: &str,
    kind: fn(String) -> AccelCountKind,
) -> Result<()> {
    for row in sqlx::query(query).fetch_all(pool).await? {
        let value: String = row.try_get("value")?;
        out.push(count_row(&row, kind(value))?);
    }
    Ok(())
}

fn count_row(row: &sqlx::postgres::PgRow, kind: AccelCountKind) -> Result<AccelCountImport> {
    Ok(AccelCountImport {
        graph: row.try_get("graph_iri")?,
        kind,
        count: row_u64(row, "n")?,
    })
}

async fn copy_search_state(
    pool: &PgPool,
    loader: &RocksdbBulkLoader,
    chunk_size: usize,
) -> Result<()> {
    let mut iri = loader.checkpoint("pagerank").await?.unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT iri, score FROM object_pagerank WHERE iri>$1 ORDER BY iri LIMIT $2",
        )
        .bind(&iri)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        iri = rows.last().expect("page").try_get("iri")?;
        let page = rows
            .into_iter()
            .map(|row| {
                Ok(RankRow {
                    iri: row.try_get("iri")?,
                    score: row.try_get("score")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        loader.write_ranks(page, iri.clone()).await?;
    }

    let mut iri = loader.checkpoint("clusters").await?.unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT sequence_iri, cluster_id FROM sbol_sequence_cluster \
             WHERE sequence_iri>$1 ORDER BY sequence_iri LIMIT $2",
        )
        .bind(&iri)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        iri = rows.last().expect("page").try_get("sequence_iri")?;
        let page = rows
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get("sequence_iri")?,
                    ClusterId(row.try_get("cluster_id")?),
                ))
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        loader.write_clusters(page, iri.clone()).await?;
    }

    let mut iri = loader.checkpoint("sketches").await?.unwrap_or_default();
    loop {
        let rows = sqlx::query(
            "SELECT sequence_iri, signature FROM sbol_sequence_sketch \
             WHERE sequence_iri>$1 ORDER BY sequence_iri LIMIT $2",
        )
        .bind(&iri)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        iri = rows.last().expect("page").try_get("sequence_iri")?;
        let page = rows
            .into_iter()
            .map(|row| {
                Ok(SketchImport {
                    iri: row.try_get("sequence_iri")?,
                    signature: row.try_get("signature")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        loader.write_sketches(page, iri.clone()).await?;
    }

    // PostgreSQL orders this bit-pattern column as a signed bigint. Starting
    // at i64::MIN is essential: the ordinary `(0, "")` default silently
    // omits roughly half of the LSH rows. v2 deliberately ignores the old
    // checkpoint so a partially populated destination repairs itself by
    // replaying the full range (RocksDB puts are idempotent).
    let (mut band, mut iri) = loader
        .checkpoint("sketch_bands_v2")
        .await?
        .map(|value| serde_json::from_str::<(i64, String)>(&value))
        .transpose()?
        .unwrap_or((i64::MIN, String::new()));
    loop {
        let rows = sqlx::query(
            "SELECT band_hash, sequence_iri FROM sbol_sequence_lsh_band \
             WHERE (band_hash, sequence_iri)>($1,$2) ORDER BY band_hash, sequence_iri LIMIT $3",
        )
        .bind(band)
        .bind(&iri)
        .bind(i64::try_from(chunk_size)?)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().expect("page");
        band = last.try_get("band_hash")?;
        iri = last.try_get("sequence_iri")?;
        let page = rows
            .into_iter()
            .map(|row| {
                Ok(SketchBandImport {
                    band_hash: row.try_get("band_hash")?,
                    iri: row.try_get("sequence_iri")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        loader
            .write_sketch_bands(page, serde_json::to_string(&(band, iri.clone()))?)
            .await?;
    }
    tracing::info!("derived search-state copy complete");
    Ok(())
}

async fn reconcile_destination(loader: &RocksdbBulkLoader, expected: &SourceCounts) -> Result<()> {
    let actual = SourceCounts {
        graphs: expected.graphs,
        triples: loader.count("gspo").await?,
        accelerator_objects: loader.count("acc_meta").await?,
        accelerator_members: loader.count("acc_member").await?,
        accelerator_facets: loader.count("acc_facet").await?,
        users: loader.count("users").await?,
        api_tokens: loader.count("api_tokens").await?,
        config_entries: loader.count("app_config").await?,
        pageranks: loader.count("object_pagerank").await?,
        clusters: loader.count("sequence_cluster").await?,
        sketches: loader.count("seq_sketch").await?,
        sketch_bands: loader.count("seq_lsh_band").await?,
    };
    if &actual != expected {
        bail!("RocksDB reconciliation failed: expected={expected:?}, actual={actual:?}");
    }
    tracing::info!(?actual, "RocksDB cardinality reconciliation passed");
    Ok(())
}

fn row_u64(row: &sqlx::postgres::PgRow, column: &str) -> Result<u64> {
    u64::try_from(row.try_get::<i64, _>(column)?).context("negative database count")
}
