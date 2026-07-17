"""Self-consistency smoke: the sbol-db subject side, no classic stack.

This always-run smoke boots the compiled `sbol-db` compat server on each local
backend (SQLite and RocksDB), seeds the shared corpus, and drives the
Elasticsearch-independent read / metadata / SPARQL / download subset, asserting
each endpoint answers coherently. It proves the subject half of the differential
harness works before any reference is in the loop.

The final test runs the *driver itself* across two independent backends (SQLite
as the stand-in reference, RocksDB as the subject) so the fan-out, comparator
selection, and semantic diffs are exercised end to end without the classic
stack: two honest backends holding the identical corpus must compare equal on
every subset case.

The suite skips (never fails) only when the `sbol-db` binary cannot be located
or built in this environment; the assertions themselves never depend on the
reference stack.
"""

from __future__ import annotations

import pytest
from rdflib import Graph

import cases as case_defs
from conformance import Target, run_cases
from subject import LocalSubject, SubjectError, find_binary

BACKENDS = ["sqlite", "rocksdb"]


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
        pytest.skip(f"could not start {request.param} subject: {err}")
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
    binding = _rows(response.json())[0]
    assert binding["displayId"]["value"] == "pSmoke"
    assert binding["version"]["value"] == "1"
    assert binding["name"]["value"] == "pSmoke promoter"
    assert binding["type"]["value"] == "http://sbols.org/v2#ComponentDefinition"


def test_type_counts(seeded):
    """The SPARQL-backed type counts reflect the one CD and one Collection."""
    cd = seeded.get("/ComponentDefinition/count", headers={"Accept": "application/json"})
    assert cd.status_code == 200
    assert int(_one_value(cd.json(), "count")) == 1

    coll = seeded.get("/Collection/count", headers={"Accept": "application/json"})
    assert coll.status_code == 200
    assert int(_one_value(coll.json(), "count")) == 1


def test_root_collections(seeded):
    """The lone Collection is a root (it is nobody's member)."""
    response = seeded.get("/rootCollections", headers={"Accept": "application/json"})
    assert response.status_code == 200
    collections = {row["Collection"]["value"] for row in _rows(response.json())}
    assert "http://synbiohub.org/public/smoke/smoke_collection/1" in collections


def test_sbol_download_is_valid_rdf(seeded):
    """The `/sbol` closure is well-formed RDF/XML naming the object and its
    Sequence, so the semantic SBOL comparator has something to parse."""
    response = seeded.get(f"{case_defs.OBJECT_PATH}/sbol")
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
    """Exercise the full driver end to end: SQLite stands in as the reference,
    RocksDB is the subject, both hold the identical corpus, and every subset
    case must compare equal. A failure here is a harness bug, not a data bug."""
    try:
        reference_subject = LocalSubject("sqlite", binary=binary)
        subject_subject = LocalSubject("rocksdb", binary=binary)
        reference_subject.__enter__()
    except SubjectError as err:
        pytest.skip(f"could not start subjects: {err}")
    try:
        try:
            subject_subject.__enter__()
        except SubjectError as err:
            pytest.skip(f"could not start rocksdb subject: {err}")
        try:
            corpus = case_defs.load_corpus()
            reference_subject.seed(corpus)
            subject_subject.seed(corpus)

            reference = Target("reference-sqlite", reference_subject.base, is_reference=True)
            subject = Target("subject-rocksdb", subject_subject.base)
            results = run_cases(case_defs.read_subset_cases(), reference, [subject])

            assert results, "no cases ran"
            for result in results:
                for target_result in result.results:
                    assert target_result.equal, f"{result.case} diverged across backends: {target_result.detail}"
        finally:
            subject_subject.__exit__(None, None, None)
    finally:
        reference_subject.__exit__(None, None, None)
