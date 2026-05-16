-- Phase 3: ontology terms + transitive closure for role/type expansion.
--
-- The high-leverage SBOL ontologies are SO (sequence features and roles) and
-- SBO (interaction types). ChEBI / EDAM / GO / NCIT may follow. Each
-- ontology occupies one row in `ontologies` and contributes terms to
-- `ontology_terms` keyed by canonical IRI.
--
-- `ontology_term_aliases` lets the same term resolve via either canonical
-- OBO Foundry IRI (`http://purl.obolibrary.org/obo/SO_0000167`) or the
-- identifiers.org form (`http://identifiers.org/so/SO:0000167`) that SBOL
-- documents historically use.
--
-- `ontology_closure` is precomputed at load time. The (ancestor, descendant)
-- pairs include the trivial self-pair (X, X, 0). Query-time role expansion
-- joins ontology_closure on ancestor and uses descendant_iri as the array
-- containment probe against sbol_components.roles / sbol_features.roles.

CREATE TABLE ontologies (
    prefix      text PRIMARY KEY,
    name        text NOT NULL,
    source_url  text,
    version     text,
    term_count  integer NOT NULL DEFAULT 0,
    imported_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ontology_terms (
    iri         iri PRIMARY KEY,
    prefix      text NOT NULL REFERENCES ontologies(prefix) ON DELETE CASCADE,
    curie       text NOT NULL,
    name        text NOT NULL,
    definition  text,
    is_obsolete boolean NOT NULL DEFAULT false,
    synonyms    text[] NOT NULL DEFAULT '{}'
);

CREATE INDEX ontology_terms_prefix_idx ON ontology_terms (prefix);
CREATE INDEX ontology_terms_curie_idx  ON ontology_terms (curie);
CREATE INDEX ontology_terms_name_trgm  ON ontology_terms USING gin (name gin_trgm_ops);

CREATE TABLE ontology_term_aliases (
    alias_iri     iri PRIMARY KEY,
    canonical_iri iri NOT NULL REFERENCES ontology_terms(iri) ON DELETE CASCADE
);

CREATE INDEX ontology_term_aliases_canonical_idx
    ON ontology_term_aliases (canonical_iri);

CREATE TABLE ontology_closure (
    ancestor_iri   iri NOT NULL REFERENCES ontology_terms(iri) ON DELETE CASCADE,
    descendant_iri iri NOT NULL REFERENCES ontology_terms(iri) ON DELETE CASCADE,
    depth          smallint NOT NULL CHECK (depth >= 0),
    PRIMARY KEY (ancestor_iri, descendant_iri)
);

CREATE INDEX ontology_closure_descendant_idx
    ON ontology_closure (descendant_iri);
