CREATE TABLE rdf_projection_events (
    id           bigserial PRIMARY KEY,
    event_type   text NOT NULL,
    subject_iri  iri,
    graph_iri    iri,
    payload      jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    processed_at timestamptz,
    error        text
);

CREATE INDEX rdf_projection_events_unprocessed_idx
    ON rdf_projection_events (id)
    WHERE processed_at IS NULL;
