"""Shared fixtures, including a real sbol-db server for integration tests.

The ``live_server`` fixture boots ``sbol-db server`` on a SQLite backend with
auth disabled, so integration tests exercise the actual HTTP surface. It skips
cleanly when no binary is available, so ``pytest`` still runs the unit suite.
Point it at a build with ``SBOL_DB_BIN=/path/to/sbol-db`` or let it discover
``target/{debug,release}/sbol-db`` under the repo root.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import socket
import subprocess
import tempfile
import time
from typing import Iterator, Optional

import httpx
import pytest


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


@pytest.fixture(scope="session")
def live_server() -> Iterator[str]:
    binary = _find_binary()
    if binary is None:
        pytest.skip("sbol-db binary not found; build it (cargo build -p sbol-db) or set SBOL_DB_BIN")

    tmpdir = tempfile.mkdtemp(prefix="sbol-db-it-")
    db_url = f"sqlite://{os.path.join(tmpdir, 'sbol.db')}"
    env = {
        **os.environ,
        "DATABASE_URL": db_url,
        "SBOL_DB_SPARQL_AUTH_DISABLED": "true",
    }
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
