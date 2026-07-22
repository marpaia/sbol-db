"""Offline unit tests for the driver's fan-out and comparison wiring.

No live stack: fake targets return canned responses so run_case's fan-out,
category dispatch, and status-mismatch handling are all exercised in-memory.
"""

from __future__ import annotations

from typing import Any, Dict

from conformance import Case, compare_responses, run_case


class FakeResponse:
    def __init__(self, status_code: int, text: str = "", payload: Any = None, content: bytes = b""):
        self.status_code = status_code
        self.text = text
        self._payload = payload
        self.content = content

    def json(self) -> Any:
        return self._payload


class FakeTarget:
    """Duck-typed stand-in for conformance.Target that replays canned responses
    keyed by request path."""

    def __init__(self, name: str, responses: Dict[str, FakeResponse], is_reference: bool = False):
        self.name = name
        self._responses = responses
        self.is_reference = is_reference
        self.token = None

    def request(self, method: str, path: str, **kwargs: Any) -> FakeResponse:
        return self._responses[path]

    def logout(self) -> None:
        self.token = None


def _sparql(bindings):
    return {"head": {"vars": ["s"]}, "results": {"bindings": bindings}}


def test_run_case_fans_out_to_every_subject():
    ref_payload = _sparql([{"s": {"type": "uri", "value": "http://a"}}])
    ref = FakeTarget("reference", {"/sparql": FakeResponse(200, payload=ref_payload)}, is_reference=True)
    subjects = [
        FakeTarget(f"subject-{name}", {"/sparql": FakeResponse(200, payload=ref_payload)})
        for name in ("postgres", "sqlite", "rocksdb")
    ]
    case = Case(name="ask", category="sparql", method="POST", path="/sparql")
    result = run_case(case, ref, subjects)
    assert result.passed
    assert [r.target for r in result.results] == ["subject-postgres", "subject-sqlite", "subject-rocksdb"]


def test_run_case_flags_divergent_subject():
    ref_payload = _sparql([{"s": {"type": "uri", "value": "http://a"}}])
    bad_payload = _sparql([{"s": {"type": "uri", "value": "http://DIFFERENT"}}])
    ref = FakeTarget("reference", {"/sparql": FakeResponse(200, payload=ref_payload)})
    subjects = [
        FakeTarget("subject-ok", {"/sparql": FakeResponse(200, payload=ref_payload)}),
        FakeTarget("subject-bad", {"/sparql": FakeResponse(200, payload=bad_payload)}),
    ]
    case = Case(name="ask", category="sparql", method="POST", path="/sparql")
    result = run_case(case, ref, subjects)
    assert not result.passed
    by_name = {r.target: r for r in result.results}
    assert by_name["subject-ok"].equal
    assert not by_name["subject-bad"].equal


def test_status_mismatch_is_a_diff():
    ref = FakeResponse(200, text="<html></html>")
    subject = FakeResponse(404, text="<html></html>")
    diff = compare_responses("html", ref, subject)
    assert not diff.equal
    assert "status mismatch" in diff.detail


def test_case_readback_swaps_the_compared_response():
    # A mutation case compares the read-back GET, not the mutation response.
    mutated = _sparql([{"s": {"type": "uri", "value": "http://new"}}])
    target = FakeTarget(
        "reference",
        {
            "/submit": FakeResponse(200, text="ok"),
            "/state": FakeResponse(200, payload=mutated),
        },
    )
    case = Case(name="submit", category="sparql", method="POST", path="/submit", readback="/state")
    response = case.issue(target)
    assert response.json() == mutated
