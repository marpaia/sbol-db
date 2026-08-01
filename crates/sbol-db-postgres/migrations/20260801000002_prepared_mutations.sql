-- Durable, single-use mutation plans for CLI/MCP prepared changes.
CREATE TABLE sbol_prepared_mutation (
    token_hash            text PRIMARY KEY,
    user_id               uuid NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    oauth_client_id       text,
    audience              text,
    required_scopes       jsonb NOT NULL,
    operation             text NOT NULL,
    target_iri            text,
    expected_content_etag text,
    input_hash            text NOT NULL,
    effect                jsonb NOT NULL,
    payload               jsonb NOT NULL,
    created_at            timestamptz NOT NULL,
    expires_at            timestamptz NOT NULL
);

CREATE INDEX sbol_prepared_mutation_expires_idx
    ON sbol_prepared_mutation (expires_at);
