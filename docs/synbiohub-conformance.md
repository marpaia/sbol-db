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

## Running the live tier locally

The live tier runs on an arm64 developer host against the
`synbiohub/synbiohub:snapshot-standalone` image, which bundles the Node app
and its Virtuoso and boots healthy under emulation. Use `snapshot-standalone`,
not `1.6.1-standalone`: the `1.6.1` build crashes during `/setup`.

Bring-up and seed recipe (proven on this host):

1. Start the reference (classic SynBioHub at `:17777`) and a native sbol-db
   subject. The subject runs from the current build, on SQLite, with public
   signup enabled so it can register the shared account:

   ```sh
   DATABASE_URL="sqlite:///tmp/sbol-db-subject.sqlite?mode=rwc" \
       SBOL_DB_SKIP_UI_BUILD=1 sbol-db db migrate
   DATABASE_URL="sqlite:///tmp/sbol-db-subject.sqlite?mode=rwc" \
       SBOL_DB_ALLOW_PUBLIC_SIGNUP=true SBOL_DB_SKIP_UI_BUILD=1 \
       sbol-db server --bind 127.0.0.1:18903 --no-worker
   ```

2. First boot runs `/setup`, which creates admin `test@user.synbiohub` / `test`
   and sets `uriPrefix=http://synbiohub.org/` so the reference mints the SAME
   top-level URIs sbol-db does. Both sides then mint identical URIs and
   responses compare directly with no base-URI canonicalization.

3. Load `fixtures/smoke-corpus.nt` into each side's public graph over the
   graph-store protocol, then seed a curated SBOL2 corpus into both through
   `/submit`:

   ```sh
   python3 tests/synbiohub-conformance/seed_both.py \
       --reference http://localhost:17777 \
       --subject  http://127.0.0.1:18903 \
       --corpus   /path/to/sbol2-xml-dir
   ```

The account (`testuser`) is shared, so one `/login` form authenticates both
targets. Because the reference enforces `requireLogin`, unauthenticated public
reads answer `401`; the driver mints a token on both sides for every case so
body shapes are compared rather than masked by a `401`/`200` status split.

## The full-corpus differential

`cycle.sh run` is the authoritative live differential. It brings up the classic
reference, seeds the full SBOL2 test corpus (180 files) into the reference and
the subject through `/submit` and `/makePublic` (`seed_both.py`), rebuilds the
subject's native search index, and runs `run53.py`, which issues each case's
identical request to both sides and compares by payload.

`run53.py` mutates the reference (the tier exercises `/submit`, `/makePublic`,
edit, and account routes), so a trustworthy run wipes the reference first: use
`FRESH=1 cycle.sh run`. A non-`FRESH` rerun reuses the reference's persisted
state from the prior run and reads back mutated values, which is not a clean
comparison.

Both sides mint under `http://synbiohub.org/` and validate submissions as SBOL2
through `sbol-rs`, so identical corpus input produces identical top-level
identities and the responses compare directly.

## Result

The byte-equal tier — every V1 endpoint whose two implementations agree by
design — compares **equal across the full corpus**. This covers the read and
query surface (`/metadata`, `/rootCollections`, `/subCollections`, `/uses`,
`/twins`, the `/:type/count` family, `/manage`, `/shared`, `/sparql`), every
download format (`/sbol`, `/sbolnr`, versioned SBOL, `/gff`, `/fasta`,
`/genbank`, `/omex`, `/summary`), and the mutating surface (`/submit`,
`/makePublic`, the edit and permission routes, `/register`, `/login`,
`/logout`, profile).

A small set of endpoints diverge by design and are reported separately, each
with a recorded reason (`Case.expected_divergence`). They are not forced to a
false match: matching them would mean replicating a classic defect or
abandoning a sound design choice.

### Classic defects, where sbol-db is correct

- `POST /resetPassword`: classic answers `401` for a known email and `500`
  (`Cannot set property 'resetPasswordLink' of null`) for an unknown one. A
  reset request carries no credentials, so sbol-db returns the correct `200`
  acknowledgement regardless of whether the address exists, which also avoids
  disclosing account existence.
- `/search/objectType=<type>`: SBOLExplorer returns `[]` for an object-type
  facet. sbol-db resolves the facet to an `rdf:type` filter and returns the
  matching objects.

### Both valid, different by design

- `/search` and `/search/<text>`: classic ranks through SBOLExplorer's
  Elasticsearch scoring; sbol-db ranks through its native BM25 index, so the
  result ordering and recall differ. This is the ranked-search analogue of the
  `/similar` model difference documented in
  [`similar-explorer-gap.md`](similar-explorer-gap.md).
- `<uri>/similar` and `<uri>/similarCount`: the reference ranks cluster mates
  through SBOLExplorer's vsearch clustering; sbol-db clusters with a native
  global-identity model. The gap is measured and explained in
  [`similar-explorer-gap.md`](similar-explorer-gap.md).
- `/ComponentDefinition/count` and the empty `/search` and `/searchCount`: two
  URI-minting differences move these. Classic's libSBOLj compliance pass folds a
  source URI's namespace segment into the display id (`http://…/example/toggle_switch`
  becomes `example_toggle_switch`) even absent a collision, while sbol-db keeps
  the submitted display id. Classic also drops a submitted object whose source
  URI already lies under the instance's own public namespace, treating it as a
  reference to an existing object, while sbol-db preserves every submitted
  object, so its object count is one higher. Where two submitted objects share a
  display id under different source namespaces (`.../cd/BBa_B0015` and
  `.../seq/BBa_B0015`), both engines disambiguate identically with the source
  namespace prefix (`cd_BBa_B0015`, `seq_BBa_B0015`), so those objects compare
  equal.
- `/sparql` `SELECT COUNT(*)` over the public graph: libSBOLj re-mints child
  objects (`SequenceAnnotation`, `Location`, `Component`) as versioned
  identities and stamps `sbol:version` on each, while sbol-db stores children
  verbatim as submitted. The two graphs are semantically equivalent — every
  download and serialization case compares byte-equal — so only the raw triple
  total differs.

The typed-literal token also differs on `/sparql`: the reference (Virtuoso)
emits the legacy `{"type":"typed-literal"}`, while sbol-db emits the SPARQL 1.1
`{"type":"literal"}` with the same datatype. The comparator treats these as
equal, since the binding value and datatype are identical.

The self-consistency smoke (`test_subject_smoke.py`) proves the subject side
end to end regardless of whether the classic reference is reachable.
