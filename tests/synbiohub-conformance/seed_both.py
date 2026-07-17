#!/usr/bin/env python3
"""Seed the reference (classic SynBioHub) and a sbol-db subject with an
identical corpus so the differential conformance driver can compare them.

Proven recipe (verified working on this host):
  * Reference image is ``synbiohub/synbiohub:snapshot-standalone`` (the
    ``1.6.1-standalone`` image crashes in ``/setup``). Bring it up with
    ``docker compose up -d virtuoso synbiohub``.
  * First-time ``/setup`` creates admin ``test@user.synbiohub`` / ``test`` and,
    critically, sets ``uriPrefix=http://synbiohub.org/`` so the reference mints
    the same top-level URIs sbol-db does. Both sides therefore mint identical
    URIs and responses compare directly, with no base-URI canonicalization.
  * The subject runs natively from the current build (not the stale published
    image), with ``SBOL_DB_ALLOW_PUBLIC_SIGNUP=true`` so ``testuser`` can
    register. The submission username must match on both sides (``testuser``)
    so private URIs line up.

Usage:
  python3 seed_both.py --reference http://localhost:17777 \
      --subject http://127.0.0.1:18903 --corpus <dir-of-sbol2-xml>
"""
from __future__ import annotations

import argparse
import io
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ADMIN_EMAIL = "test@user.synbiohub"
ADMIN_PW = "test"
USERNAME = "testuser"
URI_PREFIX = "http://synbiohub.org/"

SETUP = {
    "userName": USERNAME,
    "userFullName": "Test User",
    "userEmail": ADMIN_EMAIL,
    "userPassword": ADMIN_PW,
    "userPasswordConfirm": ADMIN_PW,
    "instanceName": "Conformance SynBioHub",
    "instanceUrl": "http://localhost:17777/",
    "uriPrefix": URI_PREFIX,
    "color": "#D25627",
    "frontPageText": "text",
    "virtuosoINI": "/etc/virtuoso-opensource-7/virtuoso.ini",
    "virtuosoDB": "/var/lib/virtuoso-opensource-7/db",
    "allowPublicSignup": "true",
    "requireLogin": "false",
}


def _post_form(base: str, path: str, fields: dict, token: str | None = None) -> tuple[int, str]:
    data = urllib.parse.urlencode(fields).encode()
    headers = {"Accept": "text/plain", "Content-Type": "application/x-www-form-urlencoded"}
    if token:
        headers["X-authorization"] = token
    req = urllib.request.Request(base + path, data=data, headers=headers)
    try:
        r = urllib.request.urlopen(req, timeout=180)
        return r.status, r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")


def _login(base: str) -> str:
    _, body = _post_form(base, "/login", {"email": ADMIN_EMAIL, "password": ADMIN_PW})
    return body.strip()


def _submit(base: str, token: str, coll_id: str, path: Path) -> tuple[int, str]:
    boundary = "----conformseed"
    body = io.BytesIO()
    for name, value in [
        ("id", coll_id),
        ("version", "1"),
        ("name", coll_id),
        ("description", f"{coll_id} conformance corpus"),
        ("citations", ""),
        ("overwrite_merge", "0"),
    ]:
        body.write(
            f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
        )
    body.write(
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; '
        f'filename="{path.name}"\r\nContent-Type: application/xml\r\n\r\n'.encode()
    )
    body.write(path.read_bytes())
    body.write(f"\r\n--{boundary}--\r\n".encode())
    req = urllib.request.Request(
        base + "/submit",
        data=body.getvalue(),
        headers={
            "Accept": "text/plain",
            "X-authorization": token,
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
    )
    try:
        r = urllib.request.urlopen(req, timeout=180)
        return r.status, r.read()[:120].decode(errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read()[:200].decode(errors="replace")


def _ensure_reference_setup(base: str) -> None:
    # /setup is idempotent-enough: once done, the instance stops redirecting.
    try:
        with urllib.request.urlopen(base + "/setup", timeout=10) as r:
            if r.status == 200 and _login(base):
                return
    except Exception:
        pass
    status, _ = _post_form(base, "/setup", SETUP)
    print(f"reference /setup -> {status}")


def _ensure_subject_user(base: str) -> None:
    _post_form(
        base,
        "/register",
        {
            "username": USERNAME,
            "name": "Test User",
            "affiliation": "x",
            "email": ADMIN_EMAIL,
            "password1": ADMIN_PW,
            "password2": ADMIN_PW,
        },
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--reference", default="http://localhost:17777")
    ap.add_argument("--subject", default="http://127.0.0.1:18903")
    ap.add_argument("--corpus", required=True, help="directory of SBOL2 .xml files")
    args = ap.parse_args()

    files = sorted(Path(args.corpus).glob("*.xml"))
    if not files:
        print(f"no .xml files under {args.corpus}", file=sys.stderr)
        return 2

    _ensure_reference_setup(args.reference)
    ref_tok = _login(args.reference)
    _ensure_subject_user(args.subject)
    subj_tok = _login(args.subject)
    if not ref_tok or not subj_tok:
        print("could not obtain both tokens", file=sys.stderr)
        return 1

    for path in files:
        coll_id = path.stem.lower().replace("-", "_").replace(".", "_")
        rs, rb = _submit(args.reference, ref_tok, coll_id, path)
        ss, sb = _submit(args.subject, subj_tok, coll_id, path)
        print(f"{path.name}: reference={rs} subject={ss}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
