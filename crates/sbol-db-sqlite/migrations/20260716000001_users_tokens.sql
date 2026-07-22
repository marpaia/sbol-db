-- SynBioHub-compatible identity: accounts and their API tokens.
--
-- UUIDs are stored as TEXT and booleans as INTEGER (0/1), matching the rest of
-- the SQLite backend. Password hashing and token generation live in the
-- application layer; these tables store the already-computed hashes.

CREATE TABLE sbh_user (
    id                   TEXT PRIMARY KEY,
    username             TEXT NOT NULL UNIQUE,
    name                 TEXT NOT NULL,
    email                TEXT NOT NULL UNIQUE,
    affiliation          TEXT,
    password_hash        TEXT NOT NULL,
    graph_uri            TEXT NOT NULL,
    is_admin             INTEGER NOT NULL DEFAULT 0,
    is_curator           INTEGER NOT NULL DEFAULT 0,
    is_member            INTEGER NOT NULL DEFAULT 1,
    reset_password_link  TEXT,
    created_at           TEXT NOT NULL
);

CREATE TABLE sbh_api_token (
    token_hash  TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES sbh_user (id) ON DELETE CASCADE,
    created_at  TEXT NOT NULL
);

CREATE INDEX sbh_api_token_user ON sbh_api_token (user_id);
