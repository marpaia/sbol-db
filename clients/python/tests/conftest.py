"""Shared fixtures for the end-to-end suite.

``live_server`` boots a real ``sbol-db server`` and yields its base URL. It is
parametrized over storage backends: by default just SQLite (a throwaway temp
database, so every run starts clean and needs no external services), and
optionally Postgres when ``SBOL_DB_TEST_BACKENDS`` includes it (pointing at
``DATABASE_URL``, default the repo's compose Postgres). Every e2e test then runs
once per configured backend.

The suite skips cleanly when no ``sbol-db`` binary is available, so ``pytest``
still runs the unit tests. Point it at a build with ``SBOL_DB_BIN=/path/to/
sbol-db`` (the ``run-e2e.sh`` runner does this after a fresh ``cargo build``) or
let it discover ``target/{debug,release}/sbol-db`` under the repo root.

To avoid cross-run interference on a shared backend (Postgres), tests derive
their object identifiers from the session-unique ``run_id`` and the per-test
``unique`` fixture, so search assertions only ever see this run's data.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import socket
import subprocess
import tempfile
import time
import uuid
from typing import Iterator, List, Optional

import httpx
import pytest

from sbol_db import SbolDbClient

NAMESPACE = "https://sbol-db.test/e2e"


def _configured_backends() -> List[str]:
    raw = os.environ.get("SBOL_DB_TEST_BACKENDS", "sqlite")
    return [b.strip() for b in raw.split(",") if b.strip()]


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def _find_binary() -> Optional[str]:
    from_env = os.environ.get("SBOL_DB_BIN")
    if from_env and pathlib.Path(from_env).exists():
        return from_env
    repo_root = pathlib.Path(__file__).resolve().parents[3]
    for profile in ("debug", "release"):
        candidate = repo_root / "target" / profile / "sbol-db"
        if candidate.exists():
            return str(candidate)
    return None


def _wait_ready(base_url: str, proc: "subprocess.Popen[bytes]", deadline_s: float = 30.0) -> None:
    end = time.monotonic() + deadline_s
    while time.monotonic() < end:
        if proc.poll() is not None:
            raise RuntimeError(f"server exited early with code {proc.returncode}")
        try:
            resp = httpx.get(f"{base_url}/readyz", timeout=1.0)
            if resp.status_code == 200 and resp.json().get("status") == "ready":
                return
        except httpx.HTTPError:
            pass
        time.sleep(0.2)
    raise RuntimeError("server did not become ready in time")


def _database_url(backend: str, tmpdir: str) -> str:
    if backend == "sqlite":
        return f"sqlite://{os.path.join(tmpdir, 'sbol.db')}"
    if backend == "postgres":
        return os.environ.get("DATABASE_URL", "postgres://sbol:sbol@localhost:5432/sbol")
    if backend == "rocksdb":
        return f"rocksdb://{os.path.join(tmpdir, 'rocks')}"
    raise ValueError(f"unknown backend: {backend}")


@pytest.fixture(scope="session", params=_configured_backends())
def live_server(request: pytest.FixtureRequest) -> Iterator[str]:
    backend = request.param
    binary = _find_binary()
    if binary is None:
        pytest.skip("sbol-db binary not found; run clients/python/run-e2e.sh or set SBOL_DB_BIN")

    tmpdir = tempfile.mkdtemp(prefix=f"sbol-db-e2e-{backend}-")
    db_url = _database_url(backend, tmpdir)
    env = {**os.environ, "DATABASE_URL": db_url, "SBOL_DB_SPARQL_AUTH_DISABLED": "true"}
    subprocess.run([binary, "db", "migrate"], env=env, check=True, capture_output=True)

    port = _free_port()
    base_url = f"http://127.0.0.1:{port}"
    proc = subprocess.Popen(
        [binary, "server", "--bind", f"127.0.0.1:{port}", "--no-worker"],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    try:
        _wait_ready(base_url, proc)
        yield base_url
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        shutil.rmtree(tmpdir, ignore_errors=True)


@pytest.fixture(scope="session")
def run_id() -> str:
    """A short token unique to this test session, mixed into identifiers so
    concurrent or repeated runs on a shared backend do not collide."""
    return uuid.uuid4().hex[:8]


@pytest.fixture()
def unique(run_id: str) -> str:
    """A per-test alphanumeric token (valid as an SBOL displayId component)."""
    return f"e2e{run_id}{uuid.uuid4().hex[:6]}"


@pytest.fixture()
def client(live_server: str) -> Iterator[SbolDbClient]:
    with SbolDbClient(live_server) as c:
        yield c


def fasta(display_id: str, sequence: str = "ttgacggctagctcagtcctaggtacagtgctagc") -> str:
    """A one-record FASTA body; the importer projects it to an SBOL3 Component
    plus a Sequence, with `display_id` as the component's displayId."""
    return f">{display_id} e2e fixture\n{sequence}\n"
