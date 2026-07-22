"""Run the ordered mutating sequence on both sides and dump the make-public
readback closures fully so the divergence can be root-caused."""
import os

import cases as case_defs
from conformance import Target
from rdflib import Graph
from rdflib.compare import graph_diff, to_isomorphic

REF = "http://localhost:17777"
SUB = "http://127.0.0.1:18903"


def login(t):
    t.login("test@user.synbiohub", "test")


def main():
    reference = Target("reference", REF, is_reference=True)
    subject = Target("subject", SUB)
    for case in case_defs.mutating_cases():
        if case.name == "make-public":
            login(reference)
            login(subject)
            rr = case.issue(reference)
            sr = case.issue(subject)
            print("ref status", rr.status_code, "sub status", sr.status_code)
            rg = Graph().parse(data=rr.text, format="xml")
            sg = Graph().parse(data=sr.text, format="xml")
            _, ronly, sonly = graph_diff(to_isomorphic(rg), to_isomorphic(sg))
            print("=== ONLY IN REFERENCE ===")
            for t in sorted(ronly, key=str):
                print(t)
            print("=== ONLY IN SUBJECT ===")
            for t in sorted(sonly, key=str):
                print(t)
            break
        else:
            if case.auth:
                login(reference)
                login(subject)
            case.issue(reference)
            case.issue(subject)


if __name__ == "__main__":
    main()
