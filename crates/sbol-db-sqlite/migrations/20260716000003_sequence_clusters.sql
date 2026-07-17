-- Sequence cluster assignments backing /similar.
--
-- One row per clustered sequence, mapping its IRI to the numeric id of the
-- cluster it belongs to. The search-index rebuild recomputes every assignment
-- (greedy centroid clustering) and replaces the whole table in one transaction,
-- so a read never sees a partial clustering. The index on cluster_id makes
-- cluster-mate lookup (the /similar candidate set) a single indexed scan.

CREATE TABLE sbol_sequence_cluster (
    sequence_iri TEXT    PRIMARY KEY,
    cluster_id   INTEGER NOT NULL
);

CREATE INDEX sbol_sequence_cluster_cluster ON sbol_sequence_cluster (cluster_id);
