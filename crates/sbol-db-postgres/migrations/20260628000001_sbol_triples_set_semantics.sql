-- An RDF graph is a set of triples: the same triple asserted twice in a graph
-- is one triple. `sbol_triples` enforces this with a per-row identity over all
-- RDF positions (graph, subject, predicate, object, datatype, language) so that
-- repeated writes of an already-present triple are no-ops.
--
-- The identity is an md5 over a delimited encoding of the positions rather than
-- a unique index on the columns directly: `object_literal` holds SBOL sequence
-- literals that can exceed the btree index row limit, and several positions are
-- nullable (default graph, IRI vs blank node, typed vs language literal). The
-- md5 sidesteps both. The unit-separator delimiter (chr(31)) cannot occur in an
-- IRI and is not produced by the serializers, so distinct triples cannot
-- collide by concatenation.

ALTER TABLE sbol_triples
    ADD COLUMN triple_key text GENERATED ALWAYS AS (
        md5(
            coalesce(graph_iri::text, '')   || chr(31) ||
            coalesce(subject_iri::text, '') || chr(31) ||
            coalesce(subject_blank, '')     || chr(31) ||
            predicate_iri::text             || chr(31) ||
            coalesce(object_iri::text, '')  || chr(31) ||
            coalesce(object_blank, '')      || chr(31) ||
            coalesce(object_literal, '')    || chr(31) ||
            coalesce(object_json::text, '') || chr(31) ||
            coalesce(datatype_iri::text, '') || chr(31) ||
            coalesce(language, '')
        )
    ) STORED;

-- Collapse triples that are already duplicated, keeping the earliest row so
-- `document_id` provenance points at the first ingest that asserted the triple.
DELETE FROM sbol_triples a
USING sbol_triples b
WHERE a.triple_key = b.triple_key
  AND a.id > b.id;

CREATE UNIQUE INDEX sbol_triples_identity_idx ON sbol_triples (triple_key);
