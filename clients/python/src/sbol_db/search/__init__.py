"""Type contracts for runtime-loaded native sbol-db search plugins.

A plugin is an ordinary Python module with ``register(search)``. The object
passed as ``search`` is provided by the Rust process through PyO3; these
protocols exist only for type checking and editor completion.
"""

from __future__ import annotations

from typing import Any, Awaitable, Literal, Mapping, Optional, Protocol, Sequence, Union, overload

EmbeddingKind = Literal["query", "document"]
Normalization = Literal["none", "l2"]
DataEgress = Literal["none", "configured_remote"]
Distance = Literal["cosine", "dot", "euclidean", "manhattan", "hamming", "jaccard"]


class Embedding(Protocol):
    """The only method a Python embedding implementation must provide."""

    def embed(self, texts: Sequence[str], *, kind: EmbeddingKind) -> Sequence[Sequence[float]]:
        """Return one dense vector for each input string, preserving order."""
        ...


class VectorSearch(Protocol):
    """Authorization-scoped access to configured native vector indexes."""

    def query(
        self,
        vector: Any,
        /,
        *,
        index: Optional[str] = None,
        vector_name: Optional[str] = None,
        filter: Optional[Mapping[str, Any]] = None,
        limit: int = 50,
        cursor: Optional[str] = None,
        score_threshold: Optional[float] = None,
        parameters: Optional[Mapping[str, Any]] = None,
    ) -> Mapping[str, Any]:
        """Query the strategy's declared logical index."""
        ...


class DocumentHydrator(Protocol):
    """Authorization-scoped access to authoritative SBOL metadata."""

    def hydrate(self, document_ids: Sequence[str]) -> Sequence[Mapping[str, Any]]:
        """Hydrate IDs, omitting missing or unauthorized documents."""
        ...


class SearchContext(Protocol):
    """The same request-scoped services received by a Rust search strategy."""

    @property
    def scope(self) -> Mapping[str, Any]: ...

    @property
    def budget(self) -> Mapping[str, Any]: ...

    @property
    def vectors(self) -> VectorSearch: ...

    @property
    def documents(self) -> DocumentHydrator: ...

    def embed(
        self,
        texts: Union[str, Sequence[str]],
        /,
        *,
        kind: EmbeddingKind = "query",
    ) -> Sequence[Sequence[float]]:
        """Embed text with the strategy's declared profile."""
        ...


class Strategy(Protocol):
    """A Python implementation of the native search strategy contract."""

    def search(
        self, ctx: SearchContext, request: Mapping[str, Any]
    ) -> Union[Mapping[str, Any], Awaitable[Mapping[str, Any]]]:
        """Return a search-page mapping; identity and defaults are added by Rust."""
        ...


class SearchPlugin(Protocol):
    """Registration surface passed to a plugin module's ``register`` function."""

    def add_embedding(
        self,
        implementation: Embedding,
        /,
        *,
        id: str,
        model: str,
        revision: str,
        dimension: int,
        provider: str = "python",
        normalization: Normalization = "l2",
        data_egress: DataEgress = "none",
    ) -> None:
        """Register a Python model as a native sbol-db embedding provider."""
        ...

    @overload
    def add_strategy(
        self,
        implementation: Strategy,
        /,
        *,
        id: str,
        embedding_profile: str,
        vector_index: str,
        version: str = "1",
        display_name: Optional[str] = None,
        description: str = "Python embedding search",
        vector_name: str = "content",
        graph_payload_field: str = "graph",
        distance: Distance = "cosine",
    ) -> None: ...

    @overload
    def add_strategy(
        self,
        *,
        id: str,
        embedding_profile: str,
        vector_index: str,
        version: str = "1",
        display_name: Optional[str] = None,
        description: str = "Python embedding search",
        vector_name: str = "content",
        graph_payload_field: str = "graph",
        distance: Distance = "cosine",
    ) -> None:
        """Configure the built-in dense strategy without a Python implementation."""
        ...


__all__ = [
    "DocumentHydrator",
    "Embedding",
    "EmbeddingKind",
    "SearchContext",
    "SearchPlugin",
    "Strategy",
    "VectorSearch",
]
