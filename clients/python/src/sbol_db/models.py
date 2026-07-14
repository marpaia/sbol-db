"""Typed views over the JSON records sbol-db returns.

These dataclasses mirror the server's record shapes so callers get attribute
access and type checking without an SBOL library. Each ``from_json`` tolerates
extra keys, so a server that adds fields does not break an older client.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


@dataclass(frozen=True)
class SbolObject:
    """One object in the derived SBOL view (an sbol-db ``SbolObjectRecord``)."""

    iri: str
    sbol_class: str
    id: Optional[str] = None
    display_id: Optional[str] = None
    name: Optional[str] = None
    description: Optional[str] = None
    graph_id: Optional[str] = None
    types: List[str] = field(default_factory=list)
    roles: List[str] = field(default_factory=list)
    data: Dict[str, Any] = field(default_factory=dict)

    @classmethod
    def from_json(cls, d: Dict[str, Any]) -> "SbolObject":
        return cls(
            iri=d["iri"],
            sbol_class=d["sbol_class"],
            id=d.get("id"),
            display_id=d.get("display_id"),
            name=d.get("name"),
            description=d.get("description"),
            graph_id=d.get("graph_id"),
            types=list(d.get("types") or []),
            roles=list(d.get("roles") or []),
            data=dict(d.get("data") or {}),
        )


@dataclass(frozen=True)
class GraphRecord:
    """Registry metadata for one imported document graph."""

    id: str
    document_iri: Optional[str] = None
    name: Optional[str] = None
    description: Optional[str] = None
    serialization_format: Optional[str] = None
    source_uri: Optional[str] = None
    created_at: Optional[str] = None
    updated_at: Optional[str] = None

    @classmethod
    def from_json(cls, d: Dict[str, Any]) -> "GraphRecord":
        return cls(
            id=d["id"],
            document_iri=d.get("document_iri"),
            name=d.get("name"),
            description=d.get("description"),
            serialization_format=d.get("serialization_format"),
            source_uri=d.get("source_uri"),
            created_at=d.get("created_at"),
            updated_at=d.get("updated_at"),
        )


@dataclass(frozen=True)
class ImportReport:
    """The outcome of importing one document."""

    graph_id: str
    object_count: int
    triple_count: int
    validation_status: str
    validation_issue_count: int

    @classmethod
    def from_json(cls, d: Dict[str, Any]) -> "ImportReport":
        return cls(
            graph_id=str(d["graph_id"]),
            object_count=int(d["object_count"]),
            triple_count=int(d["triple_count"]),
            validation_status=str(d["validation_status"]),
            validation_issue_count=int(d["validation_issue_count"]),
        )
