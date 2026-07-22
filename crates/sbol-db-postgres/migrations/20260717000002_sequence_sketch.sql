-- MinHash/LSH similarity sketch index backing scalable clustering and
-- sequence-similarity search.
--
-- `sbol_sequence_sketch` holds one fixed-size MinHash signature per sequence
-- (little-endian packed u64 slots). `sbol_sequence_lsh_band` holds the LSH band
-- buckets a sequence falls into: one row per (band_hash, sequence). Candidate
-- generation is a posting-list union over band_hash, so the composite primary
-- key (band_hash first) serves that lookup as a prefix range scan; the index on
-- sequence_iri backs the delete-by-sequence a re-index performs.
--
-- band_hash is a 64-bit value stored as a signed bigint (bit reinterpretation);
-- the search layer never orders by it, only tests equality, so the sign is
-- immaterial. The search-index rebuild recomputes and replaces these rows; a
-- reader unions the query's band buckets and the banded aligner verifies the
-- candidates.

CREATE TABLE sbol_sequence_sketch (
    sequence_iri text  PRIMARY KEY,
    signature    bytea NOT NULL
);

CREATE TABLE sbol_sequence_lsh_band (
    band_hash    bigint NOT NULL,
    sequence_iri text   NOT NULL,
    PRIMARY KEY (band_hash, sequence_iri)
);

CREATE INDEX sbol_sequence_lsh_band_iri_idx ON sbol_sequence_lsh_band (sequence_iri);
