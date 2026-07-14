"""Helpers for working with SPARQL responses.

sbol-db serves SELECT/ASK as SPARQL-results JSON and CONSTRUCT/DESCRIBE as RDF
in the negotiated format. :class:`SparqlResult` carries the raw body plus its
content type so callers can pick the right accessor.
"""

from __future__ import annotations

from typing import Any, Dict, List

import httpx


class SparqlResult:
    """The outcome of a SPARQL query, aware of its own content type."""

    def __init__(self, response: httpx.Response) -> None:
        self.content_type = response.headers.get("content-type", "").split(";")[0].strip()
        self.truncated = response.headers.get("x-sbol-db-truncated") == "true"
        self._text = response.text
        self._response = response

    @property
    def text(self) -> str:
        """The raw response body (RDF for CONSTRUCT/DESCRIBE, JSON otherwise)."""
        return self._text

    def json(self) -> Dict[str, Any]:
        """Parse the body as SPARQL-results JSON (SELECT/ASK)."""
        return self._response.json()

    def bindings(self) -> List[Dict[str, str]]:
        """Return SELECT rows as a list of ``{variable: value}`` dicts.

        Only the lexical value of each binding is kept; use :meth:`json` for the
        full typed form (datatype, language tag, term type).
        """
        data = self.json()
        rows = data.get("results", {}).get("bindings", [])
        return [{var: cell.get("value", "") for var, cell in row.items()} for row in rows]
