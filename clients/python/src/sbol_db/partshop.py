"""A PartShop-compatible facade over :class:`~sbol_db.client.SbolDbClient`.

The method names and arguments mirror pysbol2's ``sbol2.partshop.PartShop`` so
existing scripts port with minimal change. The return types differ: because the
client carries no SBOL object model, ``pull`` returns RDF text and ``search``
returns :class:`~sbol_db.models.SbolObject` records rather than populated
documents. Hence "roughly compatible", not drop-in.
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional, Union

from .client import SbolDbClient
from .models import ImportReport, SbolObject


class PartShop:
    """A pysbol2-shaped view of an sbol-db server."""

    def __init__(self, url: str, spoofed_url: str = "") -> None:
        self._url = url.rstrip("/")
        self._spoofed_url = spoofed_url
        self._user = ""
        self._client = SbolDbClient(self._url)

    # -- auth -------------------------------------------------------------

    def login(self, user_id: str, password: str = "") -> int:
        """Authenticate for subsequent writes.

        sbol-db uses HTTP Basic auth rather than a token, so this stores the
        credentials on the client and returns 200 to match PartShop's contract.
        """
        self._user = user_id
        self._client.close()
        self._client = SbolDbClient(self._url, user=user_id, password=password)
        return 200

    def getURL(self) -> str:
        return self._url

    def getSpoofedURL(self) -> str:
        return self._spoofed_url

    def getUser(self) -> str:
        return self._user

    def getKey(self) -> str:
        """Always empty: sbol-db authenticates with HTTP Basic, not a token."""
        return ""

    # -- documents --------------------------------------------------------

    def pull(
        self,
        uris: Union[str, List[str]],
        doc: Optional[Any] = None,
        recursive: bool = True,
        version: str = "sbol3",
        format: str = "turtle",
    ) -> str:
        """Retrieve one or more objects as RDF.

        Returns the RDF text (the reference closure of each URI when
        ``recursive``). If ``doc`` is a filesystem path or a writable file
        object, the RDF is also written there.
        """
        iris = [uris] if isinstance(uris, str) else list(uris)
        parts = [self._client.export_rdf(iri, format=format, recursive=recursive, version=version) for iri in iris]
        rdf = "\n".join(parts)
        if doc is not None:
            _write_to(doc, rdf)
        return rdf

    def submit(
        self,
        rdf: str,
        collection: str = "",
        overwrite: int = 0,
        version: str = "sbol3",
        format: str = "turtle",
        name: Optional[str] = None,
        description: Optional[str] = None,
    ) -> ImportReport:
        """Upload an RDF document.

        ``collection`` maps to the document IRI that names the graph;
        ``overwrite`` is 0 (fail), 1 (replace), or 2 (merge).
        """
        return self._client.create_graph(
            rdf,
            format=format,
            document_iri=collection or None,
            name=name,
            description=description,
            overwrite=overwrite,
            version=version,
        )

    def remove(self, uri: str) -> None:
        """Delete the document graph identified by ``uri`` (its document IRI)."""
        self._client.delete_graph_by_document_iri(uri)

    # -- query ------------------------------------------------------------

    def sparqlQuery(self, query: str) -> Dict[str, Any]:
        """Run a SPARQL query, returning SPARQL-results JSON."""
        return self._client.sparql(query).json()

    def search(
        self,
        search_text: str,
        object_type: Optional[str] = None,
        property_uri: Optional[str] = None,
        offset: int = 0,
        limit: int = 25,
    ) -> List[SbolObject]:
        """Text search over the repository."""
        return self._client.search(
            search_text,
            object_type=object_type,
            property_uri=property_uri,
            offset=offset,
            limit=limit,
        )

    def searchCount(
        self,
        search_text: str,
        object_type: Optional[str] = None,
        property_uri: Optional[str] = None,
    ) -> int:
        """Count of objects matching a search."""
        return self._client.search_count(search_text, object_type=object_type, property_uri=property_uri)

    def count(self) -> int:
        """Total number of distinct subjects in the repository."""
        rows = self._client.sparql("SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ?p ?o }").bindings()
        return int(rows[0]["n"]) if rows else 0

    # -- unsupported (attachments are out of scope for sbol-db) -----------

    def attachFile(self, top_level_uri: str, filepath: str) -> None:
        raise NotImplementedError("attachments are out of scope for sbol-db")

    def downloadAttachment(self, attachment_uri: str, filepath: str = ".") -> None:
        raise NotImplementedError("attachments are out of scope for sbol-db")

    # -- lifecycle --------------------------------------------------------

    def close(self) -> None:
        self._client.close()

    def __enter__(self) -> "PartShop":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()


def _write_to(doc: Any, rdf: str) -> None:
    """Write ``rdf`` to a filesystem path or a writable file object."""
    if hasattr(doc, "write"):
        doc.write(rdf)
    elif isinstance(doc, (str, os.PathLike)):
        with open(doc, "w", encoding="utf-8") as handle:
            handle.write(rdf)
    else:
        raise TypeError("doc must be a path or a writable file object")
