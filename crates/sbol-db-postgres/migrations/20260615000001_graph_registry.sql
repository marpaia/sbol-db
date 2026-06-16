-- Unify storage on the named graph.
--
-- Before this migration, triples were owned by a `sbol_documents` row (FK
-- CASCADE) and verbatim RDF writes (Graph Store / SPARQL Update) produced
-- triples with no owner at all. That left two disjoint write paths sharing one
-- table. This migration makes the **named graph** the first-class storage
-- owner: every triple belongs to a graph, and a `sbol_documents` row becomes an
-- ingest/provenance event that targeted a graph (one graph may receive many
-- ingests, e.g. a long-lived SynBioHub public graph). SBOL document import and
-- verbatim RDF writes then converge on one model: write triples into a graph.

CREATE TABLE sbol_graphs (
    iri        sbol_iri PRIMARY KEY,
    -- 'document': a graph dedicated to one imported SBOL document (1:1, the
    -- import owns its lifecycle). 'rdf': a standalone/shared graph mutated by
    -- Graph Store CRUD or SPARQL Update (the SynBioHub triplestore case).
    kind       text NOT NULL DEFAULT 'rdf' CHECK (kind IN ('document', 'rdf')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Register every named graph already present before wiring the ownership FK:
-- first from existing triples, then the conventional per-document graph for any
-- document (covers documents that imported zero triples).
INSERT INTO sbol_graphs (iri, kind)
SELECT DISTINCT
    graph_iri::text,
    CASE WHEN graph_iri::text LIKE 'graph:document:%' THEN 'document' ELSE 'rdf' END
FROM sbol_triples
WHERE graph_iri IS NOT NULL
ON CONFLICT (iri) DO NOTHING;

INSERT INTO sbol_graphs (iri, kind)
SELECT 'graph:document:' || id::text, 'document'
FROM sbol_documents
ON CONFLICT (iri) DO NOTHING;

-- A document records which graph it populated (provenance). Deleting the graph
-- removes its provenance rows.
ALTER TABLE sbol_documents
    ADD COLUMN graph_iri sbol_iri REFERENCES sbol_graphs(iri) ON DELETE CASCADE;

UPDATE sbol_documents
SET graph_iri = 'graph:document:' || id::text;

CREATE INDEX sbol_documents_graph_idx ON sbol_documents (graph_iri);

-- Triples: the graph is now the owner (CASCADE). `document_id` is demoted from
-- owner (CASCADE) to provenance (SET NULL) — it records which ingest produced
-- the triple, but the graph governs its lifecycle.
ALTER TABLE sbol_triples DROP CONSTRAINT IF EXISTS sbol_triples_document_id_fkey;
ALTER TABLE sbol_triples
    ADD CONSTRAINT sbol_triples_document_id_fkey
    FOREIGN KEY (document_id) REFERENCES sbol_documents(id) ON DELETE SET NULL;
ALTER TABLE sbol_triples
    ADD CONSTRAINT sbol_triples_graph_fkey
    FOREIGN KEY (graph_iri) REFERENCES sbol_graphs(iri) ON DELETE CASCADE;
