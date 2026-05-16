CREATE TABLE sbol_objects (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    iri                 sbol_iri NOT NULL UNIQUE,
    sbol_class          text NOT NULL,

    persistent_identity sbol_iri,
    display_id          text,
    version             text,

    name                text,
    description         text,

    document_id         uuid REFERENCES sbol_documents(id) ON DELETE SET NULL,

    types               sbol_ontology_term[] NOT NULL DEFAULT '{}',
    roles               sbol_ontology_term[] NOT NULL DEFAULT '{}',

    data                jsonb NOT NULL DEFAULT '{}'::jsonb,
    content_hash        bytea,

    is_deleted          boolean NOT NULL DEFAULT false,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sbol_objects_class_idx
    ON sbol_objects (sbol_class);

CREATE INDEX sbol_objects_persistent_identity_idx
    ON sbol_objects (persistent_identity);

CREATE INDEX sbol_objects_display_id_trgm_idx
    ON sbol_objects USING gin (display_id gin_trgm_ops);

CREATE INDEX sbol_objects_name_trgm_idx
    ON sbol_objects USING gin (name gin_trgm_ops);

CREATE INDEX sbol_objects_types_gin
    ON sbol_objects USING gin (types);

CREATE INDEX sbol_objects_roles_gin
    ON sbol_objects USING gin (roles);

CREATE INDEX sbol_objects_data_gin
    ON sbol_objects USING gin (data jsonb_path_ops);

CREATE INDEX sbol_objects_document_class_idx
    ON sbol_objects (document_id, sbol_class);

CREATE INDEX sbol_objects_live_idx
    ON sbol_objects (iri)
    WHERE is_deleted = false;
