-- SBOL Identity OAuth 2.1 public clients and opaque grant credentials.
-- JSON arrays and UTC timestamps use the SQLite backend's TEXT representation.

CREATE TABLE sbol_oauth_client (
    client_id      TEXT PRIMARY KEY,
    client_name    TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    created_at     TEXT NOT NULL
);

CREATE TABLE sbol_oauth_authorization_code (
    code_hash      TEXT PRIMARY KEY,
    user_id        TEXT NOT NULL REFERENCES sbh_user (id) ON DELETE CASCADE,
    client_id      TEXT NOT NULL REFERENCES sbol_oauth_client (client_id) ON DELETE CASCADE,
    redirect_uri   TEXT NOT NULL,
    resource       TEXT NOT NULL,
    scopes         TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    nonce          TEXT,
    expires_at     TEXT NOT NULL,
    created_at     TEXT NOT NULL
);

CREATE TABLE sbol_oauth_access_token (
    token_hash TEXT PRIMARY KEY,
    user_id    TEXT NOT NULL REFERENCES sbh_user (id) ON DELETE CASCADE,
    client_id  TEXT NOT NULL REFERENCES sbol_oauth_client (client_id) ON DELETE CASCADE,
    resource   TEXT NOT NULL,
    scopes     TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE sbol_oauth_refresh_token (
    token_hash TEXT PRIMARY KEY,
    family_id  TEXT NOT NULL,
    user_id    TEXT NOT NULL REFERENCES sbh_user (id) ON DELETE CASCADE,
    client_id  TEXT NOT NULL REFERENCES sbol_oauth_client (client_id) ON DELETE CASCADE,
    resource   TEXT NOT NULL,
    scopes     TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX sbol_oauth_code_expiry ON sbol_oauth_authorization_code (expires_at);
CREATE INDEX sbol_oauth_access_expiry ON sbol_oauth_access_token (expires_at);
CREATE INDEX sbol_oauth_refresh_expiry ON sbol_oauth_refresh_token (expires_at);
CREATE INDEX sbol_oauth_access_user_client
    ON sbol_oauth_access_token (user_id, client_id);
CREATE INDEX sbol_oauth_refresh_user_client
    ON sbol_oauth_refresh_token (user_id, client_id);
