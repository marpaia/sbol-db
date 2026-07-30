# Built-in BGE-small model release

The default structured vector strategy is
`builtin.sbol-text-vector.v2`. It uses `builtin.sbol-text-bge-small.v1`: the
quantized ONNX bundle published as
[`Qdrant/bge-small-en-v1.5-onnx-Q`](https://huggingface.co/Qdrant/bge-small-en-v1.5-onnx-Q),
which is a Qdrant Apache-2.0 ONNX port of
[`BAAI/bge-small-en-v1.5`](https://huggingface.co/BAAI/bge-small-en-v1.5).

The model is intentionally not committed as a 67 MB Git blob. The repository
ships the complete immutable distribution manifest and installer in
[`docker/fetch-builtin-bge-small-model.sh`](../docker/fetch-builtin-bge-small-model.sh):
the upstream commit and SHA-256 of every runtime file are code-reviewed with
the profile. The production OCI image runs that verifier during its build and
copies the verified bytes into `/opt/sbol-db/models/bge-small-en-v1.5`. Runtime
search and indexing never download weights.

## Pinned artifact

| Field | Value |
| --- | --- |
| Repository | `Qdrant/bge-small-en-v1.5-onnx-Q` |
| Commit | `52398278842ec682c6f32300af41344b1c0b0bb2` |
| ONNX file | `model_optimized.onnx` |
| Dimension / pooling | 384 / CLS |
| Profile revision | `sha3-256:bf577972c34b37578aa42965fa8401d5538f4b4c007c810332e936548658c7b3` |

The installer verifies the following SHA-256 values before making any file
visible at its destination:

| File | SHA-256 |
| --- | --- |
| `config.json` | `13582bcf2effc85b7bf3d3f5532e686bc1c9ce86bb009d10f0ec33cbe92299dd` |
| `special_tokens_map.json` | `5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a` |
| `tokenizer.json` | `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66` |
| `tokenizer_config.json` | `0b29c7bfc889e53b36d9dd3e686dd4300f6525110eaa98c76a5dafceb2029f53` |
| `model_optimized.onnx` | `51f1bd0addd6e859e42c2c8021a5e5461385bb676a649f4b269aa445449f2431` |

`sbol-db util fastembed-revision` calculates the profile revision from the
five verified files. Changing a file, the tokenizer, the ONNX model, or the
profile therefore fails at startup until the profile and a new vector
generation are deliberately released together.

## Source builds and images

Fetch the exact source-build bundle once:

```sh
make model/bge-small
export SBOL_DB_BGE_SMALL_MODEL_DIR="$HOME/.cache/sbol-db/models/bge-small-en-v1.5-onnx-q-5239827"
# Point dynamic FastEmbed at a compatible host ONNX Runtime library.
export ORT_DYLIB_PATH=/absolute/path/to/libonnxruntime.so
sbol-db server
```

On macOS the filename is normally `libonnxruntime.dylib`. The source tree
does not download a host inference runtime implicitly; install a compatible
ONNX Runtime and set `ORT_DYLIB_PATH` deliberately. The published Linux image
already includes the pinned runtime and model bundle, which is the supported
zero-configuration deployment path.

The published image needs neither that mount nor the environment variable:

```sh
docker run --rm ghcr.io/marpaia/sbol-db:<version> server
```

For a custom composition root, an operator may still mount and configure a
different local FastEmbed bundle. That is a distinct embedding profile and
requires a separate rebuild; it cannot masquerade as the built-in profile.

## Relevance release gate

[`sbol-semantic-release-v1-corpus.json`](../crates/sbol-db-search-eval/fixtures/sbol-semantic-release-v1-corpus.json)
contains eight CC0 synthetic canonical-SBOL-text documents. Its paired suite
has manually reviewed graded relevance labels for promoters, inducible
promoters, RBSs, terminators, reporter CDSs, repressors, and plasmids. It is a
small regression gate for this exact model/profile/text projection, not a
claim about live SynBioHub relevance.

Run the exact candidate against the fixture after fetching the bundle:

```sh
SBOL_DB_BGE_SMALL_MODEL_DIR="$SBOL_DB_BGE_SMALL_MODEL_DIR" \
  cargo run -p sbol-db-search-eval --example bge_small_release_gate
```

The gate records the fixture fingerprint, per-query returned IDs, graded
nDCG, a minimum absolute candidate nDCG, paired delta against the deterministic
hashing baseline, and the fraction of per-query regressions. A model, text
projection, tokenizer, or threshold change therefore needs a reviewed suite
revision and fresh report rather than a silent quality assertion.

## SBOLTestSuite integration gate

The semantic fixture above is intentionally small and controlled. A separate
integration gate imports every importable document from the SBOL1, SBOL2,
SBOL2 best-practice, SBOL2 incomplete-compliant, SBOL3, and RDF directories of
a local [`SBOLTestSuite`](https://github.com/SynBioDex/SBOLTestSuite) checkout.
At the pinned revision this is 447 documents. The gate verifies the exact
upstream commit, tracked-worktree cleanliness, each directory's expected
import/parse-failure count, the total stored document count, and the exact
8,805-object default-vector rebuild. It deliberately imports sequentially: SQLite cannot
reliably accept the TestSuite's many concurrent write transactions.

It then imports all 31 canonical SBOL3 Turtle documents into the public graph
(one serialization per example, avoiding duplicate encodings), starts the
final image with no search configuration, waits for the full-corpus BGE
rebuild, and requires a known top-level object to rank first for each
independent semantic query.

The checked-in [integration manifest](../crates/sbol-db-search-eval/fixtures/sbol-test-suite-integration-v1.json)
contains source provenance, expected coverage, and query results—not a copied
SBOLTestSuite corpus. This keeps the external test corpus independently
versioned and avoids redistributing its documents from this repository.

Run it against an already-built image with a checkout at the pinned commit:

```sh
make container/test-sbol-test-suite \
  IMAGE=sbol-db:bge-e2e-fresh \
  SBOL_TEST_SUITE_ROOT=/absolute/path/to/SBOLTestSuite
```

It is a real-document import, projection, authorization, startup-rebuild, and
ranking integration test. The all-document sweep is a compatibility and scale
gate; the four top-ranked queries are still smoke checks, not a substitute for
a larger blinded human-relevance evaluation.
