-- Exact role-to-object membership for production-scale native discovery. The
-- facet table stores only aggregate counts; this relation supplies the actual
-- subject set without scanning or parsing every object's metadata JSON.
CREATE TABLE accel_role (
    graph_iri text NOT NULL,
    role_iri  text NOT NULL,
    iri       text NOT NULL,
    sort_key  text NOT NULL,
    PRIMARY KEY (graph_iri, role_iri, iri)
);

CREATE INDEX accel_role_scan_idx
    ON accel_role (
        graph_iri,
        role_iri,
        left(sort_key, 128) COLLATE "C",
        left(iri, 128) COLLATE "C"
    );

INSERT INTO accel_role (graph_iri, role_iri, iri, sort_key)
SELECT o.graph_iri, role.value, o.iri, o.sort_key
FROM accel_object o
CROSS JOIN LATERAL jsonb_array_elements_text(
    COALESCE((o.meta::jsonb)->'roles', '[]'::jsonb)
) AS role(value)
ON CONFLICT DO NOTHING;
