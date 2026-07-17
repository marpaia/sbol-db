"""The Elasticsearch-independent read / metadata / SPARQL / download subset.

These cases cover the V1 endpoints that answer straight from the triplestore:
object metadata, type counts, root collections, raw SPARQL, and the RDF/GFF
download closure. They deliberately exclude the ranked free-text `/search`
family, which classic SynBioHub serves out of SBOLExplorer + Elasticsearch (that
service OOMs under emulation), so this subset is the part of the differential
matrix that can run when the reference stack is only partially healthy.

The subset keys on the corpus in `fixtures/smoke-corpus.nt`: one public
Collection (`smoke_collection`) whose sole member is a ComponentDefinition
(`pSmoke`) carrying a Sequence. Both the self-consistency smoke and the live
differential seed that corpus and drive these cases.
"""

from __future__ import annotations

from pathlib import Path
from typing import List

from conformance import Case

CORPUS_PATH = Path(__file__).resolve().parent / "fixtures" / "smoke-corpus.nt"

# The seeded object and collection, addressed by the V1 path grammar
# `/public/<collectionId>/<displayId>/<version>`.
OBJECT_PATH = "/public/smoke/pSmoke/1"
COLLECTION_PATH = "/public/smoke/smoke_collection/1"

_JSON = {"Accept": "application/json"}
_SPARQL_JSON = {"Accept": "application/sparql-results+json"}


def load_corpus() -> str:
    """The N-Triples corpus every subset run seeds into the public graph."""
    return CORPUS_PATH.read_text(encoding="utf-8")


def read_subset_cases() -> List[Case]:
    """The read/metadata/SPARQL/download cases, each tagged with the comparator
    category the driver uses to diff reference against subject."""
    return [
        # Metadata is GetTopLevelMetadata.sparql: a SPARQL-results document.
        Case("object-metadata", "sparql", path=f"{OBJECT_PATH}/metadata", headers=_JSON),
        Case(
            "collection-metadata",
            "sparql",
            path=f"{COLLECTION_PATH}/metadata",
            headers=_JSON,
        ),
        # Type count and root collections are SPARQL-backed result sets.
        Case("componentdefinition-count", "sparql", path="/ComponentDefinition/count", headers=_JSON),
        Case("collection-count", "sparql", path="/Collection/count", headers=_JSON),
        Case("root-collections", "sparql", path="/rootCollections", headers=_JSON),
        # Raw SPARQL over the shared engine.
        Case(
            "sparql-ask",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": "ASK {}"},
            headers=_SPARQL_JSON,
        ),
        Case(
            "sparql-count",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": ("SELECT (COUNT(*) AS ?c) " "FROM <http://synbiohub.org/public> WHERE { ?s ?p ?o }")},
            headers=_SPARQL_JSON,
        ),
        # Download closure: RDF/XML compared semantically, GFF3 as a record set.
        Case("sbol-download", "sbol", path=f"{OBJECT_PATH}/sbol"),
        Case("gff-download", "gff", path=f"{OBJECT_PATH}/gff"),
    ]
