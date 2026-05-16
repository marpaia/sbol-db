CREATE TABLE validation_runs (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    target_iri         iri NOT NULL,
    target_document_id uuid REFERENCES sbol_documents(id) ON DELETE SET NULL,
    validator_name     text NOT NULL,
    validator_version  text,
    ruleset            text NOT NULL,
    status             text NOT NULL
        CHECK (status IN ('passed', 'failed', 'warning', 'error')),
    started_at         timestamptz NOT NULL DEFAULT now(),
    finished_at        timestamptz,
    summary            jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX validation_runs_target_idx
    ON validation_runs (target_iri);

CREATE INDEX validation_runs_ruleset_idx
    ON validation_runs (ruleset);

CREATE TABLE validation_findings (
    id                uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    validation_run_id uuid NOT NULL REFERENCES validation_runs(id) ON DELETE CASCADE,
    severity          text NOT NULL
        CHECK (severity IN ('info', 'warning', 'error', 'fatal')),
    rule_id           text,
    message           text NOT NULL,
    subject_iri       iri,
    predicate_iri     iri,
    object_iri        iri,
    path              text,
    data              jsonb NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX validation_findings_run_idx
    ON validation_findings (validation_run_id);

CREATE INDEX validation_findings_subject_idx
    ON validation_findings (subject_iri);

CREATE INDEX validation_findings_severity_idx
    ON validation_findings (severity);
