"""Broad end-to-end coverage of the whole client surface against a real server:
bulk import, object listing/paging, lookup, SPARQL (SELECT and CONSTRUCT),
sequence search, neighborhood, and error mapping. Complements the
feature-focused checks in ``test_integration.py``.
"""

from __future__ import annotations

import pytest
from conftest import NAMESPACE, fasta

from sbol_db import NotFoundError, SbolDbClient

pytestmark = pytest.mark.e2e


def test_bulk_import_reports_each_document(client: SbolDbClient, unique: str) -> None:
    items = [
        {
            "body": fasta(f"bulk_{unique}_a"),
            "format": "fasta",
            "namespace": NAMESPACE,
            "document_iri": f"{NAMESPACE}/{unique}/bulk-a",
        },
        {
            "body": fasta(f"bulk_{unique}_b"),
            "format": "fasta",
            "namespace": NAMESPACE,
            "document_iri": f"{NAMESPACE}/{unique}/bulk-b",
        },
    ]
    imported, reports = client.bulk_import(items)
    assert imported == 2
    assert len(reports) == 2
    assert all(r.object_count >= 1 for r in reports)
    assert client.search_count(f"bulk_{unique}") >= 2


def test_get_object_and_lookup(client: SbolDbClient, unique: str) -> None:
    display_id = f"look_{unique}"
    report = client.create_graph(
        fasta(display_id),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/lookup",
    )
    # The FASTA import also creates a Sequence whose displayId contains this
    # substring, so match the component exactly rather than taking the first hit
    # (search ordering differs across backends).
    iri = next(o.iri for o in client.search(display_id) if o.display_id == display_id)

    fetched = client.get_object(iri)
    assert fetched.display_id == display_id
    assert fetched.graph_id == report.graph_id

    missing_iri = f"{NAMESPACE}/{unique}/missing"
    found, missing = client.lookup([iri, missing_iri])
    assert [o.iri for o in found] == [iri]
    assert missing == [missing_iri]


def test_get_unknown_object_is_404(client: SbolDbClient, unique: str) -> None:
    with pytest.raises(NotFoundError):
        client.get_object(f"{NAMESPACE}/{unique}/nope")


def test_list_and_iter_objects_scoped_to_graph(client: SbolDbClient, unique: str) -> None:
    report = client.create_graph(
        fasta(f"list_{unique}"),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/list",
    )
    # The FASTA import projects a Component and its Sequence into the graph.
    page, _cursor = client.list_objects(graph_id=report.graph_id, limit=100)
    assert len(page) >= 2
    assert any(o.display_id == f"list_{unique}" for o in page)

    # Paging one row at a time via the keyset cursor yields the same objects.
    iris_paged = {o.iri for o in client.iter_objects(graph_id=report.graph_id, page_size=1)}
    assert {o.iri for o in page} == iris_paged


def test_search_object_type_filter_and_pagination(client: SbolDbClient, unique: str) -> None:
    client.create_graph(
        fasta(f"filt_{unique}"),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/filter",
    )
    obj = next(o for o in client.search(f"filt_{unique}"))

    # Filtering by the object's own class returns it; a bogus class does not.
    typed = client.search(f"filt_{unique}", object_type=obj.sbol_class)
    assert any(o.iri == obj.iri for o in typed)
    assert client.search_count(f"filt_{unique}", object_type="NoSuchClass") == 0

    # limit/offset bound the page while total reflects the full match set.
    first_page, total = client.search_page(unique, limit=1, offset=0)
    assert total >= 1
    assert len(first_page) <= 1


def test_sparql_select_and_construct(client: SbolDbClient, unique: str) -> None:
    display_id = f"sparql_{unique}"
    client.create_graph(
        fasta(display_id),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/sparql",
    )
    iri = next(o.iri for o in client.search(display_id))

    result = client.sparql(f"SELECT ?p ?o WHERE {{ <{iri}> ?p ?o }}")
    rows = result.bindings()
    assert rows
    assert all("p" in row for row in rows)

    construct = client.sparql(f"CONSTRUCT {{ <{iri}> ?p ?o }} WHERE {{ <{iri}> ?p ?o }}")
    assert display_id in construct.text


def test_sequence_search_finds_imported_sequence(client: SbolDbClient, unique: str) -> None:
    client.create_graph(
        fasta(f"seq_{unique}", "ttgacggctagctcagtcctaggtACTAGT".lower()),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/seq",
    )
    matches = client.sequence_search("ttgacggctagct")
    assert isinstance(matches, list)
    assert matches, "expected the imported sequence to match its own subsequence"


def test_neighborhood_returns_graph(client: SbolDbClient, unique: str) -> None:
    client.create_graph(
        fasta(f"hood_{unique}"),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=f"{NAMESPACE}/{unique}/hood",
    )
    iri = next(o.iri for o in client.search(f"hood_{unique}"))
    result = client.neighborhood(iri)
    assert isinstance(result, dict)
    assert iri in repr(result)


def test_health_and_ready(client: SbolDbClient) -> None:
    assert client.healthz() is True
    assert client.readyz().get("status") == "ready"
