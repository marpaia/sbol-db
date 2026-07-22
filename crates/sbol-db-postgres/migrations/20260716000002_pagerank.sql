-- Object PageRank scores backing the native ranked search.
--
-- One row per ranked top-level object. The search-index rebuild recomputes the
-- whole table and replaces it atomically, so a read never sees a partial
-- ranking.

CREATE TABLE object_pagerank (
    iri   text             PRIMARY KEY,
    score double precision NOT NULL
);
