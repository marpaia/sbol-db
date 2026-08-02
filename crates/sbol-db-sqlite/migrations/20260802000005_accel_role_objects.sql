-- Exact role-to-object membership for native discovery. Keep a dedicated
-- relation rather than parsing every metadata JSON document at query time.
CREATE TABLE accel_role (
    graph_iri TEXT NOT NULL,
    role_iri  TEXT NOT NULL,
    iri       TEXT NOT NULL,
    sort_key  TEXT NOT NULL,
    PRIMARY KEY (graph_iri, role_iri, iri)
);

CREATE INDEX accel_role_scan_idx
    ON accel_role (graph_iri, role_iri, sort_key, iri);

INSERT OR IGNORE INTO accel_role (graph_iri, role_iri, iri, sort_key)
SELECT o.graph_iri, role.value, o.iri, o.sort_key
FROM accel_object o, json_each(o.meta, '$.roles') AS role;
