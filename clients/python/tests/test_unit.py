"""Unit tests for the client, driven by an in-process mock transport.

These need no server: an ``httpx.MockTransport`` answers requests and lets us
assert on the requests the client makes and how it maps responses.
"""

from __future__ import annotations

import json
from typing import Callable, List
from urllib.parse import parse_qs

import httpx
import pytest

from sbol_db import (
    BackendUnsupportedError,
    BadRequestError,
    NotFoundError,
    PartShop,
    SbolDbClient,
    SbolDbError,
)


def make_client(handler: Callable[[httpx.Request], httpx.Response], **kwargs: object) -> SbolDbClient:
    return SbolDbClient("http://sbol-db.test", transport=httpx.MockTransport(handler), **kwargs)


def _object(iri: str, sbol_class: str = "Component") -> dict:
    return {"iri": iri, "sbol_class": sbol_class, "id": "obj-" + iri[-1], "types": [], "roles": []}


# -- error mapping --------------------------------------------------------


@pytest.mark.parametrize(
    "status,kind,expected",
    [
        (404, "not_found", NotFoundError),
        (400, "invalid_input", BadRequestError),
        (400, "bad_request", BadRequestError),
        (501, "backend_unsupported", BackendUnsupportedError),
        (500, "internal_error", SbolDbError),
    ],
)
def test_error_kinds_map_to_exceptions(status: int, kind: str, expected: type) -> None:
    def handler(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(status, json={"type": kind, "title": kind, "status": status, "detail": "boom"})

    client = make_client(handler)
    with pytest.raises(expected) as exc:
        client.get_object("https://ex.org/x")
    assert exc.value.status == status
    assert exc.value.detail == "boom"


def test_non_json_error_falls_back_to_base_error() -> None:
    def handler(_request: httpx.Request) -> httpx.Response:
        return httpx.Response(502, text="bad gateway")

    with pytest.raises(SbolDbError) as exc:
        make_client(handler).healthz()
    assert exc.value.status == 502
    assert exc.value.kind is None


# -- request shaping ------------------------------------------------------


def test_none_params_are_dropped() -> None:
    seen: List[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request.url.query.decode())
        return httpx.Response(200, json={"objects": [], "next_cursor": None})

    make_client(handler).list_objects(limit=10)
    query = seen[0]
    assert "limit=10" in query
    assert "sbol_class" not in query
    assert "role" not in query


def test_search_count_requests_zero_limit_and_reads_total() -> None:
    captured = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured["params"] = parse_qs(request.url.query.decode())
        return httpx.Response(200, json={"objects": [], "total": 42, "offset": 0, "limit": 0})

    total = make_client(handler).search_count("pLac", object_type="Component")
    assert total == 42
    assert captured["params"]["limit"] == ["0"]
    assert captured["params"]["object_type"] == ["Component"]


def test_iter_objects_follows_cursor_until_exhausted() -> None:
    pages = {
        None: {"objects": [_object("https://ex.org/a")], "next_cursor": "https://ex.org/a"},
        "https://ex.org/a": {"objects": [_object("https://ex.org/b")], "next_cursor": None},
    }

    def handler(request: httpx.Request) -> httpx.Response:
        after = parse_qs(request.url.query.decode()).get("after", [None])[0]
        return httpx.Response(200, json=pages[after])

    iris = [o.iri for o in make_client(handler).iter_objects()]
    assert iris == ["https://ex.org/a", "https://ex.org/b"]


def test_lookup_splits_found_and_missing() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        assert body["iris"] == ["a", "b"]
        return httpx.Response(200, json={"found": [_object("a")], "missing": ["b"]})

    found, missing = make_client(handler).lookup(["a", "b"])
    assert [o.iri for o in found] == ["a"]
    assert missing == ["b"]


# -- PartShop facade ------------------------------------------------------


def test_partshop_pull_concatenates_and_uses_neighborhood() -> None:
    paths: List[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        paths.append(request.url.path)
        return httpx.Response(200, text=f"# rdf for {request.url.params.get('iri')}")

    shop = PartShop("http://sbol-db.test")
    shop._client = make_client(handler)
    rdf = shop.pull(["https://ex.org/a", "https://ex.org/b"])
    assert paths == ["/objects/neighborhood.rdf", "/objects/neighborhood.rdf"]
    assert "https://ex.org/a" in rdf and "https://ex.org/b" in rdf


def test_partshop_remove_deletes_by_document_iri() -> None:
    captured = {}

    def handler(request: httpx.Request) -> httpx.Response:
        captured["method"] = request.method
        captured["path"] = request.url.path
        captured["doc"] = request.url.params.get("document_iri")
        return httpx.Response(204)

    shop = PartShop("http://sbol-db.test")
    shop._client = make_client(handler)
    shop.remove("https://ex.org/mine")
    assert captured == {"method": "DELETE", "path": "/graphs", "doc": "https://ex.org/mine"}


def test_partshop_attachments_are_not_implemented() -> None:
    shop = PartShop("http://sbol-db.test")
    with pytest.raises(NotImplementedError):
        shop.attachFile("https://ex.org/x", "/tmp/f")
    with pytest.raises(NotImplementedError):
        shop.downloadAttachment("https://ex.org/x")


def test_login_reports_basic_auth_has_no_key() -> None:
    shop = PartShop("http://sbol-db.test")
    assert shop.login("alice", "secret") == 200
    assert shop.getUser() == "alice"
    assert shop.getKey() == ""
