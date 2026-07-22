"""Pytest fixtures for the live differential conformance run.

These fixtures build the reference and subject `Target`s from environment
variables (see .env.example) and gate the live cases on the stack being up. The
comparison-library unit tests (test_compare.py) and the driver unit tests
(test_conformance_driver.py) need none of this and run without a stack.
"""

from __future__ import annotations

import os
from typing import List

import pytest

from conformance import Target

# Default host ports match docker-compose.yaml.
REFERENCE_URL = os.environ.get("REFERENCE_URL", "http://localhost:17777")
# The classic stack reads the public graph straight from Virtuoso, so a
# differential run seeds the reference by loading the corpus into Virtuoso's
# graph store (the Node app then serves it), mirroring the bench harness.
REFERENCE_VIRTUOSO_URL = os.environ.get("REFERENCE_VIRTUOSO_URL", "http://localhost:18990")
SUBJECT_URLS = {
    "postgres": os.environ.get("SUBJECT_POSTGRES_URL", "http://localhost:18902"),
    "sqlite": os.environ.get("SUBJECT_SQLITE_URL", "http://localhost:18903"),
    "rocksdb": os.environ.get("SUBJECT_ROCKSDB_URL", "http://localhost:18904"),
}


@pytest.fixture(scope="session")
def reference() -> Target:
    return Target("reference", REFERENCE_URL, is_reference=True)


@pytest.fixture(scope="session")
def reference_virtuoso() -> Target:
    """The reference stack's Virtuoso, used only to seed the corpus the Node app
    then serves. Not a comparison target."""
    return Target("reference-virtuoso", REFERENCE_VIRTUOSO_URL)


@pytest.fixture(scope="session")
def subjects() -> List[Target]:
    return [Target(f"subject-{name}", url) for name, url in SUBJECT_URLS.items()]


@pytest.fixture(scope="session")
def stack(reference: Target, subjects: List[Target]) -> None:
    """Skip the whole live suite unless the reference and every subject answer a
    SPARQL ASK probe."""
    targets = [reference] + subjects
    down = [t.name for t in targets if not t.sparql_ready()]
    if down:
        pytest.skip(f"conformance stack not up (unreachable: {', '.join(down)})")


def test_stack_healthy(stack: None, reference: Target, subjects: List[Target]) -> None:
    """Smoke: every service answers POST /sparql ASK {} with 200."""
    for target in [reference] + subjects:
        assert target.sparql_ready(), f"{target.name} not ready"
