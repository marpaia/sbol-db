# Differential conformance suite

The differential conformance suite proves that sbol-db's SynBioHub-compat
server behaves like classic SynBioHub. It lives in
[`tests/synbiohub-conformance/`](../tests/synbiohub-conformance/); that
directory's `README.md` is the operational reference, and this document is
the orientation: what the suite is, how to run each tier, how comparison
works, and the environment caveats that bound a live run.

## The oracle flip

The Virtuoso-drop-in suite treats sbol-db as a triplestore and diffs it
against Virtuoso. This suite flips the oracle: an unmodified classic
SynBioHub stack (Node + Virtuoso + SBOLExplorer + Elasticsearch + libSBOLj)
is the **reference**, and sbol-db is the **subject** under test, run once per
storage backend (postgres, sqlite, rocksdb) the way the bench harness does.

For every case the driver issues the **identical** HTTP request to the
reference and to each subject, then compares the responses semantically,
choosing the comparator by payload.

## The three tiers

The suite is layered so that most of it runs with no classic stack, and only
the final tier needs the emulated reference.

1. **Comparison-library and driver unit tests** (`test_compare.py`,
   `test_conformance_driver.py`) exercise the comparators and the
   request-fan-out logic with fixtures. No stack, no binary. These are the
   hard, must-pass deliverable and run anywhere.
2. **Self-consistency smoke** (`test_subject_smoke.py`) boots the compiled
   `sbol-db` compat server on SQLite and RocksDB, seeds
   `fixtures/smoke-corpus.nt`, and drives the read/metadata/SPARQL/download
   subset, asserting each endpoint answers coherently. Its last test runs the
   driver itself across two independent backends (SQLite as a stand-in
   reference, RocksDB as the subject) so the fan-out and semantic comparators
   are exercised end to end with no classic reference.
3. **Best-effort live subset** (`test_differential_subset.py`) runs the
   Elasticsearch-independent subset against the live classic reference and
   every subject, seeding the shared corpus into both sides. It is gated on
   the `stack` fixture, so it skips cleanly when the reference or any subject
   is unreachable.

## How comparison works

The comparator is chosen by payload, following classic SynBioHub's own test
rules (ported from its `tests/test_functions.py`). Each comparator returns a
structured diff (equal, detail, context).

- **HTML** (`compare_html`): a `difflib` line diff taken after removing every
  element whose class is `testignore` or `buorg` and normalizing whitespace,
  so presentation chrome that legitimately differs between the two servers is
  ignored.
- **SBOL / GFF / OMEX** are compared **semantically**, never as byte diffs:
  - SBOL/RDF (`compare_rdf`) is parsed into RDF graphs and tested for
    blank-node-aware isomorphism. `compare_sbol_via_validator` is the
    `validator.sbolstandard.org` `test_equality:true` equivalent, with an
    injectable HTTP poster so it is unit-testable offline.
  - GFF (`compare_gff`) compares the set of feature records.
  - OMEX (`compare_omex`) compares manifest content entries plus the member
    set, each member compared semantically when it is RDF.
- **SPARQL / JSON** are compared **structurally**: `compare_sparql` compares
  `head.vars` and the `results.bindings` as order-insensitive sets (and ASK
  booleans directly); `compare_json_setequal` folds metadata JSON into an
  order-insensitive canonical set.

## Running the suite

### Unit tests (no stack, no binary)

```sh
python3 -m venv tests/synbiohub-conformance/.venv
tests/synbiohub-conformance/.venv/bin/pip install -r tests/synbiohub-conformance/requirements.txt
tests/synbiohub-conformance/.venv/bin/python -m pytest \
    tests/synbiohub-conformance/test_compare.py \
    tests/synbiohub-conformance/test_conformance_driver.py
```

### Self-consistency smoke (needs only the sbol-db binary)

The smoke locates the binary from `$SBOLDB_BIN`, then a prebuilt
`target/{release,debug}` artifact, else builds it with
`SBOL_DB_SKIP_UI_BUILD=1 cargo build -p sbol-db`.

```sh
SBOLDB_BIN=target/debug/sbol-db \
    tests/synbiohub-conformance/.venv/bin/python -m pytest \
    tests/synbiohub-conformance/test_subject_smoke.py
```

### Live matrix (needs the classic stack)

Copy and adjust the environment file, then use the runner, which brings up
the stack, waits for health, runs the matrix, and writes the diff report:

```sh
cp tests/synbiohub-conformance/.env.example tests/synbiohub-conformance/.env
tests/synbiohub-conformance/run-conformance.sh --corpus /path/to/sbol2-rdfxml-dir
```

The stack and target URLs are configured through environment variables
(defaults match the host ports in `docker-compose.yaml`):

| Variable | Purpose |
| --- | --- |
| `REFERENCE_URL` | Classic SynBioHub Node app (the reference's HTTP surface) |
| `REFERENCE_VIRTUOSO_URL` | The reference's Virtuoso, used to seed the corpus directly |
| `SUBJECT_POSTGRES_URL` / `SUBJECT_SQLITE_URL` / `SUBJECT_ROCKSDB_URL` | The three sbol-db subjects |
| `VALIDATOR_URL` | The SBOL validator used as the semantic-equality oracle |
| `CORPUS` | Directory of SBOL2 RDF/XML files seeded into both sides |
| `CONFORMANCE_OUT` | Path for the reference-vs-subject diff report |

The runner leaves the stack up and prints the teardown command; pass
`--down` to tear it down afterward. Tear down manually with:

```sh
docker compose -p sbhconformance -f tests/synbiohub-conformance/docker-compose.yaml down -v
```

## Environment caveats

The classic reference images are amd64. On Apple Silicon they run under
emulation, which bounds what a live run can cover:

- **Virtuoso** (amd64) runs emulated. It boots and is directly usable: the
  corpus loads and SPARQL returns it.
- **Elasticsearch 6.3.2** is prone to OOM under emulation. The classic app
  serves objects only through its submission pipeline (libSBOLj conversion +
  Elasticsearch indexing), so with Elasticsearch down the app's read surface
  cannot serve a raw-seeded corpus.
- The classic Node app gates every route behind its first-boot `/setup`
  onboarding, which the 1.6.1 standalone build does not complete reliably
  under scripted setup.

On an arm64 developer host the reference stack (Virtuoso +
SynBioHub) boots healthy and Virtuoso is usable, but a valid
classic-vs-sbol-db comparison of even the Elasticsearch-independent subset
could not be driven, so `test_differential_subset.py` stays best-effort and
is intended for an amd64 CI runner. The self-consistency smoke proves the
subject side regardless of whether the classic reference is reachable.
