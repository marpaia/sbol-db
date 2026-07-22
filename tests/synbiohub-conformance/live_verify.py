"""Ad-hoc live differential runner: reference vs the single native subject.

Mints a token on both sides per auth case (the reference enforces requireLogin
on public reads), runs a chosen case group, and prints a per-case pass/fail
table plus a summary. Not a pytest; a manual verification aid for the fix pass.
"""

import sys

import cases as case_defs
from conformance import Target, run_cases

REFERENCE = "http://localhost:17777"
SUBJECT = "http://127.0.0.1:18903"
EMAIL = "test@user.synbiohub"
PASSWORD = "test"


def login(target: Target) -> None:
    target.login(EMAIL, PASSWORD)


def main() -> int:
    group = sys.argv[1] if len(sys.argv) > 1 else "read"
    reference = Target("reference", REFERENCE, is_reference=True)
    subject = Target("subject-sqlite", SUBJECT)

    groups = {
        "read": case_defs.read_only_cases,
        "query": case_defs.query_cases,
        "auth": case_defs.auth_read_cases,
        "download": case_defs.download_cases,
        "mutating": case_defs.mutating_cases,
    }
    selected = groups[group]()

    # The reference instance enforces requireLogin, so it 401s unauthenticated
    # public reads. Thread a token on every case (not just auth cases) so body
    # shapes are actually compared rather than masked by a 401/200 status split.
    equal = 0
    from conformance import run_case

    for case in selected:
        # Mint a fresh token on both sides before every case: the reference
        # enforces requireLogin (so unauthenticated reads 401), and the /logout
        # case would otherwise invalidate a shared token for later cases.
        login(reference)
        login(subject)
        try:
            r = run_case(case, reference, [subject])
            tr = r.results[0]
            ok = tr.equal
            detail = tr.detail
            rs, ss = tr.status_reference, tr.status_subject
        except Exception as exc:  # noqa: BLE001 - report, don't abort the run
            ok = False
            detail = f"exception: {exc}"
            rs = ss = "?"
        mark = "ok " if ok else "XX "
        if ok:
            equal += 1
        print(f"{mark}{case.name:32s} [{case.category:9s}] ref={rs} sub={ss}")
        if not ok:
            print("      " + detail.replace("\n", "\n      ")[:600])
    print(f"\n{equal}/{len(selected)} equal ({group})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
