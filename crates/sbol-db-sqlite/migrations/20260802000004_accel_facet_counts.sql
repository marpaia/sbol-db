-- Exact top-level subject counts for the native discovery facet sidebar.
ALTER TABLE accel_facet
    ADD COLUMN subject_count INTEGER NOT NULL DEFAULT 0;

UPDATE accel_facet AS f
SET subject_count = CASE f.kind
    WHEN 1 THEN (
        SELECT COUNT(DISTINCT ty.iri)
        FROM accel_type ty
        JOIN accel_object o
          ON o.graph_iri = ty.graph_iri AND o.iri = ty.iri
        WHERE ty.graph_iri = f.graph_iri
          AND ty.type_iri = f.value
          AND o.top_level
    )
    WHEN 2 THEN (
        SELECT COUNT(DISTINCT o.iri)
        FROM accel_object o, json_each(o.meta, '$.roles') AS role
        WHERE o.graph_iri = f.graph_iri
          AND o.top_level
          AND role.value = f.value
    )
    WHEN 3 THEN (
        SELECT COUNT(DISTINCT o.iri)
        FROM accel_object o, json_each(o.meta, '$.creators') AS creator
        WHERE o.graph_iri = f.graph_iri
          AND o.top_level
          AND creator.value = f.value
    )
    ELSE 0
END;
