"""Self-consistency smoke: the sbol-db subject side, no classic stack.

This always-run smoke boots the compiled `sbol-db` compatibility server on
every configured backend (SQLite and RocksDB by default; Postgres in the phase
gate), seeds the shared corpus, and drives the
Elasticsearch-independent read / metadata / SPARQL / download subset, asserting
each endpoint answers coherently. It proves the subject half of the differential
harness works before any reference is in the loop.

The final test runs the *driver itself* across all configured backends so the
fan-out, comparator selection, and semantic diffs are exercised end to end
without the classic stack: honest backends holding the identical corpus must
compare equal on every subset case.

The suite skips (never fails) only when the `sbol-db` binary cannot be located
or built in this environment; the assertions themselves never depend on the
reference stack.
"""

from __future__ import annotations

import os

import pytest
from rdflib import Graph

import cases as case_defs
from conformance import Target, run_cases
from subject import LocalSubject, SubjectError, find_binary

SUPPORTED_BACKENDS = {"sqlite", "rocksdb", "postgres"}


def configured_backends() -> list[str]:
    configured = os.environ.get("SBOL_DB_TEST_BACKENDS", "sqlite,rocksdb")
    backends = [item.strip() for item in configured.split(",") if item.strip()]
    unknown = set(backends) - SUPPORTED_BACKENDS
    if unknown:
        raise RuntimeError(f"unsupported SBOL_DB_TEST_BACKENDS values: {sorted(unknown)}")
    if not backends:
        raise RuntimeError("SBOL_DB_TEST_BACKENDS selected no backends")
    return backends


BACKENDS = configured_backends()


@pytest.fixture(scope="module")
def binary():
    try:
        return find_binary()
    except SubjectError as err:
        pytest.skip(f"sbol-db binary unavailable: {err}")


@pytest.fixture(params=BACKENDS)
def seeded(request, binary):
    """A booted, corpus-seeded subject on one backend."""
    try:
        subject = LocalSubject(request.param, binary=binary)
        subject.__enter__()
    except SubjectError as err:
        pytest.fail(f"could not start {request.param} subject: {err}")
    try:
        subject.seed(case_defs.load_corpus())
        yield subject
    finally:
        subject.__exit__(None, None, None)


def _rows(result: dict) -> list:
    return result["results"]["bindings"]


def _one_value(result: dict, var: str) -> str:
    return _rows(result)[0][var]["value"]


def test_seed_is_verbatim(seeded):
    """The whole corpus loads into the public graph and reads back exactly."""
    count = case_defs.load_corpus().strip().count("\n") + 1
    result = seeded.sparql_json("SELECT (COUNT(*) AS ?c) FROM <http://synbiohub.org/public> WHERE { ?s ?p ?o }")
    assert int(_one_value(result, "c")) == count


def test_object_metadata_coherent(seeded):
    """Object metadata returns the seeded displayId, version, and title."""
    response = seeded.get(f"{case_defs.OBJECT_PATH}/metadata", headers={"Accept": "application/json"})
    assert response.status_code == 200
    metadata = response.json()[0]
    assert metadata["displayId"] == "pSmoke"
    assert metadata["version"] == "1"
    assert metadata["name"] == "pSmoke promoter"
    assert metadata["type"] == "http://sbols.org/v2#ComponentDefinition"


def test_type_counts(seeded):
    """Classic's plain-integer counts reflect the one CD and one Collection."""
    cd = seeded.get("/ComponentDefinition/count", headers={"Accept": "text/plain"})
    assert cd.status_code == 200
    assert int(cd.text) == 1

    coll = seeded.get("/Collection/count", headers={"Accept": "text/plain"})
    assert coll.status_code == 200
    assert int(coll.text) == 1


def test_root_collections(seeded):
    """The lone Collection is a root (it is nobody's member)."""
    response = seeded.get("/rootCollections", headers={"Accept": "application/json"})
    assert response.status_code == 200
    collections = {row["uri"] for row in response.json()}
    assert "http://synbiohub.org/public/smoke/smoke_collection/1" in collections


def test_sbol_download_is_valid_rdf(seeded):
    """The explicit SBOL 2 closure preserves the seeded object identities."""
    response = seeded.get(f"{case_defs.OBJECT_PATH}/sbol", params={"version": "sbol2"})
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("application/rdf+xml")
    graph = Graph().parse(data=response.text, format="xml")
    subjects = {str(s) for s in graph.subjects()}
    assert "http://synbiohub.org/public/smoke/pSmoke/1" in subjects
    assert "http://synbiohub.org/public/smoke/pSmoke_seq/1" in subjects


def test_gff_download(seeded):
    """The `/gff` closure carries the GFF3 header and sequence region."""
    response = seeded.get(f"{case_defs.OBJECT_PATH}/gff")
    assert response.status_code == 200
    assert "##gff-version 3" in response.text


def test_subset_cases_all_coherent(seeded):
    """Every subset case answers 200 with a payload its comparator can parse."""
    target = Target(f"subject-{seeded.backend}", seeded.base)
    for case in case_defs.read_subset_cases():
        response = case.issue(target)
        assert response.status_code == 200, f"{case.name}: {response.status_code}"
        if case.category == "sparql":
            body = response.json()
            assert "head" in body, f"{case.name}: no head in {body}"
        elif case.category == "sbol":
            Graph().parse(data=response.text, format="xml")
        elif case.category == "gff":
            assert response.text.strip(), f"{case.name}: empty gff"


def test_driver_cross_backend_consistency(binary):
    """Exercise the full driver end to end across every configured backend.

    The first backend stands in as the reference; every remaining backend holds
    the identical corpus and must compare equal on every subset case. A failure
    here is either a backend divergence or a harness bug, never a skipped gate.
    """
    if len(BACKENDS) < 2:
        pytest.fail("the cross-backend driver requires at least two configured backends")

    subjects: list[LocalSubject] = []
    try:
        try:
            for backend in BACKENDS:
                subject = LocalSubject(backend, binary=binary)
                subject.__enter__()
                subjects.append(subject)
        except SubjectError as err:
            pytest.fail(f"could not start configured subjects: {err}")

        corpus = case_defs.load_corpus()
        for subject in subjects:
            subject.seed(corpus)

        reference_subject = subjects[0]
        reference = Target(
            f"reference-{reference_subject.backend}", reference_subject.base, is_reference=True
        )
        targets = [Target(f"subject-{subject.backend}", subject.base) for subject in subjects[1:]]
        results = run_cases(case_defs.read_subset_cases(), reference, targets)

        assert results, "no cases ran"
        for result in results:
            for target_result in result.results:
                assert target_result.equal, (
                    f"{result.case} diverged across backends: {target_result.detail}"
                )
    finally:
        for subject in reversed(subjects):
            subject.__exit__(None, None, None)
