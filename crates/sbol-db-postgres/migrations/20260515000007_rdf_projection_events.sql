CREATE TABLE sbol_rdf_projection_events (
    id           bigserial PRIMARY KEY,
    event_type   text NOT NULL,
    subject_iri  sbol_iri,
    graph_iri    sbol_iri,
    payload      jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    error        text
);

CREATE INDEX sbol_rdf_projection_events_unprocessed_idx
    ON sbol_rdf_projection_events (id)
    WHERE processed_at IS NULL;
