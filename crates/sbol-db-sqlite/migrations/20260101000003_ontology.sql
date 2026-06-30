-- Ontology storage: the ontology, its terms, alias IRIs, and the precomputed
-- transitive `is_a` closure. Terms, aliases, and closure all carry `prefix`
-- with an ON DELETE CASCADE back to the ontology, so reloading an ontology is
-- a single DELETE of its row followed by fresh inserts. Synonyms are JSON text.

CREATE TABLE sbol_ontologies (
    prefix      TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    source_url  TEXT,
    version     TEXT,
    term_count  INTEGER NOT NULL DEFAULT 0,
    imported_at TEXT NOT NULL
);

CREATE TABLE sbol_ontology_terms (
    iri         TEXT PRIMARY KEY,
    prefix      TEXT NOT NULL REFERENCES sbol_ontologies (prefix) ON DELETE CASCADE,
    curie       TEXT NOT NULL,
    name        TEXT NOT NULL,
    definition  TEXT,
    is_obsolete INTEGER NOT NULL DEFAULT 0,
    synonyms    TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX sbol_ontology_terms_prefix ON sbol_ontology_terms (prefix, curie);

CREATE TABLE sbol_ontology_term_aliases (
    alias_iri     TEXT PRIMARY KEY,
    canonical_iri TEXT NOT NULL,
    prefix        TEXT NOT NULL REFERENCES sbol_ontologies (prefix) ON DELETE CASCADE
);

CREATE TABLE sbol_ontology_closure (
    ancestor_iri   TEXT NOT NULL,
    descendant_iri TEXT NOT NULL,
    depth          INTEGER NOT NULL,
    prefix         TEXT NOT NULL REFERENCES sbol_ontologies (prefix) ON DELETE CASCADE,
    PRIMARY KEY (ancestor_iri, descendant_iri)
);

CREATE INDEX sbol_ontology_closure_ancestor ON sbol_ontology_closure (ancestor_iri);
