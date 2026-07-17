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
| `cases.py` | the Elasticsearch-independent read/metadata/SPARQL/download subset |
| `subject.py` | boots a local sbol-db subject (SQLite/RocksDB) as a subprocess and seeds it |
| `conftest.py` | live fixtures (reference/subject targets, stack health gate) |
| `test_compare.py` | comparison-library unit tests (no stack required) |
| `test_conformance_driver.py` | driver fan-out unit tests (no stack required) |
| `test_subject_smoke.py` | self-consistency smoke: subject side end to end, no classic stack |
| `test_differential_subset.py` | best-effort live subset run (skips when the stack is down) |
| `fixtures/smoke-corpus.nt` | the shared seed corpus (one Collection + ComponentDefinition + Sequence) |
| `run-conformance.sh` | bring up the stack, run the matrix, write the diff report |

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

The reference services are amd64 images. On Apple Silicon Virtuoso is emulated
and Elasticsearch 6.3.2 is prone to OOM under emulation, so the full live run is
intended for the amd64 CI runner. The comparison-library and driver unit tests
run anywhere.

```sh
cp tests/synbiohub-conformance/.env.example tests/synbiohub-conformance/.env   # adjust
tests/synbiohub-conformance/run-conformance.sh --corpus ~/git/SynBioHub/synbiohub/tests/Emulated
```

`test_differential_subset.py` runs the Elasticsearch-independent
read/metadata/SPARQL/download subset (`cases.py`) against the live reference and
every subject. It seeds the shared corpus into the reference's Virtuoso (the Node
app then serves it) and into each subject's graph store, and is gated on the
`stack` fixture, so it skips cleanly when the reference or any subject is
unreachable.

### Live-run status on this developer environment (Apple Silicon)

Observed on an arm64 host with Docker present and the reference images cached:
the reference stack (Virtuoso 7.2.5 + SynBioHub 1.6.1-standalone) boots and both
containers report healthy under amd64 emulation. Virtuoso is directly usable
(the corpus loads and SPARQL returns it). The classic SynBioHub Node app,
however, gates **every** route behind its first-boot `/setup` onboarding, and
even once onboarded it serves objects only through its submission pipeline
(libSBOLj conversion + Elasticsearch 6.3.2 indexing); Elasticsearch OOMs under
emulation. A valid classic-vs-sbol-db comparison of even the ES-independent
subset therefore could not be driven here, so `test_differential_subset.py`
remains best-effort and CI-runner (amd64) territory. The self-consistency smoke
proves the subject side regardless.

The runner leaves the stack up and prints the teardown command; pass `--down` to
tear it down afterward. The reference-vs-subject diff report is written to
`results/conformance-<host>.json` (override with `CONFORMANCE_OUT`).

Tear the stack down manually with:

```sh
docker compose -p sbhconformance -f tests/synbiohub-conformance/docker-compose.yaml down -v
```
