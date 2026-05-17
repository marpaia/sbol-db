-- Loaded once on first cluster init via /docker-entrypoint-initdb.d.
-- The library itself is preloaded via `command:` in docker-compose.yaml.
-- For existing volumes (where this script won't re-run), run manually:
--   docker compose exec postgres psql -U sbol -d sbol \
--     -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements;'
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
