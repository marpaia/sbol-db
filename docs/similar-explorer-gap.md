# `/similar` gap: sbol-db native clustering vs live SBOLExplorer

This report quantifies how close sbol-db's native `/similar` clustering is to a
live SBOLExplorer running over the full SBOL2 test corpus, and characterizes
every difference by cause. The measurement backs a design decision: sbol-db
clusters with a correct, scalable global-identity model (MinHash/LSH plus banded
alignment) and deliberately does not replicate SBOLExplorer's vsearch
`cluster_fast` byte behavior. Differences are therefore expected; the goal is to
measure and explain them, not to force equality.

## Stack under measurement

- Reference: classic SynBioHub with `useSBOLExplorer=true`, Elasticsearch, and a
  live SBOLExplorer, all seeded from the full SBOL2 corpus (179 files, 114 public
  collections). SBOLExplorer carries the container-local patches it needs to
  index this corpus: it drops vsearch's `-sort length` (which `cluster_fast`
  rejects as redundant and aborts on) and skips empty, gapped, and
  non-nucleotide sequences.
- Subject: sbol-db on SQLite, run with the embedded worker so
  `rebuild_search_index` populates the sketch and cluster stages. A full-corpus
  rebuild produced 4308 sketches, 4424 clustered parts across 1078 clusters, and
  PageRank over 9111 objects.

## Method

The two engines reshape object URIs differently on submit (classic rewrites
displayIds, e.g. `cd_BBa_C0062`), so an IRI-to-IRI set comparison over the HTTP
layer is not apples-to-apples. Because both engines cluster by sequence, each
part is keyed by its sequence `elements` to normalize away the URI-shape
differences, a comparison that is robust regardless of how either side mints
URIs.
For each subject part that carries a sequence, the subject cluster-mate set (from
`sbol_sequence_cluster`) and the reference cluster-mate set (from SBOLExplorer's
`clusters_dump`) are each mapped to the set of mate sequences, then compared for
exact-set-match and Jaccard overlap. This compares the two clusterings directly
and bypasses both engines' URI-minting and HTTP-wrapper behavior.

## Aggregate numbers

| Metric | Value |
| --- | --- |
| Parts compared | 4424 |
| Exact set match | 1356 (30.7%) |
| Exact matches that are both-empty (trivial) | 121 |
| Exact matches that are non-empty | 1235 |
| Mean Jaccard (all parts) | 0.527 |
| Mean Jaccard (parts with a mate on either side) | 0.513 |
| Non-exact parts | 3068 |

Directionality of the non-exact parts:

| Direction | Count |
| --- | --- |
| Reference clusters mates the subject does not (reference over-clusters) | 1625 |
| Subject clusters mates the reference does not | 782 |
| Both directions differ | 661 |

The reference over-clusters more than twice as often as the subject. The gap is
not the subject missing mates; it is the reference forming larger, looser
clusters.

## Cause breakdown

| Cause | Count | Meaning |
| --- | --- | --- |
| so-role-filter | 0 | Not present in this SBOLExplorer build |
| borderline-clustering | 293 | Similar-length pair straddling the 0.8 identity threshold |
| real-bug (clustering) | 0 | No clustering defect: no empty/crash/malformed where a correct result exists |
| other (model difference) | 2775 | vsearch local/centroid identity vs native global identity |

The `other` bucket splits into length-mismatch / local containment merges (1634)
and low-global-similarity centroid chaining (1141). No non-nucleotide-skip
differences surfaced in the shared-sequence universe.

### (a) so-role-filter: absent

The premise that SBOLExplorer drops parts carrying a Sequence Ontology role does
not hold for this build. SBOLExplorer's clustering query selects every
`ComponentDefinition` with a sequence and applies no role filter. Verification:
`cd_BBa_C0062` carries `SO:0000316` (CDS) and is clustered with 14 mates. Role
membership does not change clustering on either side, so this cause contributes
nothing to the gap.

### (b) borderline-clustering: 293

Genuine threshold ambiguity: a same-length pair whose similarity sits just below
the point where one side accepts the alignment and the other rejects it. Example:
`_3xFLAG_003` (66 bp) has a decisive same-length mate at k-mer Jaccard 0.507 that
the reference clusters and the subject does not. Cases like this are inherent to
any hard identity cutoff and are split roughly evenly between the two engines.

### (c) real-bug: 0 in clustering

Three candidates initially looked like subject recall failures: the plasmids
`pICH41551`, `pICH49477`, and `pICH75111` (clean 3-4 kb nucleotide sequences)
are singletons for the subject but carry ~20 reference mates. Inspection shows
the reference mates are not globally similar. For `pICH41551` (3748 bp) the
reference cluster mixes a 589 bp origin of replication whose 12-mers are 99%
contained in the plasmid (whole-sequence k-mer Jaccard 0.154) with other plasmids
sharing only a backbone region (Jaccard ~0.49). vsearch `cluster_fast` accepts
these on local/centroid identity; the subject's global model correctly declines
to call a 589 bp fragment "similar" to a 3748 bp plasmid. The subject is the more
precise of the two here, so these are design differences, not defects.

### (d) other, the dominant cause: 2775

SBOLExplorer's vsearch `cluster_fast` clusters against a centroid using local
alignment, which merges sequences that share a region but differ globally, and it
chains transitively (A joins B, B joins C, so A, B, and C share a cluster even
when A and C are unrelated). Example: `BBa_B0015` (129 bp terminator) is clustered
by the reference with `cd_BBa_C0062` (701 bp CDS, k-mer Jaccard 0.000) and
`BBa_I0462` (936 bp, Jaccard 0.128) alongside several terminators at Jaccard
0.18-0.46. The subject keeps `BBa_B0015` with only its exact twin. Removing
`-sort length` (required for this corpus, since `cluster_fast` aborts otherwise)
also makes vsearch centroid selection partly input-order dependent, so a slice of
these reference clusters is not reproducible in principle.

## Implication: byte-parity with vsearch is not worth pursuing

The clustering gap (mean Jaccard ~0.51, 31% exact set match) is almost entirely a
model difference. SBOLExplorer's vsearch `cluster_fast` over-clusters through local
containment merges and transitive centroid chaining, and its centroid choice is
partly order-dependent once `-sort length` is removed for this corpus. Matching it
byte for byte would mean adopting local-alignment centroid clustering and
reproducing its chaining and order sensitivity, trading away the precision and
near-linear scalability of the native global model for a target that is itself not
fully reproducible. The 293 borderline cases are the only differences a threshold
tweak could move, and they split roughly evenly between the engines, so no single
adjustment closes the gap. The native global-identity clustering is the sounder
primitive: more precise, near-linear, and independent of vsearch's input-order
sensitivity.
