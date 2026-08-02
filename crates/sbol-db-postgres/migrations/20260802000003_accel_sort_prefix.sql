-- A production displayId can be much larger than PostgreSQL's maximum B-tree
-- index row. Keep the full sort_key in the table for exact ordering, but index
-- only a bounded prefix. Queries order by prefix and then the full value; that
-- is lexicographically equivalent to ordering by the full value alone.
DROP INDEX IF EXISTS accel_object_toplevel_idx;
CREATE INDEX accel_object_toplevel_idx
    ON accel_object (
        graph_iri,
        (left(sort_key, 128) COLLATE "C"),
        (left(iri, 128) COLLATE "C")
    )
    WHERE top_level;

DROP INDEX IF EXISTS accel_type_scan_idx;
CREATE INDEX accel_type_scan_idx
    ON accel_type (
        graph_iri,
        type_iri,
        (left(sort_key, 128) COLLATE "C"),
        (left(iri, 128) COLLATE "C")
    );

DROP INDEX IF EXISTS accel_member_scan_idx;
CREATE INDEX accel_member_scan_idx
    ON accel_member (
        graph_iri,
        collection_iri,
        (left(sort_key, 128) COLLATE "C"),
        (left(member_iri, 128) COLLATE "C")
    );
