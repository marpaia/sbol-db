-- Durable instance configuration, a flat key/JSON-value store.
--
-- The persistent equivalent of classic SynBioHub's mutable config.local.json:
-- each section (registries, remotes, plugins, mail, theme, and the like) lives
-- under one stable key with an arbitrary JSON value.

CREATE TABLE sbh_app_config (
    key        text        PRIMARY KEY,
    value      jsonb       NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
