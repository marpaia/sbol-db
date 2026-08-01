#!/usr/bin/env python3
"""Cross-repository SBOL Identity, CLI sync, and MCP prepared-write test.

This uses only the Python standard library and real debug binaries. It is
intentionally separate from either Cargo workspace because its contract spans
the sbol-rs and sbol-db repositories.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import queue
import re
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any
import urllib.error
import urllib.parse
import urllib.request


FIXTURE = """\
@prefix sbol: <http://sbols.org/v2#> .
@prefix dcterms: <http://purl.org/dc/terms/> .

<http://example.org/loopback_promoter/1>
    a sbol:ComponentDefinition ;
    sbol:displayId "loopback_promoter" ;
    sbol:persistentIdentity <http://example.org/loopback_promoter> ;
    sbol:version "1" ;
    dcterms:title "Private promoter" ;
    sbol:type <http://www.biopax.org/release/biopax-level3.owl#DnaRegion> ;
    sbol:role <http://identifiers.org/so/SO:0000167> .
"""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: N802
        return None


def request(
    opener: urllib.request.OpenerDirector,
    url: str,
    *,
    payload: bytes | None = None,
    headers: dict[str, str] | None = None,
    method: str | None = None,
) -> tuple[int, dict[str, str], bytes]:
    req = urllib.request.Request(
        url,
        data=payload,
        headers=headers or {},
        method=method,
    )
    try:
        with opener.open(req, timeout=15) as response:
            return response.status, dict(response.headers.items()), response.read()
    except urllib.error.HTTPError as error:
        return error.code, dict(error.headers.items()), error.read()


def json_request(
    opener: urllib.request.OpenerDirector,
    url: str,
    value: Any,
    *,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, str], Any]:
    combined = {"Content-Type": "application/json", **(headers or {})}
    status, response_headers, body = request(
        opener,
        url,
        payload=json.dumps(value).encode(),
        headers=combined,
        method="POST",
    )
    decoded = json.loads(body) if body else None
    return status, response_headers, decoded


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def run(
    command: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=120,
        check=False,
    )
    if result.returncode != 0:
        rendered = " ".join(command)
        raise RuntimeError(
            f"command failed ({result.returncode}): {rendered}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def wait_for_server(origin: str, process: subprocess.Popen[str]) -> None:
    opener = urllib.request.build_opener()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            output = process.stdout.read() if process.stdout else ""
            raise RuntimeError(f"sbol-db exited before becoming ready:\n{output}")
        try:
            status, _, _ = request(opener, f"{origin}/healthz")
            if status == 200:
                return
        except urllib.error.URLError:
            pass
        time.sleep(0.1)
    raise RuntimeError("sbol-db did not become ready within 30 seconds")


def register_user(
    opener: urllib.request.OpenerDirector,
    origin: str,
    username: str,
) -> None:
    form = urllib.parse.urlencode(
        {
            "name": f"{username.title()} Example",
            "username": username,
            "email": f"{username}@example.org",
            "password": "s3cret",
        }
    ).encode()
    status, _, body = request(
        opener,
        f"{origin}/register",
        payload=form,
        headers={
            "Accept": "application/json",
            "Content-Type": "application/x-www-form-urlencoded",
        },
        method="POST",
    )
    if status != 200:
        raise RuntimeError(f"registration failed ({status}): {body.decode(errors='replace')}")


def browser_session(origin: str, username: str) -> urllib.request.OpenerDirector:
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    status, _, body = json_request(
        opener,
        f"{origin}/api/v2/session",
        {"identifier": username, "password": "s3cret"},
        headers={"Origin": origin},
    )
    if status != 200 or not body.get("authenticated"):
        raise RuntimeError(f"browser session login failed ({status}): {body}")
    return opener


def cli_oauth_login(
    sbol: Path,
    origin: str,
    opener: urllib.request.OpenerDirector,
    env: dict[str, str],
) -> None:
    process = subprocess.Popen(
        [str(sbol), "registry", "login", origin, "--no-browser"],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        bufsize=1,
    )
    assert process.stdout is not None
    output: queue.Queue[str] = queue.Queue()

    def read_output() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            output.put(line)

    reader = threading.Thread(target=read_output, daemon=True)
    reader.start()
    lines: list[str] = []
    authorization_url: str | None = None
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and authorization_url is None:
        try:
            line = output.get(timeout=0.2)
        except queue.Empty:
            if process.poll() is not None:
                break
            continue
        lines.append(line)
        stripped = line.strip()
        if stripped.startswith(origin) and "/oauth/authorize?" in stripped:
            authorization_url = stripped
    if authorization_url is None:
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError("sbol did not print an OAuth authorization URL:\n" + "".join(lines))

    status, _, page = request(opener, authorization_url)
    if status != 200 or b"SBOL Identity" not in page:
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError(f"authorization page failed ({status})")

    parsed = urllib.parse.urlsplit(authorization_url)
    decision = urllib.parse.urlencode(
        [*urllib.parse.parse_qsl(parsed.query, keep_blank_values=True), ("decision", "allow")]
    ).encode()
    status, _, callback = request(
        opener,
        f"{origin}/oauth/authorize",
        payload=decision,
        headers={
            "Origin": origin,
            "Content-Type": "application/x-www-form-urlencoded",
        },
        method="POST",
    )
    if status != 200 or b"Sign in complete" not in callback:
        process.terminate()
        process.wait(timeout=5)
        raise RuntimeError(f"OAuth callback failed ({status}): {callback.decode(errors='replace')}")

    process.wait(timeout=20)
    reader.join(timeout=1)
    while not output.empty():
        lines.append(output.get_nowait())
    if process.returncode != 0:
        raise RuntimeError("sbol OAuth login failed:\n" + "".join(lines))


def base64url(value: bytes) -> str:
    return base64.urlsafe_b64encode(value).rstrip(b"=").decode()


def mcp_access_token(
    origin: str,
    browser: urllib.request.OpenerDirector,
) -> str:
    redirect_uri = "http://127.0.0.1:9/callback"
    status, _, registration = json_request(
        browser,
        f"{origin}/oauth/register",
        {
            "client_name": "SBOL machine-access loopback test",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        },
        headers={"Origin": origin},
    )
    if status != 201:
        raise RuntimeError(f"MCP OAuth client registration failed ({status}): {registration}")
    client_id = registration["client_id"]
    verifier = secrets.token_urlsafe(48)
    challenge = base64url(hashlib.sha256(verifier.encode()).digest())
    state = secrets.token_urlsafe(24)
    resource = f"{origin}/mcp"
    query = {
        "response_type": "code",
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
        "resource": resource,
        "scope": "sbol:read sbol:write",
        "state": state,
    }
    authorization_url = f"{origin}/oauth/authorize?{urllib.parse.urlencode(query)}"
    status, _, _ = request(browser, authorization_url)
    if status != 200:
        raise RuntimeError(f"MCP authorization page failed ({status})")

    jar = next(
        handler.cookiejar
        for handler in browser.handlers
        if isinstance(handler, urllib.request.HTTPCookieProcessor)
    )
    no_redirect = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(jar), NoRedirect()
    )
    decision = urllib.parse.urlencode({**query, "decision": "allow"}).encode()
    status, response_headers, body = request(
        no_redirect,
        f"{origin}/oauth/authorize",
        payload=decision,
        headers={
            "Origin": origin,
            "Content-Type": "application/x-www-form-urlencoded",
        },
        method="POST",
    )
    if status != 302:
        raise RuntimeError(f"MCP authorization decision failed ({status}): {body!r}")
    location = response_headers.get("Location") or response_headers.get("location")
    if not location:
        raise RuntimeError("MCP authorization response omitted Location")
    callback = urllib.parse.urlsplit(location)
    callback_query = dict(urllib.parse.parse_qsl(callback.query))
    if callback_query.get("state") != state or "code" not in callback_query:
        raise RuntimeError("MCP authorization response had an invalid state or no code")

    token_form = urllib.parse.urlencode(
        {
            "grant_type": "authorization_code",
            "code": callback_query["code"],
            "client_id": client_id,
            "redirect_uri": redirect_uri,
            "resource": resource,
            "code_verifier": verifier,
        }
    ).encode()
    status, _, token_body = request(
        urllib.request.build_opener(),
        f"{origin}/oauth/token",
        payload=token_form,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    token = json.loads(token_body)
    if status != 200 or token.get("resource") != resource:
        raise RuntimeError(f"MCP token exchange failed ({status}): {token}")
    return str(token["access_token"])


def mcp_message(origin: str, token: str, message: dict[str, Any]) -> Any:
    opener = urllib.request.build_opener()
    status, _, body = json_request(
        opener,
        f"{origin}/mcp",
        message,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json, text/event-stream",
            "MCP-Protocol-Version": "2025-11-25",
        },
    )
    if status != 200:
        raise RuntimeError(f"MCP request failed ({status}): {body}")
    return body


def mcp_tool(origin: str, token: str, request_id: int, name: str, arguments: Any) -> Any:
    return mcp_message(
        origin,
        token,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
    )


def assert_private(origin: str, iri: str) -> None:
    encoded = urllib.parse.quote(iri, safe="")
    status, _, _ = request(
        urllib.request.build_opener(),
        f"{origin}/api/v2/collections/{encoded}",
    )
    if status != 404:
        raise RuntimeError(f"private collection was anonymously visible ({status})")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--sbol-rs",
        type=Path,
        required=True,
        help="sbol-rs checkout containing target/debug/sbol",
    )
    parser.add_argument(
        "--sbol-db-bin",
        type=Path,
        help="sbol-db binary (defaults to this checkout's target/debug/sbol-db)",
    )
    args = parser.parse_args()

    sbol_db_root = Path(__file__).resolve().parents[2]
    sbol_db = (args.sbol_db_bin or sbol_db_root / "target/debug/sbol-db").resolve()
    sbol = (args.sbol_rs / "target/debug/sbol").resolve()
    for binary, build in [
        (sbol_db, "cargo build -p sbol-db"),
        (sbol, "cargo build -p sbol-cli"),
    ]:
        if not binary.is_file():
            raise RuntimeError(f"missing {binary}; run `{build}` in its repository")

    port = free_port()
    origin = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="sbol-machine-access-") as temporary:
        root = Path(temporary)
        database = root / "registry.sqlite"
        database_url = f"sqlite://{database}"
        credentials = root / "credentials.json"
        project_a = root / "project-a"
        project_b = root / "project-b"
        project_a.mkdir()
        project_b.mkdir()

        run([str(sbol_db), "--database-url", database_url, "db", "migrate"])
        server_env = os.environ.copy()
        server_env.update(
            {
                "SBOL_DB_PUBLIC_ORIGIN": origin,
                "SBOL_DB_MCP_ENABLED": "true",
                "SBOL_DB_PORTAL_ENABLED": "false",
                "RUST_LOG": "warn",
            }
        )
        server = subprocess.Popen(
            [
                str(sbol_db),
                "--database-url",
                database_url,
                "server",
                "--bind",
                f"127.0.0.1:{port}",
                "--no-worker",
            ],
            env=server_env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_for_server(origin, server)
            anonymous = urllib.request.build_opener()
            register_user(anonymous, origin, "alice")
            browser = browser_session(origin, "alice")
            cli_env = os.environ.copy()
            cli_env.update(
                {
                    "SBOL_CREDENTIALS_FILE": str(credentials),
                    "NO_COLOR": "1",
                }
            )
            cli_oauth_login(sbol, origin, browser, cli_env)

            run([str(sbol), "init", "--registry", origin], cwd=project_a, env=cli_env)
            seed = project_a / "seed.ttl"
            seed.write_text(FIXTURE)
            pushed = run(
                [
                    str(sbol),
                    "registry",
                    "push",
                    str(seed),
                    "--id",
                    "loopback",
                    "--name",
                    "Loopback private design",
                ],
                cwd=project_a,
                env=cli_env,
            )
            match = re.search(r"^pushed (\S+)$", pushed.stdout, re.MULTILINE)
            if not match:
                raise RuntimeError(f"could not find created collection URI:\n{pushed.stdout}")
            collection = match.group(1)
            if not (project_a / "sbol.toml").is_file() or not (project_a / "sbol.lock").is_file():
                raise RuntimeError("tracked push did not create both project tracking files")
            design_files = list((project_a / "designs").glob("*.ttl"))
            if len(design_files) != 1:
                raise RuntimeError(f"expected one tracked design file, found {design_files}")
            tracked = design_files[0]
            assert_private(origin, collection)

            run([str(sbol), "init", "--registry", origin], cwd=project_b, env=cli_env)
            run(
                [str(sbol), "registry", "pull", collection],
                cwd=project_b,
                env=cli_env,
            )

            updated = tracked.read_text() + (
                f'\n<{collection}> <http://purl.org/dc/terms/description> '
                '"Updated by CLI loopback test" .\n'
            )
            tracked.write_text(updated)
            before_push = json.loads(
                run([str(sbol), "status", "--json"], cwd=project_a, env=cli_env).stdout
            )
            if before_push[0]["state"] != "push":
                raise RuntimeError(f"local edit was not classified as a push: {before_push}")
            run(
                [str(sbol), "registry", "push", str(tracked)],
                cwd=project_a,
                env=cli_env,
            )

            mcp_token = mcp_access_token(origin, browser)
            initialized = mcp_message(
                origin,
                mcp_token,
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {"protocolVersion": "2025-11-25", "capabilities": {}},
                },
            )
            if initialized["result"]["protocolVersion"] != "2025-11-25":
                raise RuntimeError(f"unexpected MCP negotiation: {initialized}")
            sync_state = mcp_tool(
                origin,
                mcp_token,
                2,
                "get_collection_sync_state",
                {"iri": collection, "format": "turtle"},
            )
            state = sync_state["result"]["structuredContent"]
            mcp_content = state["content"].replace(
                "Updated by CLI loopback test", "Updated by MCP prepared change"
            )
            if mcp_content == state["content"]:
                raise RuntimeError("MCP synchronization content did not contain the CLI update")
            prepared = mcp_tool(
                origin,
                mcp_token,
                3,
                "prepare_collection_update",
                {
                    "iri": collection,
                    "format": "turtle",
                    "expected_content_etag": state["content_etag"],
                    "content": mcp_content,
                },
            )
            plan = prepared["result"]["structuredContent"]["prepared_change"]
            applied = mcp_tool(
                origin,
                mcp_token,
                4,
                "apply_prepared_change",
                {"plan_token": plan["plan_token"]},
            )
            if applied["result"].get("isError"):
                raise RuntimeError(f"prepared MCP update failed: {applied}")
            replay = mcp_tool(
                origin,
                mcp_token,
                5,
                "apply_prepared_change",
                {"plan_token": plan["plan_token"]},
            )
            if not replay["result"].get("isError"):
                raise RuntimeError("prepared MCP token could be replayed")

            remote = json.loads(
                run([str(sbol), "status", "--json"], cwd=project_b, env=cli_env).stdout
            )
            if remote[0]["state"] != "pull":
                raise RuntimeError(f"MCP edit was not classified as a remote pull: {remote}")
            run([str(sbol), "sync"], cwd=project_b, env=cli_env)
            pulled_files = list((project_b / "designs").glob("*.ttl"))
            if len(pulled_files) != 1 or "Updated by MCP prepared change" not in pulled_files[0].read_text():
                raise RuntimeError("second checkout did not receive the prepared MCP update")
            assert_private(origin, collection)
        finally:
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)

    print("machine-access loopback: OAuth, private CLI sync, CAS, and prepared MCP update passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:  # keep CI output compact while preserving cause
        print(f"machine-access loopback failed: {error}", file=sys.stderr)
        raise SystemExit(1)
