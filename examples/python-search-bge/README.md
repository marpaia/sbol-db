# Hugging Face Python search plugin

This module supplies a native sbol-db embedding provider using
`transformers.AutoModel` and a Python search strategy using the native
`SearchContext`. The strategy embeds the query, searches sbol-db's integrated
FAISS index through its ACL-scoped vector handle, hydrates authoritative SBOL
documents, and constructs the structured search response. There is no
plugin-local vector corpus.

Create the example's Python environment with uv:

```console
uv sync --frozen --project examples/python-search-bge
```

Build sbol-db against that interpreter with both runtime bridges enabled, then
start the server with its embedded worker so the FAISS index is maintained in
the same process:

```console
PYO3_PYTHON=examples/python-search-bge/.venv/bin/python \
  cargo run -p sbol-db --features python,faiss -- server \
  --search-config examples/python-search-bge/search.json
```

The embedded worker owns initial indexing, incremental updates, checkpoints,
and generation publication. It calls the same Python embedding with
`kind="document"`; query execution calls it with `kind="query"`. Mutable
reindex administration is intentionally not exposed to a request strategy.

The profile revision is pinned because document vectors and query vectors must
always inhabit the same model space. Changing the model or revision requires a
new profile ID and a new logical index generation.
