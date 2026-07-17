-- Durable instance configuration, a flat key/JSON-value store.
--
-- The persistent equivalent of classic SynBioHub's mutable config.local.json:
-- each section (registries, remotes, plugins, mail, theme, and the like) lives
-- under one stable key with an arbitrary JSON value. The value is stored as
-- TEXT holding serialized JSON and updated_at as TEXT, matching the rest of the
-- SQLite backend.

CREATE TABLE sbh_app_config (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
