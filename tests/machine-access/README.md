# Cross-repository machine-access verification

`loopback.py` is the executable acceptance scenario shared by `sbol-db` and
the `sbol` CLI in `sbol-rs`. It launches a fresh SQLite registry on a randomly
selected loopback port and proves:

- browser-style SBOL Identity authorization with PKCE and dynamic client
  registration;
- authenticated creation and checkout of a private collection;
- `sbol.toml`, first-sync `sbol.lock`, and the `designs/` layout;
- local-only detection and an ETag-protected tracked CLI push;
- an MCP token with the exact `/mcp` audience;
- prepare/apply of a whole-collection MCP update and one-time replay rejection;
- remote-only detection and pull into a second CLI checkout; and
- continued anonymous invisibility of the private collection.

Build both debug binaries, then point the test at the `sbol-rs` checkout:

```sh
# in sbol-db
cargo build -p sbol-db

# in sbol-rs
cargo build -p sbol-cli

# back in sbol-db
python3 tests/machine-access/loopback.py --sbol-rs /path/to/sbol-rs
```

The scenario uses only the Python standard library, allocates an isolated
temporary database and credentials file, never opens a browser, and does not
contact `sbol.io`.
