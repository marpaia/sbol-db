-- Phase 3: nucleotide k-mer index for sequence substring search.
--
-- One row per observed k-mer position in each indexed sequence. The k-mer
-- value stored is the *canonical* form: min(forward, reverse_complement) of
-- the 2-bit packed nucleotide k-mer. `strand` records which arm the
-- canonical form was drawn from -- '+' if the forward k-mer was lex-smaller
-- (or equal) to its reverse complement, '-' otherwise.
--
-- At query time the caller computes the canonical k-mer of each seed in the
-- query pattern, joins on `kmer`, then verifies each candidate hit against
-- the full pattern (forward or reverse complement, determined by `strand`)
-- using a direct substring check on sbol_sequences.elements.
--
-- k = 8 (16-bit packed); ambiguous IUPAC bases (N, R, Y, ...) skip the
-- position rather than emitting partial k-mers.

CREATE TABLE sequence_kmers (
    sequence_object_id uuid    NOT NULL REFERENCES sbol_sequences(object_id) ON DELETE CASCADE,
    kmer               integer NOT NULL,
    position           integer NOT NULL,
    strand             char(1) NOT NULL CHECK (strand IN ('+', '-'))
);

CREATE INDEX sequence_kmers_kmer_idx ON sequence_kmers (kmer);
CREATE INDEX sequence_kmers_seq_idx  ON sequence_kmers (sequence_object_id);
