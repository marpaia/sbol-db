"""Best-effort live differential: reference vs subjects, ES-independent subset.

This is the differential matrix restricted to the endpoints that answer straight
from the triplestore (see cases.py), so it can run even when SBOLExplorer /
Elasticsearch is unavailable. It is gated on the `stack` fixture: if the classic
reference or any subject is unreachable, the whole module skips rather than
fails. Bringing the classic stack up healthy is environment-dependent (the
reference images are amd64; Virtuoso is emulated and Elasticsearch 6.3.2 OOMs
under emulation on Apple Silicon), so a green run here is CI-runner (amd64)
territory. When the stack is down, the self-consistency smoke
(test_subject_smoke.py) still proves the subject side end to end.

The corpus is seeded identically on both sides: into Virtuoso's graph store for
the reference (the Node app serves it) and into each subject's graph store, then
the identical request is fanned out and compared per the §7.3 matrix.
"""

from __future__ import annotations

import json
import os
from typing import List

import pytest
from requests.auth import HTTPBasicAuth, HTTPDigestAuth

import cases as case_defs
from conformance import Target, run_cases

PUBLIC_GRAPH = "http://synbiohub.org/public"

# The write endpoints' credentials both default to dba/dba: the sbol-db subjects
# challenge with HTTP Basic, the classic stack's Virtuoso with digest.
SUBJECT_AUTH = HTTPBasicAuth("dba", "dba")
VIRTUOSO_AUTH = HTTPDigestAuth("dba", "dba")


@pytest.fixture(scope="module")
def seeded_stack(stack, reference_virtuoso: Target, subjects: List[Target]) -> None:
    """Load the shared corpus into the reference's Virtuoso and every subject.

    `stack` has already gated on reachability, so a failure here is a real seed
    error worth surfacing."""
    corpus = case_defs.load_corpus()
    reference_virtuoso.load_graph(corpus, PUBLIC_GRAPH, auth=VIRTUOSO_AUTH)
    for subject in subjects:
        subject.load_graph(corpus, PUBLIC_GRAPH, auth=SUBJECT_AUTH)


def test_read_subset_matches(
    seeded_stack,
    reference: Target,
    subjects: List[Target],
) -> None:
    """Every ES-independent case compares equal on every subject, and the
    reference-vs-subject report is written for the run record."""
    results = run_cases(case_defs.read_subset_cases(), reference, subjects)

    out = os.environ.get("CONFORMANCE_OUT")
    if out:
        report = {
            "mode": "read-subset",
            "reference": reference.base,
            "subjects": [s.base for s in subjects],
            "cases": [r.to_dict() for r in results],
        }
        with open(out, "w") as handle:
            json.dump(report, handle, indent=2)

    failures = [f"{r.case}/{tr.target}: {tr.detail}" for r in results for tr in r.results if not tr.equal]
    assert not failures, "differential subset diverged:\n" + "\n".join(failures)
