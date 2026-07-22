-- Track when an account was last modified, alongside the existing created_at.
--
-- SQLite requires a constant default for a NOT NULL column added in place, so
-- the column seeds from a fixed epoch and is then backfilled from created_at:
-- existing rows have never been modified. New rows get their timestamp from the
-- application layer.

ALTER TABLE sbh_user ADD COLUMN updated_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00+00:00';

UPDATE sbh_user SET updated_at = created_at;
