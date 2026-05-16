CREATE TABLE namespaces (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    prefix        text NOT NULL UNIQUE,
    namespace_iri iri NOT NULL UNIQUE,
    description   text,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sbol_documents (
    id                   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    document_iri         iri UNIQUE,
    name                 text,
    description          text,
    serialization_format text NOT NULL DEFAULT 'json'
        CHECK (serialization_format IN ('json', 'jsonld', 'rdfxml', 'turtle', 'trig', 'ntriples', 'nquads')),
    source_uri           text,
    raw_payload          jsonb,
    content_hash         bytea,
    created_by           text,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_documents_raw_payload_gin
    ON sbol_documents USING gin (raw_payload);
