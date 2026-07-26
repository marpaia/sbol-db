# sbol-db-search-faiss

Production-oriented embedded FAISS indexing for sbol-db. FAISS performs dense
nearest-neighbor search; sbol-db owns document identity, payload filtering,
durable build records, checksummed immutable generations, snapshots, and
atomic activation.

The native backend is opt-in:

```toml
sbol-db-search-faiss = { version = "0.1.1", features = ["native"] }
```

It requires FAISS 1.14 with its C API and shared libraries. On macOS:

```shell
brew install faiss
```

Linux builds need Clang/libclang and can point `FAISS_DIR` at a FAISS
installation produced with `FAISS_ENABLE_C_API=ON` and
`BUILD_SHARED_LIBS=ON`. The crate generates bindings from the target's FAISS C
headers instead of relying on architecture-specific pregenerated bindings.

The standard `ghcr.io/marpaia/sbol-db` image is built with this feature and
contains pinned FAISS, OpenBLAS, and OpenMP runtime libraries. Container users
only need to supply `SBOL_DB_SEARCH_CONFIG` and persist the configured backend
path under `/var/lib/sbol-db`; no native package installation is required.
One running sbol-db process exclusively owns each local store.

The default generation profile selects exact `IDMap2,Flat` below the configured
cutoff and `IndexIVFFlat` above it. Cosine, dot-product, and Euclidean scoring
are supported. Portable sbol-db filters are compiled to ID sets and supplied
to FAISS as native search selectors before candidate ranking.
