-- Whole-collection synchronization writes the same durable named graph as an
-- initial submission, but records the graph's operational role explicitly so
-- storage introspection can distinguish it from raw verbatim RDF imports.
ALTER TABLE sbol_graphs
    DROP CONSTRAINT IF EXISTS sbol_graphs_kind_check;

ALTER TABLE sbol_graphs
    ADD CONSTRAINT sbol_graphs_kind_check
    CHECK (kind IN ('sbol3', 'verbatim', 'collection-sync'));
