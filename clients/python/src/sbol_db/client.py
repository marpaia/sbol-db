"""The broad sbol-db client.

:class:`SbolDbClient` wraps sbol-db's native typed REST API: objects, graphs,
text and sequence search, SPARQL, ontology, and jobs. SBOL documents cross the
wire as RDF strings; structured records come back as the dataclasses in
:mod:`sbol_db.models`. The :class:`~sbol_db.partshop.PartShop` facade is a thin
layer over this class.
"""

from __future__ import annotations

from typing import Any, Dict, Iterator, List, Optional, Tuple

from .models import GraphRecord, ImportReport, SbolObject
from .sparql import SparqlResult
from .transport import Transport

# Query-parameter format token -> request body MIME type. The server takes the
# format from the query parameter; the content type is sent for correctness.
_RDF_MIME = {
    "turtle": "text/turtle",
    "ntriples": "application/n-triples",
    "rdfxml": "application/rdf+xml",
    "jsonld": "application/ld+json",
}


class SbolDbClient:
    """A synchronous client for one sbol-db server."""

    def __init__(
        self,
        base_url: str,
        *,
        user: Optional[str] = None,
        password: str = "",
        timeout: float = 30.0,
        transport: Optional[Any] = None,
    ) -> None:
        auth = (user, password) if user is not None else None
        self._t = Transport(base_url, auth=auth, timeout=timeout, transport=transport)
        self.base_url = base_url.rstrip("/")

    def close(self) -> None:
        self._t.close()

    def __enter__(self) -> "SbolDbClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # -- health / ops -----------------------------------------------------

    def healthz(self) -> bool:
        """True when the server is up."""
        return self._t.request("GET", "/healthz").text.strip() == "ok"

    def readyz(self) -> Dict[str, Any]:
        """Readiness detail, including database connectivity."""
        return self._t.request("GET", "/readyz").json()

    # -- objects ----------------------------------------------------------

    def get_object(self, iri: str) -> SbolObject:
        """Fetch one object by its IRI."""
        data = self._t.request("GET", "/objects", params={"iri": iri}).json()
        return SbolObject.from_json(data)

    def lookup(self, iris: List[str]) -> Tuple[List[SbolObject], List[str]]:
        """Batch-fetch objects by IRI, returning ``(found, missing)``."""
        body = self._t.request("POST", "/objects/lookup", json={"iris": iris}).json()
        found = [SbolObject.from_json(o) for o in body.get("found", [])]
        return found, list(body.get("missing", []))

    def list_objects(
        self,
        *,
        sbol_class: Optional[str] = None,
        role: Optional[str] = None,
        graph_id: Optional[str] = None,
        after: Optional[str] = None,
        limit: int = 1000,
    ) -> Tuple[List[SbolObject], Optional[str]]:
        """One page of objects plus the keyset cursor for the next page."""
        body = self._t.request(
            "GET",
            "/objects/list",
            params={
                "sbol_class": sbol_class,
                "role": role,
                "graph_id": graph_id,
                "after": after,
                "limit": limit,
            },
        ).json()
        objects = [SbolObject.from_json(o) for o in body.get("objects", [])]
        return objects, body.get("next_cursor")

    def iter_objects(
        self,
        *,
        sbol_class: Optional[str] = None,
        role: Optional[str] = None,
        graph_id: Optional[str] = None,
        page_size: int = 1000,
    ) -> Iterator[SbolObject]:
        """Iterate every matching object, paging via the keyset cursor."""
        cursor: Optional[str] = None
        while True:
            page, cursor = self.list_objects(
                sbol_class=sbol_class,
                role=role,
                graph_id=graph_id,
                after=cursor,
                limit=page_size,
            )
            yield from page
            if cursor is None:
                return

    def export_rdf(
        self,
        iri: str,
        *,
        format: str = "turtle",
        recursive: bool = True,
        version: str = "sbol3",
    ) -> str:
        """Serialize an object to RDF.

        With ``recursive`` (the default) the object's whole reference closure is
        returned; otherwise only the object's own triples. ``version`` selects
        the SBOL vocabulary of the output (``sbol3`` or ``sbol2``); conversion
        happens on the server.
        """
        params = {"format": format, "version": version}
        if recursive:
            return self._t.request(
                "GET",
                "/objects/neighborhood.rdf",
                params={"iri": iri, **params},
            ).text
        object_id = self.get_object(iri).id
        if object_id is None:
            raise ValueError(f"object {iri} has no id to export")
        return self._t.request("GET", f"/objects/{object_id}/rdf", params=params).text

    def neighborhood(self, iri: str) -> Dict[str, Any]:
        """The reference-closure of an object as a JSON graph."""
        return self._t.request("GET", "/objects/neighborhood", params={"iri": iri}).json()

    # -- search -----------------------------------------------------------

    def search_page(
        self,
        text: str,
        *,
        object_type: Optional[str] = None,
        property_uri: Optional[str] = None,
        offset: int = 0,
        limit: int = 50,
    ) -> Tuple[List[SbolObject], int]:
        """One page of search results plus the total match count."""
        body = self._t.request(
            "GET",
            "/search",
            params={
                "q": text,
                "object_type": object_type,
                "property_uri": property_uri,
                "offset": offset,
                "limit": limit,
            },
        ).json()
        objects = [SbolObject.from_json(o) for o in body.get("objects", [])]
        return objects, int(body.get("total", len(objects)))

    def search(
        self,
        text: str,
        *,
        object_type: Optional[str] = None,
        property_uri: Optional[str] = None,
        offset: int = 0,
        limit: int = 50,
    ) -> List[SbolObject]:
        """Substring search over the object view, returning one page."""
        objects, _total = self.search_page(
            text,
            object_type=object_type,
            property_uri=property_uri,
            offset=offset,
            limit=limit,
        )
        return objects

    def search_count(
        self,
        text: str,
        *,
        object_type: Optional[str] = None,
        property_uri: Optional[str] = None,
    ) -> int:
        """Total number of objects matching a search, fetching no rows."""
        _objects, total = self.search_page(
            text,
            object_type=object_type,
            property_uri=property_uri,
            limit=0,
        )
        return total

    def sequence_search(
        self,
        pattern: str,
        *,
        max_hits: int = 1024,
        forward_only: bool = False,
    ) -> List[Dict[str, Any]]:
        """Search sequences for a nucleotide subsequence."""
        return self._t.request(
            "GET",
            "/sequences/search",
            params={"pattern": pattern, "max_hits": max_hits, "forward_only": forward_only},
        ).json()

    # -- graphs -----------------------------------------------------------

    def create_graph(
        self,
        rdf: str,
        *,
        format: str = "turtle",
        document_iri: Optional[str] = None,
        name: Optional[str] = None,
        description: Optional[str] = None,
        namespace: Optional[str] = None,
        source_uri: Optional[str] = None,
        created_by: Optional[str] = None,
        overwrite: int = 0,
        version: Optional[str] = None,
    ) -> ImportReport:
        """Import an RDF document as a new graph.

        ``overwrite`` controls collisions on ``document_iri``: 0 fails, 1
        replaces, 2 merges. ``version`` declares the SBOL vocabulary of ``rdf``
        (``sbol2`` is upgraded to SBOL3 on the server).
        """
        headers = {"content-type": _RDF_MIME.get(format, "text/plain")}
        data = self._t.request(
            "POST",
            "/graphs",
            params={
                "format": format,
                "document_iri": document_iri,
                "name": name,
                "description": description,
                "namespace": namespace,
                "source_uri": source_uri,
                "created_by": created_by,
                "overwrite": overwrite,
                "version": version,
            },
            content=rdf.encode("utf-8"),
            headers=headers,
        ).json()
        return ImportReport.from_json(data)

    def bulk_import(self, items: List[Dict[str, Any]]) -> Tuple[int, List[ImportReport]]:
        """Atomically import a batch of documents.

        Each item is ``{"body", "format", ...}`` matching the server's bulk
        schema. Returns ``(imported_count, reports)``.
        """
        body = self._t.request("POST", "/graphs/bulk", json={"graphs": items}).json()
        reports = [ImportReport.from_json(r) for r in body.get("reports", [])]
        return int(body.get("imported", len(reports))), reports

    def get_graph(self, graph_id: str) -> GraphRecord:
        """Fetch one document graph's registry metadata."""
        return GraphRecord.from_json(self._t.request("GET", f"/graphs/{graph_id}").json())

    def delete_graph(self, graph_id: str) -> None:
        """Delete one document graph by its surrogate id."""
        self._t.request("DELETE", f"/graphs/{graph_id}")

    def delete_graph_by_document_iri(self, document_iri: str) -> None:
        """Delete the document graph carrying ``document_iri``."""
        self._t.request("DELETE", "/graphs", params={"document_iri": document_iri})

    # -- query ------------------------------------------------------------

    def sparql(
        self,
        query: str,
        *,
        default_graph: Optional[str] = None,
        format: Optional[str] = None,
    ) -> SparqlResult:
        """Run a SPARQL 1.1 read query (SELECT/ASK/CONSTRUCT/DESCRIBE).

        The query goes in a form-encoded body (the server reads POST bodies, not
        the URL query string), alongside the optional `default-graph-uri` scope.
        With no `format`, the server picks each query form's natural result type
        (JSON for SELECT/ASK, RDF for CONSTRUCT/DESCRIBE); pass `format` to
        override.
        """
        data: Dict[str, Any] = {"query": query}
        if default_graph is not None:
            data["default-graph-uri"] = default_graph
        if format is not None:
            data["format"] = format
        return SparqlResult(self._t.request("POST", "/sparql", data=data))

    # -- ontology ---------------------------------------------------------

    def ontology_term(self, iri: str) -> Dict[str, Any]:
        """Look up one ontology term by IRI."""
        return self._t.request("GET", "/ontology/term", params={"iri": iri}).json()

    def ontology_descendants(self, iri: str) -> List[Dict[str, Any]]:
        """The transitive descendants of an ontology term."""
        return self._t.request("GET", "/ontology/descendants", params={"iri": iri}).json()

    # -- jobs -------------------------------------------------------------

    def enqueue_job(self, spec: Dict[str, Any]) -> Dict[str, Any]:
        """Enqueue a background job."""
        return self._t.request("POST", "/jobs", json=spec).json()

    def get_job(self, job_id: str) -> Dict[str, Any]:
        """Fetch one job's current state."""
        return self._t.request("GET", f"/jobs/{job_id}").json()

    def cancel_job(self, job_id: str) -> None:
        """Request cancellation of a job."""
        self._t.request("POST", f"/jobs/{job_id}/cancel")

    def job_logs(self, job_id: str) -> List[Dict[str, Any]]:
        """Fetch a job's log lines."""
        return self._t.request("GET", f"/jobs/{job_id}/logs").json()
