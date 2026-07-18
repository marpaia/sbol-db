# SynBioHub differential conformance harness

This harness proves that sbol-db's SynBioHub-compat server behaves like classic
SynBioHub. The oracle is flipped from the Virtuoso-drop-in suite: here an
unmodified classic SynBioHub stack (Node + Virtuoso + SBOLExplorer +
Elasticsearch + libSBOLj) is the **reference** we diff against, and sbol-db is
the **subject** under test, run once per storage backend (postgres, sqlite,
rocksdb) exactly as the bench harness does.

For every case the driver issues the **identical** HTTP request to the reference
and to each subject, then compares the responses semantically per the payload.

## Layout

| File | Role |
| --- | --- |
| `docker-compose.yaml` | reference SynBioHub full stack + three sbol-db subjects |
| `compare.py` | comparison library (HTML, semantic SBOL/GFF/OMEX, SPARQL/JSON) |
| `conformance.py` | pytest driver: identical-request fan-out, auth token threading, mutation read-back |
| `cases.py` | the full V1 case list (byte-equal tier: every endpoint except /similar + /similarCount) |
| `cycle.sh` | one-command reproducible full-corpus run (reference + subject + seed + reindex + differential) |
| `seed_both.py` | submit an identical corpus to the reference and the subject |
| `run53.py` | run the full case list and print a per-case pass/fail table |
| `reference-patches/patch_explorer.py` | in-place SBOLExplorer corpus-indexing fixes applied by cycle.sh |
| `subject.py` | boots a local sbol-db subject (SQLite/RocksDB) as a subprocess and seeds it |
| `conftest.py` | live fixtures (reference/subject targets, stack health gate) |
| `test_compare.py` | comparison-library unit tests (no stack required) |
| `test_conformance_driver.py` | driver fan-out unit tests (no stack required) |
| `test_subject_smoke.py` | self-consistency smoke: subject side end to end, no classic stack |
| `test_differential_subset.py` | best-effort live subset run (skips when the stack is down) |
| `fixtures/corpus/smoke.xml` | the object-scoped anchor (mints /public/smoke/pSmoke/1 on both sides) |
| `fixtures/smoke-corpus.nt` | the N-Triples form of the smoke anchor, for the graph-store subset |
| `run-conformance.sh` | bring up the stack, run the matrix, write the diff report |

## One-command full-corpus run

```sh
tests/synbiohub-conformance/cycle.sh run
```

`cycle.sh run` brings the reference up with its full search stack
(`useSBOLExplorer=true` + Elasticsearch + SBOLExplorer), applies the
container-local SBOLExplorer patches the corpus needs (`patch_explorer.py`:
drop vsearch's redundant `-sort length`, skip empty/gapped/non-nucleotide
sequences, resolve real `sbolType`/`type` in the empty-`/search` projection),
seeds the full SBOL2 test corpus into the reference and the subject
identically, rebuilds the subject's native search index on its embedded worker,
then runs the byte-equal differential. Set `FRESH=1` to tear the reference down
(`down -v`) and reseed it from scratch; a warm reference is reused and only the
subject is reseeded. `/similar` and `/similarCount` are outside this tier and
are characterized in `docs/similar-explorer-gap.md`.

## Comparison methods

The comparator is chosen by payload, following classic SynBioHub's own rules:

- **HTML** (`compare_html`): line diff with `difflib` after removing every
  element whose class is `testignore` or `buorg` and normalizing whitespace, so
  presentation chrome that legitimately differs is ignored.
- **SBOL / GFF / OMEX** (semantic, not byte diff):
  - SBOL/RDF (`compare_rdf`) is parsed into RDF graphs and tested for blank-node
    aware isomorphism; `compare_sbol_via_validator` is the validator.sbolstandard.org
    `test_equality:true` equivalent for CI.
  - GFF (`compare_gff`) compares the set of feature records.
  - OMEX (`compare_omex`) compares manifest content entries plus the member set,
    each member semantically when it is RDF.
- **SPARQL / JSON** (structural): `compare_sparql` compares `head.vars` and the
  `results.bindings` as sets (order-insensitive); `compare_json_setequal`
  compares metadata JSON as an order-insensitive structural set.

## Self-consistency smoke (no classic stack)

`test_subject_smoke.py` boots the compiled `sbol-db` compat server on SQLite and
RocksDB (via `subject.py`), seeds `fixtures/smoke-corpus.nt`, and drives the
read/metadata/SPARQL/download subset, asserting each endpoint answers
coherently. Its last test runs the driver itself across two independent backends
(SQLite as the stand-in reference, RocksDB as the subject) so the fan-out and
semantic comparators are exercised end to end without a classic reference. It
locates the binary from `$SBOLDB_BIN`, then a prebuilt `target/{release,debug}`
artifact, else `cargo build -p sbol-db`.

```sh
SBOLDB_BIN=target/debug/sbol-db \
    tests/synbiohub-conformance/.venv/bin/python -m pytest \
    tests/synbiohub-conformance/test_subject_smoke.py
```

## Running the unit tests (no stack)

```sh
python3 -m venv tests/synbiohub-conformance/.venv
tests/synbiohub-conformance/.venv/bin/pip install -r tests/synbiohub-conformance/requirements.txt
tests/synbiohub-conformance/.venv/bin/python -m pytest \
    tests/synbiohub-conformance/test_compare.py \
    tests/synbiohub-conformance/test_conformance_driver.py
```

## Running the live matrix

The reference services are amd64 images. On Apple Silicon they run under
emulation; the full stack (Virtuoso 7.2.5 + SynBioHub snapshot-standalone +
Elasticsearch 6.3.2 + SBOLExplorer) boots and drives the full-corpus differential
on a 128 GB arm64 host. Elasticsearch 6.3.2 is memory-hungry under emulation, so
give Docker generous memory. The comparison-library and driver unit tests run
anywhere.

The `cycle.sh run` path above is the maintained entry point. `run-conformance.sh`
and `test_differential_subset.py` remain for the pytest-driven subset:

```sh
cp tests/synbiohub-conformance/.env.example tests/synbiohub-conformance/.env   # adjust
tests/synbiohub-conformance/run-conformance.sh --corpus ~/git/SynBioHub/synbiohub/tests/Emulated
```

`test_differential_subset.py` runs the Elasticsearch-independent
read/metadata/SPARQL/download subset (`read_subset_cases()` in `cases.py`)
against the live reference and every subject. It is gated on the `stack` fixture,
so it skips cleanly when the reference or any subject is unreachable.

The runner leaves the stack up and prints the teardown command; pass `--down` to
tear it down afterward. The reference-vs-subject diff report is written to
`results/conformance-<host>.json` (override with `CONFORMANCE_OUT`).

Tear the stack down manually with:

```sh
docker compose -p sbhconformance -f tests/synbiohub-conformance/docker-compose.yaml down -v
```
