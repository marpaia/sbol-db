-- Fold `sbol_documents` into `sbol_graphs`: the graph is the single container.
--
-- After the graph-registry migration, a triple pointed at both a graph (owner)
-- and a document (provenance), and a document pointed at a graph — a redundant
-- triangle, with `raw_payload` duplicating the triples. This migration collapses
-- it: `sbol_graphs` absorbs the per-import metadata and a normalization policy,
-- `sbol_documents` is dropped, and the derived-view / validation foreign keys
-- re-point to the graph. A "document" becomes simply a `kind = 'sbol3'` graph.
--
-- Trick: we seed `sbol_graphs.id` with the *same* uuid the document had, so the
-- existing `document_id` / `target_document_id` columns stay valid and we only
-- re-point their foreign keys rather than re-map data.

-- 1. Absorb the document's surrogate id, identity, and import metadata.
ALTER TABLE sbol_graphs ADD COLUMN id uuid;
ALTER TABLE sbol_graphs ADD COLUMN document_iri sbol_iri;
ALTER TABLE sbol_graphs ADD COLUMN name text;
ALTER TABLE sbol_graphs ADD COLUMN description text;
ALTER TABLE sbol_graphs ADD COLUMN serialization_format text;
ALTER TABLE sbol_graphs ADD COLUMN source_uri text;
ALTER TABLE sbol_graphs ADD COLUMN content_hash bytea;
ALTER TABLE sbol_graphs ADD COLUMN created_by text;

UPDATE sbol_graphs g
SET id = d.id,
    document_iri = d.document_iri,
    name = d.name,
    description = d.description,
    serialization_format = d.serialization_format,
    source_uri = d.source_uri,
    content_hash = d.content_hash,
    created_by = d.created_by
FROM sbol_documents d
WHERE d.graph_iri = g.iri;

-- Graphs that never had a document (verbatim / Graph Store writes) get a fresh id.
UPDATE sbol_graphs SET id = gen_random_uuid() WHERE id IS NULL;

ALTER TABLE sbol_graphs ALTER COLUMN id SET NOT NULL;
ALTER TABLE sbol_graphs ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE sbol_graphs ADD CONSTRAINT sbol_graphs_id_key UNIQUE (id);
ALTER TABLE sbol_graphs ADD CONSTRAINT sbol_graphs_document_iri_key UNIQUE (document_iri);

-- 2. The `kind` column becomes the normalization policy:
--    'sbol3'    = a single imported document, normalized/upgraded to SBOL3,
--    'verbatim' = a standalone/shared graph stored exactly as written.
-- Drop the old policy CHECK before remapping values, or the remap trips it.
ALTER TABLE sbol_graphs DROP CONSTRAINT IF EXISTS sbol_graphs_kind_check;
UPDATE sbol_graphs
SET kind = CASE kind WHEN 'document' THEN 'sbol3' WHEN 'rdf' THEN 'verbatim' ELSE kind END;
ALTER TABLE sbol_graphs ADD CONSTRAINT sbol_graphs_kind_check CHECK (kind IN ('sbol3', 'verbatim'));
ALTER TABLE sbol_graphs ALTER COLUMN kind SET DEFAULT 'verbatim';

-- 3. Re-point the derived-view and validation foreign keys from the (going
--    away) document table to the graph. The column values are unchanged
--    because `sbol_graphs.id` was seeded from `sbol_documents.id`.
ALTER TABLE sbol_objects DROP CONSTRAINT IF EXISTS sbol_objects_document_id_fkey;
ALTER TABLE sbol_objects
    ADD CONSTRAINT sbol_objects_document_id_fkey
    FOREIGN KEY (document_id) REFERENCES sbol_graphs(id) ON DELETE SET NULL;

ALTER TABLE sbol_validation_runs DROP CONSTRAINT IF EXISTS sbol_validation_runs_target_document_id_fkey;
ALTER TABLE sbol_validation_runs
    ADD CONSTRAINT sbol_validation_runs_target_document_id_fkey
    FOREIGN KEY (target_document_id) REFERENCES sbol_graphs(id) ON DELETE SET NULL;

-- 4. Triples belong to their graph alone — drop the redundant provenance edge.
ALTER TABLE sbol_triples DROP CONSTRAINT IF EXISTS sbol_triples_document_id_fkey;
DROP INDEX IF EXISTS sbol_triples_document_idx;
ALTER TABLE sbol_triples DROP COLUMN document_id;

-- 5. Nothing references the document table now; drop it (and `raw_payload`).
DROP TABLE sbol_documents;
