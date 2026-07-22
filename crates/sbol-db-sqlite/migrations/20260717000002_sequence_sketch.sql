-- MinHash/LSH similarity sketch index backing scalable clustering and
-- sequence-similarity search.
--
-- `sbol_sequence_sketch` holds one fixed-size MinHash signature per sequence
-- (little-endian packed u64 slots). `sbol_sequence_lsh_band` holds the LSH band
-- buckets a sequence falls into: one row per (band_hash, sequence). Candidate
-- generation is a posting-list union over band_hash, so the composite primary
-- key (band_hash first) serves that lookup; the index on sequence_iri backs the
-- delete-by-sequence a re-index performs.
--
-- band_hash is a 64-bit value stored in an INTEGER column (i64) by bit
-- reinterpretation; the search layer only tests equality, so the sign is
-- immaterial.

CREATE TABLE sbol_sequence_sketch (
    sequence_iri TEXT PRIMARY KEY,
    signature    BLOB NOT NULL
);

CREATE TABLE sbol_sequence_lsh_band (
    band_hash    INTEGER NOT NULL,
    sequence_iri TEXT    NOT NULL,
    PRIMARY KEY (band_hash, sequence_iri)
);

CREATE INDEX sbol_sequence_lsh_band_iri ON sbol_sequence_lsh_band (sequence_iri);
