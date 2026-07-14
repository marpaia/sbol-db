# sbol-db-client

A pure-Python client for the [sbol-db](https://github.com/marpaia/sbol-db) HTTP
API, plus a PartShop-compatible facade for code migrating from SynBioHub's
pysbol2 `PartShop`.

The client depends only on `httpx`. SBOL documents cross the wire as RDF
strings; structured records come back as typed dataclasses. There is no SBOL
object model and no dependency on pysbol2/pysbol3 or `sbol-rs`; SBOL2 to SBOL3
conversion happens on the server via the `version` parameter.

## Install

```bash
pip install sbol-db-client
# optional: rdflib, for navigating pulled RDF as a graph
pip install "sbol-db-client[rdf]"
```

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

```bash
python -m venv .venv && source .venv/bin/activate
pip install -e ".[dev]"
pytest                       # unit tests always run
pre-commit run --all-files
```

Integration tests boot a real `sbol-db` server on a SQLite backend. They run
when a binary is discoverable (`cargo build -p sbol-db`, or set `SBOL_DB_BIN`);
otherwise they skip.
