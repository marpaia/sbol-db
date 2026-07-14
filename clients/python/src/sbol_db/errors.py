"""Exceptions raised by the sbol-db client.

The server reports failures as an RFC 7807-style JSON body,
``{"type", "title", "status", "detail"}``, where ``type`` is a stable machine
readable kind. :func:`error_for_response` maps that kind to a specific
exception subclass so callers can branch on the failure mode without parsing
status codes.
"""

from __future__ import annotations

from typing import Optional

import httpx


class SbolDbError(Exception):
    """Base class for every error returned by an sbol-db server."""

    def __init__(self, detail: str, *, status: Optional[int] = None, kind: Optional[str] = None) -> None:
        super().__init__(detail)
        self.detail = detail
        self.status = status
        self.kind = kind


class BadRequestError(SbolDbError):
    """The request was malformed or rejected as invalid input (HTTP 400)."""


class NotFoundError(SbolDbError):
    """The addressed object, graph, or route does not exist (HTTP 404)."""


class SparqlError(SbolDbError):
    """A SPARQL query failed to parse or evaluate."""


class TimeoutError_(SbolDbError):
    """The server timed out handling the request (HTTP 504)."""


class BackendUnsupportedError(SbolDbError):
    """The active storage backend does not support the requested feature (HTTP 501)."""


# Server "type" kind -> exception subclass. Kinds absent here fall back to the
# base SbolDbError.
_KIND_TO_ERROR = {
    "bad_request": BadRequestError,
    "invalid_input": BadRequestError,
    "parse_error": BadRequestError,
    "invalid_iri": BadRequestError,
    "not_found": NotFoundError,
    "sparql_parse_error": SparqlError,
    "sparql_update_not_allowed": SparqlError,
    "sparql_unsupported_format": SparqlError,
    "sparql_unsupported": SparqlError,
    "sparql_error": SparqlError,
    "timeout": TimeoutError_,
    "backend_unsupported": BackendUnsupportedError,
}


def error_for_response(response: httpx.Response) -> SbolDbError:
    """Build the most specific :class:`SbolDbError` for a failed response."""
    kind: Optional[str] = None
    detail = response.text
    try:
        body = response.json()
    except ValueError:
        body = None
    if isinstance(body, dict):
        kind = body.get("type")
        detail = body.get("detail") or detail
    cls = _KIND_TO_ERROR.get(kind or "", SbolDbError)
    return cls(detail, status=response.status_code, kind=kind)
