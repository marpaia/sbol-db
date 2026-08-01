"""Feature-focused end-to-end tests: search, delete, overwrite, version
negotiation, and the PartShop facade, against a real server (see
``live_server``). Documents are imported as FASTA so the tests do not
hand-author SBOL RDF.

Deleting a graph removes the graph, its triples, and its derived objects, so
after a delete the object is gone from both ``get_graph`` and the search view.
"""

from __future__ import annotations

import pytest
from conftest import NAMESPACE, LiveServer, fasta

from sbol_db import BadRequestError, ImportReport, NotFoundError, PartShop, SbolDbClient

pytestmark = pytest.mark.e2e


def _import(
    client: SbolDbClient,
    display_id: str,
    document_iri: str,
    *,
    sequence: str = "ttgacggctagctcagtcctaggt",
    overwrite: int = 0,
) -> ImportReport:
    return client.create_graph(
        fasta(display_id, sequence),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=document_iri,
        overwrite=overwrite,
    )


def test_create_search_export_delete_roundtrip(client: SbolDbClient, unique: str) -> None:
    display_id = f"round_{unique}"
    doc_iri = f"{NAMESPACE}/{unique}/roundtrip"
    report = _import(client, display_id, doc_iri)

    hits = client.search(unique)
    assert any(o.display_id == display_id for o in hits)
    assert client.search_count(unique) >= 1

    target = next(o for o in hits if o.display_id == display_id)
    assert display_id in client.export_rdf(target.iri)

    client.delete_graph_by_document_iri(doc_iri)
    with pytest.raises(NotFoundError):
        client.get_graph(report.graph_id)
    assert client.search_count(unique) == 0


def test_search_empty_query_is_rejected(client: SbolDbClient) -> None:
    with pytest.raises(BadRequestError):
        client.search("   ")


def test_export_downgrades_to_sbol2(client: SbolDbClient, unique: str) -> None:
    doc_iri = f"{NAMESPACE}/{unique}/version"
    _import(client, f"ver_{unique}", doc_iri)
    target = next(o for o in client.search(unique))

    sbol3 = client.export_rdf(target.iri, format="ntriples", version="sbol3")
    sbol2 = client.export_rdf(target.iri, format="ntriples", version="sbol2")
    assert "sbols.org/v3#" in sbol3
    assert "sbols.org/v2#" in sbol2

    client.delete_graph_by_document_iri(doc_iri)


def test_delete_unknown_document_iri_is_404(client: SbolDbClient, unique: str) -> None:
    with pytest.raises(NotFoundError):
        client.delete_graph_by_document_iri(f"{NAMESPACE}/{unique}/does-not-exist")


def test_overwrite_replace_swaps_the_graph(client: SbolDbClient, unique: str) -> None:
    doc_iri = f"{NAMESPACE}/{unique}/replace"
    first = _import(client, f"v1_{unique}", doc_iri, sequence="aaaacccc")
    second = _import(client, f"v2_{unique}", doc_iri, sequence="ggggtttt", overwrite=1)

    assert first.graph_id != second.graph_id
    with pytest.raises(NotFoundError):
        client.get_graph(first.graph_id)
    assert client.get_graph(second.graph_id).document_iri == doc_iri
    assert client.search_count(f"v2_{unique}") >= 1
    assert client.search_count(f"v1_{unique}") == 0

    client.delete_graph_by_document_iri(doc_iri)


def test_overwrite_merge_unions_documents(client: SbolDbClient, unique: str) -> None:
    doc_iri = f"{NAMESPACE}/{unique}/merge"
    first = _import(client, f"alpha_{unique}", doc_iri, sequence="aaaatttt")
    merged = _import(client, f"beta_{unique}", doc_iri, sequence="ccccgggg", overwrite=2)

    assert first.graph_id != merged.graph_id
    with pytest.raises(NotFoundError):
        client.get_graph(first.graph_id)
    assert client.search_count(f"alpha_{unique}") >= 1
    assert client.search_count(f"beta_{unique}") >= 1

    client.delete_graph_by_document_iri(doc_iri)


def test_overwrite_zero_requires_document_iri_for_replace(client: SbolDbClient, unique: str) -> None:
    with pytest.raises(BadRequestError):
        client.create_graph(fasta(f"noiri_{unique}"), format="fasta", namespace=NAMESPACE, overwrite=1)


def test_partshop_facade_roundtrip(live_server: LiveServer, unique: str) -> None:
    display_id = f"shop_{unique}"
    doc_iri = f"{NAMESPACE}/{unique}/partshop"
    with PartShop(live_server.base_url) as shop:
        shop.login("dba", "dba")
        assert shop.getUser() == "dba"
        assert shop.getKey() == ""
        shop.submit(fasta(display_id), collection=doc_iri, format="fasta")

        results = shop.search(unique)
        assert any(o.display_id == display_id for o in results)
        assert shop.searchCount(unique) >= 1

        shop.remove(doc_iri)
        assert shop.searchCount(unique) == 0
        with pytest.raises(NotFoundError):
            shop.remove(doc_iri)


def test_partshop_attachments_not_implemented(live_server: LiveServer) -> None:
    with PartShop(live_server.base_url) as shop:
        with pytest.raises(NotImplementedError):
            shop.attachFile("https://ex.org/x", "/tmp/f")
        with pytest.raises(NotImplementedError):
            shop.downloadAttachment("https://ex.org/x")
