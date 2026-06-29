-- SynBioHub query accelerator: per-graph derived indexes that answer the fixed
-- SynBioHub query templates with point lookups and range scans instead of
-- graph-pattern evaluation. The indexes are derived from a graph's triples (the
-- same derivation every backend shares), maintained on the verbatim write path:
-- a write marks the graph dirty in `accel_dirty`, and the next read that needs
-- the indexes rebuilds them in one pass.

-- Presence of a row means the graph's accelerator indexes are stale.
CREATE TABLE accel_dirty (
    graph_iri TEXT PRIMARY KEY
);

-- One row per object, with its projected metadata as a JSON `MetaRecord` (the
-- shared serialization), plus the displayId sort key and the top-level flag the
-- enumeration scans key on.
CREATE TABLE accel_object (
    graph_iri TEXT NOT NULL,
    iri       TEXT NOT NULL,
    sort_key  TEXT NOT NULL,
    top_level INTEGER NOT NULL,
    meta      TEXT NOT NULL,
    PRIMARY KEY (graph_iri, iri)
);
CREATE INDEX accel_object_toplevel_idx
    ON accel_object (graph_iri, sort_key, iri)
    WHERE top_level;

-- One row per (object, rdf:type), for the `ByType` enumeration and count.
CREATE TABLE accel_type (
    graph_iri TEXT NOT NULL,
    type_iri  TEXT NOT NULL,
    iri       TEXT NOT NULL,
    sort_key  TEXT NOT NULL,
    PRIMARY KEY (graph_iri, type_iri, iri)
);
CREATE INDEX accel_type_scan_idx
    ON accel_type (graph_iri, type_iri, sort_key, iri);

-- One row per collection membership. `is_root` is the precomputed
-- `FILTER NOT EXISTS` anti-join: a member is a root unless another member
-- references it directly or via a child.
CREATE TABLE accel_member (
    graph_iri      TEXT NOT NULL,
    collection_iri TEXT NOT NULL,
    member_iri     TEXT NOT NULL,
    sort_key       TEXT NOT NULL,
    is_root        INTEGER NOT NULL,
    PRIMARY KEY (graph_iri, collection_iri, member_iri)
);
CREATE INDEX accel_member_scan_idx
    ON accel_member (graph_iri, collection_iri, sort_key, member_iri);

-- Distinct facet values over a graph's top-level objects: kind 1 = rdf:type,
-- 2 = sbol2:role, 3 = dc:creator.
CREATE TABLE accel_facet (
    graph_iri TEXT NOT NULL,
    kind      INTEGER NOT NULL,
    value     TEXT NOT NULL,
    PRIMARY KEY (graph_iri, kind, value)
);
