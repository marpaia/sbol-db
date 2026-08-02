-- Exact top-level subject counts for the native discovery facet sidebar.
ALTER TABLE accel_facet
    ADD COLUMN subject_count bigint NOT NULL DEFAULT 0;

WITH counts AS (
    SELECT ty.graph_iri, 1::smallint AS kind, ty.type_iri AS value,
           COUNT(DISTINCT ty.iri)::bigint AS subject_count
    FROM accel_type ty
    JOIN accel_object o
      ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri
    WHERE o.top_level
    GROUP BY ty.graph_iri, ty.type_iri
    UNION ALL
    SELECT o.graph_iri, 2::smallint, role.value,
           COUNT(DISTINCT o.iri)::bigint
    FROM accel_object o
    CROSS JOIN LATERAL jsonb_array_elements_text(
        COALESCE((o.meta::jsonb)->'roles', '[]'::jsonb)
    ) AS role(value)
    WHERE o.top_level
    GROUP BY o.graph_iri, role.value
    UNION ALL
    SELECT o.graph_iri, 3::smallint, creator.value,
           COUNT(DISTINCT o.iri)::bigint
    FROM accel_object o
    CROSS JOIN LATERAL jsonb_array_elements_text(
        COALESCE((o.meta::jsonb)->'creators', '[]'::jsonb)
    ) AS creator(value)
    WHERE o.top_level
    GROUP BY o.graph_iri, creator.value
)
UPDATE accel_facet f
SET subject_count = counts.subject_count
FROM counts
WHERE f.graph_iri = counts.graph_iri
  AND f.kind = counts.kind
  AND f.value = counts.value;
