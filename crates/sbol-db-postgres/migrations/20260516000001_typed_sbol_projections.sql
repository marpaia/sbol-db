-- Phase 2: typed projections for the structural SBOL classes parsed by sbol-rs.
-- Each row joins back to sbol_objects via object_id and is uniquely keyed by IRI.

CREATE TABLE sbol_components (
    object_id        uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri              iri NOT NULL UNIQUE,
    types            ontology_term[] NOT NULL DEFAULT '{}',
    roles            ontology_term[] NOT NULL DEFAULT '{}',
    sequence_iris    iri[] NOT NULL DEFAULT '{}',
    feature_iris     iri[] NOT NULL DEFAULT '{}',
    interaction_iris iri[] NOT NULL DEFAULT '{}',
    model_iris       iri[] NOT NULL DEFAULT '{}',
    organism         text,
    chassis          text,
    design_domain    text,
    modality         text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_components_types_gin
    ON sbol_components USING gin (types);

CREATE INDEX sbol_components_roles_gin
    ON sbol_components USING gin (roles);

CREATE INDEX sbol_components_sequence_iris_gin
    ON sbol_components USING gin (sequence_iris);

CREATE INDEX sbol_components_feature_iris_gin
    ON sbol_components USING gin (feature_iris);

CREATE INDEX sbol_components_design_domain_idx
    ON sbol_components (design_domain);

CREATE INDEX sbol_components_modality_idx
    ON sbol_components (modality);

CREATE TABLE sbol_sequences (
    object_id    uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri          iri NOT NULL UNIQUE,
    encoding_iri iri,
    elements     text,
    length_bp    integer GENERATED ALWAYS AS (char_length(elements)) STORED,
    alphabet     text CHECK (alphabet IN ('DNA', 'RNA', 'PROTEIN', 'SMILES', 'OTHER')),
    topology     text CHECK (topology IN ('linear', 'circular', 'unknown')) DEFAULT 'unknown',
    content_hash bytea,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_sequences_hash_idx
    ON sbol_sequences (content_hash);

CREATE INDEX sbol_sequences_length_idx
    ON sbol_sequences (length_bp);

CREATE INDEX sbol_sequences_alphabet_idx
    ON sbol_sequences (alphabet);

CREATE TABLE sbol_features (
    object_id            uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri                  iri NOT NULL UNIQUE,
    parent_component_iri iri,
    feature_kind         text NOT NULL,
    instance_of_iri      iri,
    roles                ontology_term[] NOT NULL DEFAULT '{}',
    orientation_iri      iri,
    created_at           timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_features_parent_idx
    ON sbol_features (parent_component_iri);

CREATE INDEX sbol_features_instance_idx
    ON sbol_features (instance_of_iri);

CREATE INDEX sbol_features_roles_gin
    ON sbol_features USING gin (roles);

CREATE INDEX sbol_features_kind_idx
    ON sbol_features (feature_kind);

CREATE TABLE sbol_locations (
    object_id       uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri             iri NOT NULL UNIQUE,
    feature_iri     iri,
    sequence_iri    iri,
    location_kind   text NOT NULL,
    start_pos       integer,
    end_pos         integer,
    cut_pos         integer,
    orientation_iri iri,
    data            jsonb NOT NULL DEFAULT '{}'::jsonb,

    CHECK (
        location_kind <> 'Range'
        OR (start_pos IS NOT NULL AND end_pos IS NOT NULL AND start_pos <= end_pos)
    )
);

CREATE INDEX sbol_locations_feature_idx
    ON sbol_locations (feature_iri);

CREATE INDEX sbol_locations_sequence_range_idx
    ON sbol_locations (sequence_iri, start_pos, end_pos);

CREATE TABLE sbol_constraints (
    object_id            uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri                  iri NOT NULL UNIQUE,
    parent_component_iri iri,
    restriction_iri      iri,
    subject_iri          iri,
    object_iri           iri,
    created_at           timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_constraints_parent_idx
    ON sbol_constraints (parent_component_iri);

CREATE INDEX sbol_constraints_subject_object_idx
    ON sbol_constraints (subject_iri, object_iri);

CREATE INDEX sbol_constraints_restriction_idx
    ON sbol_constraints (restriction_iri);

CREATE TABLE sbol_interactions (
    object_id            uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri                  iri NOT NULL UNIQUE,
    parent_component_iri iri,
    interaction_types    ontology_term[] NOT NULL DEFAULT '{}',
    created_at           timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_interactions_parent_idx
    ON sbol_interactions (parent_component_iri);

CREATE INDEX sbol_interactions_types_gin
    ON sbol_interactions USING gin (interaction_types);

CREATE TABLE sbol_participations (
    object_id       uuid PRIMARY KEY REFERENCES sbol_objects(id) ON DELETE CASCADE,
    iri             iri NOT NULL UNIQUE,
    interaction_iri iri,
    participant_iri iri,
    roles           ontology_term[] NOT NULL DEFAULT '{}',
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_participations_interaction_idx
    ON sbol_participations (interaction_iri);

CREATE INDEX sbol_participations_participant_idx
    ON sbol_participations (participant_iri);

CREATE INDEX sbol_participations_roles_gin
    ON sbol_participations USING gin (roles);
