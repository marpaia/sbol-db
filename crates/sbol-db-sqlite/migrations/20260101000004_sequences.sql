-- Nucleotide sequences plus the canonical 8-mer seed index that backs
-- substring + reverse-complement search. Keyed by sequence IRI (the Postgres
-- backend keys the k-mer table by the object id; SQLite keys both by IRI, which
-- is what the search joins on).

CREATE TABLE sbol_sequences (
    iri          TEXT PRIMARY KEY,
    encoding_iri TEXT,
    elements     TEXT,
    alphabet     TEXT,
    content_hash BLOB
);

CREATE TABLE sbol_sequence_kmers (
    sequence_iri TEXT NOT NULL,
    kmer         INTEGER NOT NULL,
    position     INTEGER NOT NULL,
    strand       TEXT NOT NULL
);

CREATE INDEX sbol_sequence_kmers_kmer ON sbol_sequence_kmers (kmer);
CREATE INDEX sbol_sequence_kmers_iri ON sbol_sequence_kmers (sequence_iri);
