# sbol-db

A pure-Python client for the [sbol-db](https://github.com/marpaia/sbol-db) HTTP
API, plus a PartShop-compatible facade for code migrating from SynBioHub's
pysbol2 `PartShop`.

The client depends only on `httpx`. SBOL documents cross the wire as RDF
strings; structured records come back as typed dataclasses. There is no SBOL
object model and no dependency on pysbol2/pysbol3 or `sbol-rs`; SBOL2 to SBOL3
conversion happens on the server via the `version` parameter.

## Install

```bash
pip install sbol-db
# optional: rdflib, for navigating pulled RDF as a graph
pip install "sbol-db[rdf]"
```

The import name is `sbol_db`:

```python
from sbol_db import SbolDbClient, PartShop
```

## Sign in with SBOL

`SbolIdentityClient` supplies the typed OAuth/OpenID Connect pieces an
ecosystem application such as Flapjack or SBOL Canvas needs. It discovers the
provider, registers a public client, creates a state- and S256-PKCE-bound
authorization URL, validates the callback, and exchanges the code. The host
application remains responsible for opening or redirecting the browser and
storing the rotating tokens appropriately for its platform.

```python
from sbol_db import SbolIdentityClient

redirect_uri = "https://canvas.example/oauth/callback"

with SbolIdentityClient("https://sbol.io") as identity:
    provider = identity.discover()
    client = identity.register(
        "SBOL Canvas", [redirect_uri], metadata=provider
    )
    authorization = identity.begin_authorization(
        client, redirect_uri, metadata=provider
    )

    # Redirect the user's browser to authorization.url. In the callback route:
    code = authorization.code_from_callback(complete_callback_url)
    tokens = identity.exchange_code(authorization, code, metadata=provider)
    profile = identity.userinfo(tokens.access_token, metadata=provider)
```

The same helper can request `sbol:read` / `sbol:write` for an advertised API
resource, refresh with rotation, and revoke either credential. Secrets are
redacted from dataclass representations. Applications that consume
`tokens.id_token` locally must still verify its EdDSA signature through the
provider's `jwks_uri` and check issuer, audience, expiry, and the
`authorization.nonce`; calling UserInfo is the dependency-free identity path.

## Broad client

```python
from sbol_db import SbolDbClient

db = SbolDbClient("http://localhost:8888")

# import a document, search, pull, delete
db.create_graph(open("part.ttl").read(), format="turtle",
                document_iri="https://ex.org/part")
for obj in db.search("pLac", object_type="Component"):
    print(obj.display_id, obj.iri)

rdf = db.export_rdf("https://ex.org/pLac", version="sbol2")   # server converts
rows = db.sparql("SELECT ?s WHERE { ?s ?p ?o } LIMIT 10").bindings()
db.delete_graph_by_document_iri("https://ex.org/part")
```

## PartShop facade

```python
from sbol_db import PartShop

shop = PartShop("http://localhost:8888")
shop.login("dba", "dba")                       # HTTP Basic; getKey() is empty
rdf = shop.pull("https://ex.org/pLac")         # RDF text
shop.submit(rdf, collection="https://ex.org/myCollection", overwrite=1)
print(shop.searchCount("pLac"))
```

Attachments (`attachFile`, `downloadAttachment`) raise `NotImplementedError`:
they are out of scope for sbol-db.

## Development

The environment is managed by [uv](https://docs.astral.sh/uv/); the dev tools
live in a `dev` dependency group that `uv run` installs automatically.

```bash
make test    # unit tests only (uv run pytest -m "not e2e")
make e2e     # build the sbol-db server, then run the full suite against it
make lint    # isort + black + flake8 + mypy
```

`make e2e` builds a fresh `sbol-db` binary and points the test fixtures at it,
so the end-to-end tests never run against a stale build. They boot a real
server on a throwaway SQLite database (no external services). To also run them
against Postgres:

```bash
docker compose up -d postgres          # from the repo root
make e2e SBOL_DB_TEST_BACKENDS=sqlite,postgres
```

Without a discoverable binary the e2e tests skip, so `uv run pytest` still runs
the unit suite anywhere.

## Publishing

The package is built and published with uv. Bump `version` in `pyproject.toml`
and in `src/sbol_db/__init__.py`, then:

```bash
make build           # sdist + wheel into dist/
make check           # twine check dist/*
make publish         # upload to PyPI (needs a token, see below)
```

`make publish` reads a PyPI API token from the environment:

```bash
export UV_PUBLISH_TOKEN=pypi-...
make publish
```

To dry-run against TestPyPI first:

```bash
uv publish --publish-url https://test.pypi.org/legacy/ dist/*
```
