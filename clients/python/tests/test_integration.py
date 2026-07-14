"""End-to-end tests against a real sbol-db server (see ``live_server``).

Documents are imported as FASTA so the tests do not hand-author SBOL RDF; the
server's importer projects them into the SBOL3 object view the client reads.

Delete is graph-granular: deleting a graph removes the graph and its triples,
so ``get_graph`` then 404s. The derived object rows are reclaimed by
reprojection rather than synchronously, so these tests assert on graph
deletion, not on the search index emptying.
"""

from __future__ import annotations

import pytest

from sbol_db import BadRequestError, ImportReport, NotFoundError, PartShop, SbolDbClient

NAMESPACE = "https://sbol-db.test/it"


def _fasta(display_id: str, sequence: str) -> str:
    return f">{display_id} integration fixture\n{sequence}\n"


def _import(
    client: SbolDbClient,
    display_id: str,
    sequence: str,
    document_iri: str,
    overwrite: int = 0,
) -> ImportReport:
    return client.create_graph(
        _fasta(display_id, sequence),
        format="fasta",
        namespace=NAMESPACE,
        document_iri=document_iri,
        overwrite=overwrite,
    )


@pytest.fixture()
def client(live_server: str) -> SbolDbClient:
    return SbolDbClient(live_server)


def test_create_search_export_delete_roundtrip(client: SbolDbClient) -> None:
    doc_iri = f"{NAMESPACE}/roundtrip"
    report = _import(client, "pLacRoundtrip", "ttgacggctagctcagtcctaggt", doc_iri)

    hits = client.search("pLacRoundtrip")
    assert any(o.display_id == "pLacRoundtrip" for o in hits)
    assert client.search_count("pLacRoundtrip") >= 1

    target = next(o for o in hits if o.display_id == "pLacRoundtrip")
    rdf = client.export_rdf(target.iri)
    assert "pLacRoundtrip" in rdf

    client.delete_graph_by_document_iri(doc_iri)
    with pytest.raises(NotFoundError):
        client.get_graph(report.graph_id)
    # Deleting the graph also drops its derived objects from the search view.
    assert client.search_count("pLacRoundtrip") == 0


def test_export_downgrades_to_sbol2(client: SbolDbClient) -> None:
    doc_iri = f"{NAMESPACE}/version"
    _import(client, "pLacVersion", "ttgacggctagctcagt", doc_iri)
    target = next(o for o in client.search("pLacVersion") if o.display_id == "pLacVersion")

    sbol3 = client.export_rdf(target.iri, format="ntriples", version="sbol3")
    sbol2 = client.export_rdf(target.iri, format="ntriples", version="sbol2")
    assert "sbols.org/v3#" in sbol3
    assert "sbols.org/v2#" in sbol2

    client.delete_graph_by_document_iri(doc_iri)


def test_search_empty_query_is_rejected(client: SbolDbClient) -> None:
    with pytest.raises(BadRequestError):
        client.search("   ")


def test_delete_unknown_document_iri_is_404(client: SbolDbClient) -> None:
    with pytest.raises(NotFoundError):
        client.delete_graph_by_document_iri(f"{NAMESPACE}/does-not-exist")


def test_overwrite_replace_swaps_the_graph(client: SbolDbClient) -> None:
    doc_iri = f"{NAMESPACE}/replace"
    first = _import(client, "seqReplaceV1", "aaaacccc", doc_iri)
    second = _import(client, "seqReplaceV2", "ggggtttt", doc_iri, overwrite=1)

    # Replace drops the prior graph and installs the new one under the same IRI.
    assert first.graph_id != second.graph_id
    with pytest.raises(NotFoundError):
        client.get_graph(first.graph_id)
    assert client.get_graph(second.graph_id).document_iri == doc_iri
    assert client.search_count("seqReplaceV2") >= 1
    assert client.search_count("seqReplaceV1") == 0

    client.delete_graph_by_document_iri(doc_iri)


def test_overwrite_merge_unions_documents(client: SbolDbClient) -> None:
    doc_iri = f"{NAMESPACE}/merge"
    first = _import(client, "mergeAlpha", "aaaatttt", doc_iri)
    merged = _import(client, "mergeBeta", "ccccgggg", doc_iri, overwrite=2)

    # The merged graph replaces the original but carries objects from both.
    assert first.graph_id != merged.graph_id
    with pytest.raises(NotFoundError):
        client.get_graph(first.graph_id)
    assert client.search_count("mergeAlpha") >= 1
    assert client.search_count("mergeBeta") >= 1

    client.delete_graph_by_document_iri(doc_iri)


def test_partshop_facade_roundtrip(live_server: str) -> None:
    shop = PartShop(live_server)
    shop.login("dba", "dba")
    doc_iri = f"{NAMESPACE}/partshop"
    shop.submit(_fasta("shopPart", "acgtacgtacgt"), collection=doc_iri, format="fasta")

    results = shop.search("shopPart")
    assert any(o.display_id == "shopPart" for o in results)
    assert shop.searchCount("shopPart") >= 1

    # remove() deletes the graph and its objects; removing it again 404s.
    shop.remove(doc_iri)
    assert shop.searchCount("shopPart") == 0
    with pytest.raises(NotFoundError):
        shop.remove(doc_iri)
