-- SQLite schema for the graph-owned triplestore and its derived object view.
-- Mirrors the Postgres model with portable types: UUIDs and timestamps are
-- TEXT, ontology-term arrays are JSON TEXT, and a triple's set-identity is a
-- BLOB `triple_key` hashed in Rust (Postgres uses a generated md5 column;
-- SQLite has no equivalent, and a hash sidesteps the index-size limit on large
-- sequence literals).

CREATE TABLE sbol_graphs (
    iri                  TEXT PRIMARY KEY,
    id                   TEXT NOT NULL UNIQUE,
    kind                 TEXT NOT NULL,
    document_iri         TEXT,
    name                 TEXT,
    description          TEXT,
    serialization_format TEXT,
    source_uri           TEXT,
    content_hash         BLOB,
    created_by           TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

CREATE INDEX sbol_graphs_kind_hash ON sbol_graphs (kind, content_hash);

-- A named graph owns its triples: deleting the graph row cascades them away.
-- Default-graph triples carry a NULL graph_iri (no owner, no cascade).
CREATE TABLE sbol_triples (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    graph_iri     TEXT REFERENCES sbol_graphs (iri) ON DELETE CASCADE,
    subject_iri   TEXT,
    subject_blank TEXT,
    predicate_iri TEXT NOT NULL,
    object_iri    TEXT,
    object_blank  TEXT,
    object_literal TEXT,
    datatype_iri  TEXT,
    language      TEXT,
    source        TEXT,
    triple_key    BLOB NOT NULL UNIQUE
);

CREATE INDEX sbol_triples_spog
    ON sbol_triples (subject_iri, predicate_iri, object_iri, graph_iri);
CREATE INDEX sbol_triples_posg
    ON sbol_triples (predicate_iri, object_iri, subject_iri, graph_iri);
CREATE INDEX sbol_triples_graph ON sbol_triples (graph_iri);

CREATE TABLE sbol_objects (
    id           TEXT PRIMARY KEY,
    iri          TEXT NOT NULL UNIQUE,
    sbol_class   TEXT NOT NULL,
    display_id   TEXT,
    name         TEXT,
    description  TEXT,
    graph_id     TEXT REFERENCES sbol_graphs (id) ON DELETE SET NULL,
    types        TEXT NOT NULL DEFAULT '[]',
    roles        TEXT NOT NULL DEFAULT '[]',
    data         TEXT NOT NULL DEFAULT '{}',
    content_hash BLOB,
    is_deleted   INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

CREATE INDEX sbol_objects_graph ON sbol_objects (graph_id);
CREATE INDEX sbol_objects_class ON sbol_objects (sbol_class);
