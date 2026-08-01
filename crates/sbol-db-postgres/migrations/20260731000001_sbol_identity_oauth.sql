-- SBOL Identity OAuth 2.1 public clients and opaque grant credentials.
-- Secret material is hashed before persistence; authorization codes and
-- refresh tokens are consumed atomically by DELETE ... RETURNING.

CREATE TABLE sbol_oauth_client (
    client_id      text        PRIMARY KEY,
    client_name    text        NOT NULL,
    redirect_uris jsonb       NOT NULL,
    created_at     timestamptz NOT NULL
);

CREATE TABLE sbol_oauth_authorization_code (
    code_hash      text        PRIMARY KEY,
    user_id        uuid        NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    client_id      text        NOT NULL REFERENCES sbol_oauth_client(client_id) ON DELETE CASCADE,
    redirect_uri   text        NOT NULL,
    resource       text        NOT NULL,
    scopes         jsonb       NOT NULL,
    code_challenge text        NOT NULL,
    nonce          text,
    expires_at     timestamptz NOT NULL,
    created_at     timestamptz NOT NULL
);

CREATE TABLE sbol_oauth_access_token (
    token_hash text        PRIMARY KEY,
    user_id    uuid        NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    client_id  text        NOT NULL REFERENCES sbol_oauth_client(client_id) ON DELETE CASCADE,
    resource   text        NOT NULL,
    scopes     jsonb       NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE TABLE sbol_oauth_refresh_token (
    token_hash text        PRIMARY KEY,
    family_id  text        NOT NULL,
    user_id    uuid        NOT NULL REFERENCES sbh_user(id) ON DELETE CASCADE,
    client_id  text        NOT NULL REFERENCES sbol_oauth_client(client_id) ON DELETE CASCADE,
    resource   text        NOT NULL,
    scopes     jsonb       NOT NULL,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL
);

CREATE INDEX sbol_oauth_code_expiry_idx ON sbol_oauth_authorization_code (expires_at);
CREATE INDEX sbol_oauth_access_expiry_idx ON sbol_oauth_access_token (expires_at);
CREATE INDEX sbol_oauth_refresh_expiry_idx ON sbol_oauth_refresh_token (expires_at);
CREATE INDEX sbol_oauth_access_user_client_idx
    ON sbol_oauth_access_token (user_id, client_id);
CREATE INDEX sbol_oauth_refresh_user_client_idx
    ON sbol_oauth_refresh_token (user_id, client_id);
