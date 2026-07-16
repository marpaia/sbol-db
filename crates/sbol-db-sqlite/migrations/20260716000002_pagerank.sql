-- Object PageRank scores backing the native ranked search.
--
-- One row per ranked top-level object. The search-index rebuild recomputes the
-- whole table and replaces it in one transaction, so a read never sees a
-- partial ranking. Scores are stored as REAL, matching the rest of the SQLite
-- backend.

CREATE TABLE object_pagerank (
    iri   TEXT PRIMARY KEY,
    score REAL NOT NULL
);
