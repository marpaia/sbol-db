"""Run the full differential suite (read-only + mutating) and print a per-case
pass/fail table with a summary. Reference vs the single native subject. The
byte-equal tier: every V1 endpoint except /similar and /similarCount (see
docs/similar-explorer-gap.md).
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

    # The full byte-equal surface: read-only, then the ordered mutating
    # sequence, then account-lifecycle and admin cases. /similar and
    # /similarCount are not in cases.py (see docs/similar-explorer-gap.md).
    selected = (
        case_defs.read_only_cases()
        + case_defs.mutating_cases()
        + case_defs.account_cases()
        + case_defs.admin_cases()
    )
    eq = 0
    divergences = []
    tier = 0
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
        # A documented classic defect is reported outside the byte-equal tier:
        # the subject deliberately does not replicate the bug.
        if case.expected_divergence is not None:
            divergences.append((case, rs, ss))
            print(
                f"DIV {case.name:32s} [{case.category:9s}] ref={rs} sub={ss}"
                f"  ({case.expected_divergence})"
            )
            continue
        tier += 1
        mark = "ok " if ok else "XX "
        if ok:
            eq += 1
        print(f"{mark}{case.name:32s} [{case.category:9s}] ref={rs} sub={ss}")
        if not ok:
            print("      " + detail.replace("\n", "\n      ")[: int(sys.argv[1]) if len(sys.argv) > 1 else 900])
    print(f"\n===== {eq}/{tier} equal (byte-equal tier) =====")
    if divergences:
        print(f"===== {len(divergences)} documented divergence(s) reported separately =====")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
