-- Durable, resumable classic SynBioHub production migration state.
--
-- Classic permits more than one account to carry the same exact email. Keep
-- usernames unique, retain an ordinary email lookup index, and make login
-- ambiguity an application-level decision instead of silently discarding an
-- account or selecting an arbitrary row.
ALTER TABLE sbh_user DROP CONSTRAINT IF EXISTS sbh_user_email_key;
CREATE INDEX IF NOT EXISTS sbh_user_email_idx ON sbh_user (email);

CREATE TABLE sbh_migration_run (
    id                     uuid        PRIMARY KEY,
    source_bundle_sha256   text        NOT NULL,
    importer_version       text        NOT NULL,
    manifest               jsonb       NOT NULL,
    status                 text        NOT NULL CHECK (
        status IN ('preparing', 'loading', 'reconciling', 'ready', 'failed', 'abandoned')
    ),
    started_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now(),
    completed_at           timestamptz,
    UNIQUE (source_bundle_sha256, importer_version)
);

CREATE TABLE sbh_migration_artifact (
    run_id                 uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    kind                   text        NOT NULL,
    bytes                  bigint      NOT NULL CHECK (bytes >= 0),
    sha256                 text        NOT NULL,
    status                 text        NOT NULL CHECK (status IN ('pending', 'verified', 'failed')),
    error                  text,
    updated_at             timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, kind)
);

CREATE TABLE sbh_migration_graph (
    run_id                 uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    graph_iri              text        NOT NULL,
    graph_class            text        NOT NULL CHECK (graph_class IN ('public', 'user', 'other')),
    expected_quads         bigint      NOT NULL CHECK (expected_quads >= 0),
    expected_fingerprint   text        NOT NULL,
    loaded_quads           bigint      NOT NULL DEFAULT 0 CHECK (loaded_quads >= 0),
    status                 text        NOT NULL CHECK (
        status IN ('pending', 'loading', 'loaded', 'verified', 'failed', 'excluded')
    ),
    error                  text,
    updated_at             timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, graph_iri)
);

CREATE INDEX sbh_migration_graph_status_idx
    ON sbh_migration_graph (run_id, status, graph_iri);

-- Reconciliation walks one graph in stable keyset pages. The existing GSPO
-- index cannot satisfy `WHERE graph_iri = ? AND id > ? ORDER BY id` without a
-- sort, which is prohibitive for the production public graph.
CREATE INDEX IF NOT EXISTS sbol_triples_graph_id_idx
    ON sbol_triples (graph_iri, id);

CREATE TABLE sbh_migration_identity (
    run_id                 uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    source_user_id         bigint      NOT NULL,
    target_user_id         uuid        REFERENCES sbh_user(id) ON DELETE SET NULL,
    source_graph_uri       text        NOT NULL,
    status                 text        NOT NULL CHECK (status IN ('pending', 'loaded', 'verified', 'failed')),
    error                  text,
    updated_at             timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, source_user_id),
    UNIQUE (run_id, target_user_id)
);

CREATE TABLE sbh_migration_blob (
    run_id                 uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    sha1                   text        NOT NULL,
    compressed_bytes       bigint      NOT NULL CHECK (compressed_bytes >= 0),
    uncompressed_bytes     bigint      NOT NULL CHECK (uncompressed_bytes >= 0),
    compressed_sha256      text        NOT NULL,
    referenced             boolean     NOT NULL,
    status                 text        NOT NULL CHECK (
        status IN ('pending', 'copied', 'verified', 'missing', 'failed', 'orphaned')
    ),
    error                  text,
    updated_at             timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, sha1)
);

CREATE INDEX sbh_migration_blob_status_idx
    ON sbh_migration_blob (run_id, status, sha1);

CREATE TABLE sbh_migration_issue (
    id                     bigserial   PRIMARY KEY,
    run_id                 uuid        NOT NULL REFERENCES sbh_migration_run(id) ON DELETE CASCADE,
    severity               text        NOT NULL CHECK (severity IN ('blocker', 'warning')),
    scope                  text        NOT NULL CHECK (scope IN ('source', 'target', 'policy')),
    code                   text        NOT NULL,
    details                jsonb       NOT NULL DEFAULT '{}'::jsonb,
    waived_at              timestamptz,
    waiver_reason          text,
    created_at             timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbh_migration_issue_run_idx
    ON sbh_migration_issue (run_id, severity, code);
