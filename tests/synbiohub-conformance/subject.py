"""Boot an sbol-db compat server as a local subprocess subject.

The self-consistency smoke needs a live sbol-db to drive but no classic stack,
so this launches the compiled `sbol-db` binary directly against a throwaway
SQLite or RocksDB store, seeds a corpus over the Virtuoso-compatible graph store
protocol, and hands back the base URL. It is the same subject the docker-compose
stack runs, minus Docker: `db migrate` then `server --no-worker` on a free port.

The binary is located from `$SBOLDB_BIN`, then a prebuilt `target/{release,debug}`
artifact, and is built with `cargo build -p sbol-db` (UI build skipped) only as a
last resort so a normal run reuses whatever the workspace already compiled.
"""

from __future__ import annotations

import os
import socket
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Optional

import requests
from requests.auth import HTTPBasicAuth

# tests/synbiohub-conformance/subject.py -> repo root is two parents up.
REPO_ROOT = Path(__file__).resolve().parents[2]

PUBLIC_GRAPH = "http://synbiohub.org/public"

# The compat server's write endpoints challenge with HTTP Basic; these are the
# built-in defaults (SBOL_DB_SPARQL_AUTH_USER / _PASSWORD), pinned here so the
# subject's auth is deterministic regardless of the caller's environment.
SPARQL_AUTH_USER = "dba"
SPARQL_AUTH_PASSWORD = "dba"


class SubjectError(RuntimeError):
    """A subject could not be built, started, or reached."""


def find_binary() -> Path:
    """Locate the `sbol-db` binary. `SBOLDB_BIN` pins an explicit path; otherwise
    the current source is built fresh (UI skipped) so a stale prior artifact can
    never drive the harness. Raises SubjectError if the build fails."""
    override = os.environ.get("SBOLDB_BIN")
    if override:
        path = Path(override)
        if not path.exists():
            raise SubjectError(f"SBOLDB_BIN={override} does not exist")
        return path
    env = dict(os.environ, SBOL_DB_SKIP_UI_BUILD="1")
    result = subprocess.run(
        ["cargo", "build", "-p", "sbol-db"],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SubjectError(f"cargo build -p sbol-db failed:\n{result.stderr[-2000:]}")
    built = REPO_ROOT / "target" / "debug" / "sbol-db"
    if not built.exists():
        raise SubjectError("cargo build reported success but the binary is missing")
    return built


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class LocalSubject:
    """A locally launched sbol-db compat server on one storage backend.

    Use as a context manager: entering migrates a fresh store, starts the
    server, and blocks until `POST /sparql` answers; exiting stops the process
    and removes the store directory."""

    def __init__(self, backend: str, binary: Optional[Path] = None):
        if backend not in ("sqlite", "rocksdb", "postgres"):
            raise SubjectError(f"unsupported subject backend {backend!r}")
        self.backend = backend
        self.binary = str(binary or find_binary())
        self.auth = HTTPBasicAuth(SPARQL_AUTH_USER, SPARQL_AUTH_PASSWORD)
        self.session = requests.Session()
        self._tmp: Optional[str] = None
        self._proc: Optional[subprocess.Popen] = None
        self._log = None
        self.base: str = ""

    def _database_url(self) -> str:
        if self.backend == "postgres":
            url = os.environ.get("SBOL_DB_TEST_POSTGRES_URL")
            if not url:
                raise SubjectError(
                    "postgres was requested but SBOL_DB_TEST_POSTGRES_URL is unset; "
                    "point it at an isolated empty test database"
                )
            return url
        root = Path(self._tmp or "")
        if self.backend == "sqlite":
            return f"sqlite://{root / 'sbol.db'}?mode=rwc"
        return f"rocksdb://{root / 'sbol.rocksdb'}"

    def _env(self) -> dict:
        return dict(
            os.environ,
            DATABASE_URL=self._database_url(),
            SBOL_DB_SPARQL_AUTH_USER=SPARQL_AUTH_USER,
            SBOL_DB_SPARQL_AUTH_PASSWORD=SPARQL_AUTH_PASSWORD,
        )

    def __enter__(self) -> "LocalSubject":
        self._tmp = tempfile.mkdtemp(prefix=f"sbol-subject-{self.backend}-")
        env = self._env()

        # `db migrate` initializes the schema (and, for RocksDB, the column
        # families) and exits before the server opens the exclusive handle.
        migrate = subprocess.run(
            [self.binary, "db", "migrate"],
            env=env,
            capture_output=True,
            text=True,
        )
        if migrate.returncode != 0:
            raise SubjectError(f"db migrate ({self.backend}) failed:\n{migrate.stderr[-2000:]}")

        port = _free_port()
        self.base = f"http://127.0.0.1:{port}"
        self._log = open(Path(self._tmp) / "server.log", "w")
        self._proc = subprocess.Popen(
            [self.binary, "server", "--bind", f"127.0.0.1:{port}", "--no-worker"],
            env=env,
            stdout=self._log,
            stderr=subprocess.STDOUT,
        )
        try:
            self._wait_ready()
        except SubjectError:
            self.__exit__(None, None, None)
            raise
        return self

    def __exit__(self, *exc) -> None:
        if self._proc is not None:
            self._proc.terminate()
            try:
                self._proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self._proc.kill()
                self._proc.wait(timeout=15)
            self._proc = None
        if self._log is not None:
            self._log.close()
            self._log = None
        if self._tmp is not None:
            import shutil

            shutil.rmtree(self._tmp, ignore_errors=True)
            self._tmp = None

    def _wait_ready(self, timeout: float = 60.0) -> None:
        deadline = time.time() + timeout
        last = ""
        while time.time() < deadline:
            if self._proc is not None and self._proc.poll() is not None:
                raise SubjectError(
                    f"subject {self.backend} exited early " f"(code {self._proc.returncode}); see server.log"
                )
            try:
                response = self.session.post(
                    self.base + "/sparql",
                    data={"query": "ASK {}"},
                    headers={"Accept": "application/sparql-results+json"},
                    timeout=5,
                )
                if response.status_code == 200:
                    return
                last = f"status {response.status_code}"
            except requests.RequestException as err:
                last = str(err)
            time.sleep(0.3)
        raise SubjectError(f"subject {self.backend} not ready in {timeout}s ({last})")

    def seed(self, ntriples: str, graph: str = PUBLIC_GRAPH) -> None:
        """Replace `graph` with the given N-Triples over the graph store
        protocol, exactly as the Virtuoso-drop-in write path expects."""
        response = self.session.put(
            self.base + "/sparql-graph-crud-auth/",
            params={"graph-uri": graph},
            data=ntriples.encode("utf-8"),
            headers={"Content-Type": "application/n-triples"},
            auth=self.auth,
            timeout=120,
        )
        response.raise_for_status()

    def get(self, path: str, **kwargs) -> requests.Response:
        return self.session.get(self.base + path, timeout=120, **kwargs)

    def sparql_json(self, query: str) -> dict:
        response = self.session.post(
            self.base + "/sparql",
            data={"query": query},
            headers={"Accept": "application/sparql-results+json"},
            timeout=120,
        )
        response.raise_for_status()
        return response.json()
