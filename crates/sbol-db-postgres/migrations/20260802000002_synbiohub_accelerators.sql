-- Production imports write the canonical triple table directly in bounded
-- batches, so they must also build the per-graph SynBioHub query accelerator
-- before the imported instance is considered fully usable.  Track that work
-- graph-by-graph so a large rebuild can resume after interruption.
ALTER TABLE sbh_migration_run
    ADD COLUMN accelerators_completed_at timestamptz;

CREATE TABLE sbh_migration_accelerator (
    run_id      uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    graph_iri   text        NOT NULL,
    status      text        NOT NULL CHECK (status IN ('pending', 'building', 'verified', 'failed')),
    error       text,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, graph_iri)
);

CREATE INDEX sbh_migration_accelerator_status_idx
    ON sbh_migration_accelerator (run_id, status, graph_iri);
