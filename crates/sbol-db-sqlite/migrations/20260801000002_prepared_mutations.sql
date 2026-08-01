-- Durable, single-use mutation plans for CLI/MCP prepared changes.
CREATE TABLE sbol_prepared_mutation (
    token_hash            TEXT PRIMARY KEY,
    user_id               TEXT NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    oauth_client_id       TEXT,
    audience              TEXT,
    required_scopes       TEXT NOT NULL,
    operation             TEXT NOT NULL,
    target_iri            TEXT,
    expected_content_etag TEXT,
    input_hash            TEXT NOT NULL,
    effect                TEXT NOT NULL,
    payload               TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    expires_at            TEXT NOT NULL
);

CREATE INDEX sbol_prepared_mutation_expires_idx
    ON sbol_prepared_mutation (expires_at);
