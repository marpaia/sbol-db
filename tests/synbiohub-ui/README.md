# Run the SynBioHub UI on sbol-db

One command runs the real SynBioHub frontend (`sbh3frontend`) against sbol-db as
the complete v1 backend.

```
./run.sh up            # build sbol-db, start it + the UI, print the URLs
./run.sh up --fresh    # start from an empty instance (first-launch setup wizard)
./run.sh up --seed P   # start from a copy of the SQLite store at path P
./run.sh down          # stop the UI and sbol-db
./run.sh logs          # tail sbol-db and UI logs
./run.sh status        # show what is running
```

Then open **http://localhost:3333**.

## How it fits together

- **sbol-db** runs as a host process on `:18903` (rebuilds are fast, so it is not
  containerized). `run.sh up` rebuilds it every time.
- **The frontend** runs in Docker via `docker-compose.yml`. Its `backend` (read
  by the browser) points at `localhost:18903`; its `backendSS` (read by the
  frontend's server) reaches the host through `host.docker.internal`.
- **State** lives in `./data` (gitignored). The SQLite store persists across
  restarts, so a plain `up` resumes the same instance. `--fresh` resets it;
  `--seed P` copies another store (e.g. the conformance corpus at
  `/tmp/sbol-db-subject.sqlite`) over it.

## First run

A fresh instance has no administrator, so the UI shows its setup wizard: open
the URL, fill it in, and it creates the first admin. After that, `up` resumes the
provisioned instance.

Seeded with the conformance corpus, the admin is `testuser` / `test`.

## Overrides

`SBOLDB_PORT` and `UI_PORT` change the ports (both `run.sh` and compose read
them).

## Troubleshooting

If a page misbehaves right after switching stores (e.g. a request to an old
port, or a stale instance name), the browser's `localStorage` is holding state
from the previous instance. Clear site data for `localhost:3333` and reload.

## Related

- `../synbiohub-ui-diff` diffs sbol-db against classic on the endpoints the UI
  calls; it expects the stack this script starts.
