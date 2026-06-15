-- The plan deliberately deviates from PLAN.md §11 here: SBOL `Resource`s
-- can be blank nodes (`_:b0`), which the `sbol_iri` domain rejects. We split
-- the subject and object positions into IRI vs blank-node columns with a
-- CHECK guaranteeing exactly one is populated.

CREATE TABLE sbol_triples (
    id             bigserial PRIMARY KEY,

    graph_iri      sbol_iri,

    subject_iri    sbol_iri,
    subject_blank  text,

    predicate_iri  sbol_iri NOT NULL,

    object_iri     sbol_iri,
    object_blank   text,
    object_literal text,
    object_json    jsonb,

    datatype_iri   sbol_iri,
    language       text,

    document_id    uuid REFERENCES sbol_documents(id) ON DELETE CASCADE,
    source         text NOT NULL DEFAULT 'sbol',
    created_at     timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT sbol_triples_subject_one CHECK (
        num_nonnulls(subject_iri, subject_blank) = 1
    ),
    CONSTRAINT sbol_triples_object_one CHECK (
        num_nonnulls(object_iri, object_blank, object_literal, object_json) = 1
    )
);

CREATE INDEX sbol_triples_spog_idx
    ON sbol_triples (subject_iri, predicate_iri, object_iri, graph_iri);

CREATE INDEX sbol_triples_posg_idx
    ON sbol_triples (predicate_iri, object_iri, subject_iri, graph_iri)
    WHERE object_iri IS NOT NULL;

CREATE INDEX sbol_triples_ospg_idx
    ON sbol_triples (object_iri, subject_iri, predicate_iri, graph_iri)
    WHERE object_iri IS NOT NULL;

CREATE INDEX sbol_triples_gspo_idx
    ON sbol_triples (graph_iri, subject_iri, predicate_iri, object_iri);

CREATE INDEX sbol_triples_document_idx
    ON sbol_triples (document_id);

CREATE INDEX sbol_triples_object_literal_trgm_idx
    ON sbol_triples USING gin (object_literal gin_trgm_ops);

CREATE INDEX sbol_triples_object_json_gin
    ON sbol_triples USING gin (object_json jsonb_path_ops);
