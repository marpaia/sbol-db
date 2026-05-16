CREATE TABLE sbol_object_revisions (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    object_id       uuid NOT NULL REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri             sbol_iri NOT NULL,
    revision_number bigint NOT NULL,
    data            jsonb NOT NULL,
    triples_snapshot jsonb,
    content_hash    bytea NOT NULL,
    change_reason   text,
    created_by      text,
    created_at      timestamptz NOT NULL DEFAULT now(),

    UNIQUE (object_id, revision_number)
);

CREATE INDEX sbol_object_revisions_iri_idx
    ON sbol_object_revisions (iri);
