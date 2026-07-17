"""Independent full-suite differential runner: reference vs the single subject.

Mints a token on both sides per case (the reference enforces requireLogin) and
runs the read-only, mutating, account, and admin groups, printing a per-case
pass/fail table and a per-group + overall summary.
"""

import sys

import cases as case_defs
from conformance import Target, run_case

REFERENCE = "http://localhost:17777"
SUBJECT = "http://127.0.0.1:18903"
EMAIL = "test@user.synbiohub"
PASSWORD = "test"


def login(target: Target) -> None:
    target.login(EMAIL, PASSWORD)


GROUPS = [
    ("read_only", case_defs.read_only_cases),
    ("mutating", case_defs.mutating_cases),
    ("account", case_defs.account_cases),
    ("admin", case_defs.admin_cases),
]


def main() -> int:
    only = sys.argv[1] if len(sys.argv) > 1 else None
    reference = Target("reference", REFERENCE, is_reference=True)
    subject = Target("subject-sqlite", SUBJECT)

    grand_eq = grand_tot = 0
    for gname, gfn in GROUPS:
        if only and gname != only:
            continue
        selected = gfn()
        eq = 0
        print(f"\n===== group: {gname} ({len(selected)} cases) =====")
        for case in selected:
            login(reference)
            login(subject)
            try:
                r = run_case(case, reference, [subject])
                tr = r.results[0]
                ok = tr.equal
                detail = tr.detail
                rs, ss = tr.status_reference, tr.status_subject
            except Exception as exc:  # noqa: BLE001
                ok = False
                detail = f"exception: {exc}"
                rs = ss = "?"
            mark = "ok " if ok else "XX "
            if ok:
                eq += 1
            print(f"{mark}{case.name:32s} [{case.category:9s}] ref={rs} sub={ss}")
            if not ok:
                print("      " + detail.replace("\n", "\n      ")[:700])
        print(f"--- {gname}: {eq}/{len(selected)} equal ---")
        grand_eq += eq
        grand_tot += len(selected)
    print(f"\n===== OVERALL: {grand_eq}/{grand_tot} equal =====")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
