-- Track when an account was last modified, alongside the existing created_at.
--
-- Existing rows have never been modified, so their updated_at seeds from
-- created_at; new rows get their timestamp from the application layer.

ALTER TABLE sbh_user ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();

UPDATE sbh_user SET updated_at = created_at;
