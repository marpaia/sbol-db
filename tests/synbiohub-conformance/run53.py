"""Run the full 53-case differential suite (read-only + mutating) and print a
per-case pass/fail table with a summary. Reference vs the single native subject.
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


def main() -> int:
    reference = Target("reference", REFERENCE, is_reference=True)
    subject = Target("subject-sqlite", SUBJECT)

    selected = case_defs.all_cases()
    eq = 0
    for case in selected:
        if case.auth:
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
        if case.auth:
            reference.logout()
            subject.logout()
        mark = "ok " if ok else "XX "
        if ok:
            eq += 1
        print(f"{mark}{case.name:32s} [{case.category:9s}] ref={rs} sub={ss}")
        if not ok:
            print("      " + detail.replace("\n", "\n      ")[: int(sys.argv[1]) if len(sys.argv) > 1 else 900])
    print(f"\n===== {eq}/{len(selected)} equal =====")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
