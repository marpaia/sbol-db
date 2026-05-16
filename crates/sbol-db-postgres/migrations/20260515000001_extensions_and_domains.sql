-- Phase 1 baseline: required Postgres extensions and the iri/ontology_term
-- domains used throughout the schema.

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS btree_gin;
CREATE EXTENSION IF NOT EXISTS pg_trgm;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'iri') THEN
        CREATE DOMAIN iri AS text
        CHECK (
            VALUE ~ '^[a-zA-Z][a-zA-Z0-9+.-]*:.+'
        );
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'ontology_term') THEN
        CREATE DOMAIN ontology_term AS text
        CHECK (
            VALUE ~ '^[a-zA-Z][a-zA-Z0-9+.-]*:.+'
        );
    END IF;
END
$$;
