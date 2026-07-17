"""Dump reference vs subject bodies for a path, to eyeball divergences."""
import json
import sys
import urllib.error
import urllib.request

import seed_both as s

REF = "http://localhost:17777"
SUB = "http://127.0.0.1:18903"


def get(base, path, tok, accept):
    req = urllib.request.Request(base + path, headers={"Accept": accept, "X-authorization": tok})
    try:
        r = urllib.request.urlopen(req, timeout=60)
        return r.status, r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")


def main():
    path = sys.argv[1]
    accept = sys.argv[2] if len(sys.argv) > 2 else "application/json"
    rt = s._login(REF)
    stk = s._login(SUB)
    rs, rb = get(REF, path, rt, accept)
    ss, sb = get(SUB, path, stk, accept)
    for name, st, body in [("REFERENCE", rs, rb), ("SUBJECT", ss, sb)]:
        print(f"===== {name} status={st} =====")
        try:
            print(json.dumps(json.loads(body), indent=2, sort_keys=True))
        except Exception:
            print(body[:3000])


if __name__ == "__main__":
    main()
