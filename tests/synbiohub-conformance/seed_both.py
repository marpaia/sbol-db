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
import sqlite3
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
    # The setup form's booleans are HTML checkboxes: a field is "on" whenever it
    # is present, regardless of value. Send `allowPublicSignup` to enable signup;
    # omit `requireLogin` entirely so anonymous public reads return 200 (matching
    # the subject) rather than 401.
    "allowPublicSignup": "true",
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


def _is_token(value: str) -> bool:
    """A real login mints a short opaque token; an unconfigured instance serves
    the HTML setup/login page instead, so reject anything with markup."""
    return bool(value) and "<" not in value and len(value) < 256


def _make_public(base: str, token: str, coll_id: str) -> tuple[int, str]:
    """Publish a freshly submitted user collection into the public store so the
    read-only cases can address it under ``/public/<id>/<id>_collection/1``."""
    path = f"/user/{USERNAME}/{coll_id}/{coll_id}_collection/1/makePublic"
    return _post_form(base, path, {"id": coll_id, "version": "1", "tabState": "new"}, token)


def _promote_subject_admin(db_path: str) -> None:
    """Grant the subject's ``testuser`` the admin/curator flags the reference's
    setup admin carries, so ``/profile`` reports the same privileges on both
    sides. The subject registers via public signup (always non-admin), so the
    flags are set directly in its identity store."""
    connection = sqlite3.connect(db_path)
    try:
        connection.execute(
            "UPDATE sbh_user SET is_admin = 1, is_curator = 1, is_member = 1 WHERE username = ?",
            (USERNAME,),
        )
        connection.commit()
    finally:
        connection.close()


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
    # Once /setup has run, /login mints a real token; before then it serves the
    # setup page. Only re-run setup when a real login is not yet possible.
    if _is_token(_login(base)):
        return
    status, _ = _post_form(base, "/setup", SETUP)
    print(f"reference /setup -> {status}")


def _ensure_subject_user(base: str) -> None:
    _post_form(
        base,
        "/register",
        {
            "username": USERNAME,
            "name": "Test User",
            "affiliation": "",
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
    ap.add_argument(
        "--subject-db",
        default="/tmp/sbol-db-subject.sqlite",
        help="path to the subject's SQLite store, promoted so testuser is admin",
    )
    args = ap.parse_args()

    files = sorted(Path(args.corpus).glob("*.xml"))
    if not files:
        print(f"no .xml files under {args.corpus}", file=sys.stderr)
        return 2

    _ensure_reference_setup(args.reference)
    ref_tok = _login(args.reference)
    _ensure_subject_user(args.subject)
    _promote_subject_admin(args.subject_db)
    subj_tok = _login(args.subject)
    if not _is_token(ref_tok) or not _is_token(subj_tok):
        print("could not obtain both tokens", file=sys.stderr)
        return 1

    for path in files:
        coll_id = path.stem.lower().replace("-", "_").replace(".", "_")
        rs, _ = _submit(args.reference, ref_tok, coll_id, path)
        ss, _ = _submit(args.subject, subj_tok, coll_id, path)
        rp, _ = _make_public(args.reference, ref_tok, coll_id)
        sp, _ = _make_public(args.subject, subj_tok, coll_id)
        print(f"{path.name}: submit reference={rs} subject={ss}; makePublic reference={rp} subject={sp}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
