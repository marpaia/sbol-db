"""Differential conformance driver.

For each case the driver issues the identical HTTP request to the classic
SynBioHub reference and to each sbol-db subject, then compares the two responses
with the comparator the case's category selects (see compare.py). Auth cases
first mint a token on each side (a subject token is only valid on that subject).
Mutation cases run against a scratch collection and are compared by reading the
post-state back on both sides, never by diffing the mutation response bytes.

The driver talks to `Target`s, which wrap a `requests.Session`. `run_case` takes
those targets as arguments, so it can be exercised against the live stack from
conftest.py or against in-memory fakes in a unit test without any network.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

import requests

import compare

# Category -> (comparator, how to extract the payload each comparator wants).
# The extractor turns a Response into the argument type the comparator expects:
# parsed JSON, decoded text, or raw bytes.
_JSON = lambda r: r.json()  # noqa: E731
_TEXT = lambda r: r.text  # noqa: E731
_BYTES = lambda r: r.content  # noqa: E731

COMPARATORS: Dict[str, tuple] = {
    "html": (lambda a, b: compare.compare_html(a, b), _TEXT),
    "sparql": (lambda a, b: compare.compare_sparql(a, b), _JSON),
    "json": (lambda a, b: compare.compare_json_setequal(a, b), _JSON),
    "sbol": (lambda a, b: compare.compare_rdf(a, b, fmt="xml"), _TEXT),
    "gff": (lambda a, b: compare.compare_gff(a, b), _TEXT),
    "omex": (lambda a, b: compare.compare_omex(a, b), _BYTES),
    "fasta": (lambda a, b: compare.compare_fasta(a, b), _TEXT),
    "genbank": (lambda a, b: compare.compare_genbank(a, b), _TEXT),
    "plaintext": (lambda a, b: compare.compare_plaintext(a, b), _TEXT),
    # Body legitimately differs per side (tokens, HTML-vs-JSON); matching status
    # is the only cross-implementation invariant.
    "status": (lambda a, b: compare.compare_status(a, b), _TEXT),
}


class Target:
    """One HTTP endpoint under test: the reference or a subject backend.

    A thin wrapper over requests.Session that carries an optional per-target
    auth token threaded as `X-authorization`, mirroring how SynBioHub's own
    suite authenticates."""

    def __init__(self, name: str, base: str, is_reference: bool = False):
        self.name = name
        self.base = base.rstrip("/")
        self.is_reference = is_reference
        self.token: Optional[str] = None
        self.session = requests.Session()

    def request(self, method: str, path: str, **kwargs: Any) -> requests.Response:
        headers = dict(kwargs.pop("headers", {}) or {})
        if self.token:
            headers["X-authorization"] = self.token
        headers.setdefault("Accept", "text/plain")
        url = self.base + path
        return self.session.request(method, url, headers=headers, timeout=300, **kwargs)

    def login(self, email: str, password: str) -> str:
        """Mint a token via POST /login and thread it on later requests. The
        token is per-target: a subject's token authenticates only that subject."""
        response = self.session.post(
            self.base + "/login",
            data={"email": email, "password": password},
            headers={"Accept": "text/plain"},
            timeout=300,
        )
        response.raise_for_status()
        self.token = response.text.strip()
        return self.token

    def logout(self) -> None:
        self.token = None

    def load_graph(
        self,
        ntriples: str,
        graph: str,
        auth: Any = None,
        content_type: str = "application/n-triples",
    ) -> None:
        """Replace a named graph over the Virtuoso-compatible graph store
        protocol. Subjects challenge with HTTP Basic; a classic-stack Virtuoso
        target takes digest auth. Raises on a non-2xx write."""
        response = self.session.put(
            self.base + "/sparql-graph-crud-auth/",
            params={"graph-uri": graph},
            data=ntriples.encode("utf-8"),
            headers={"Content-Type": content_type},
            auth=auth,
            timeout=300,
        )
        response.raise_for_status()

    def sparql_ready(self) -> bool:
        try:
            response = self.session.post(
                self.base + "/sparql",
                data={"query": "ASK {}"},
                headers={"Accept": "application/sparql-results+json"},
                timeout=15,
            )
            return response.status_code == 200
        except requests.RequestException:
            return False


@dataclass
class Case:
    """One differential request. `category` selects the comparator; `payload`
    functions map a Response to the comparator's argument."""

    name: str
    category: str
    method: str = "GET"
    path: str = "/"
    params: Optional[Dict[str, Any]] = None
    data: Optional[Dict[str, Any]] = None
    headers: Optional[Dict[str, str]] = None
    auth: bool = False
    # A `multipart/form-data` upload, in requests' `files=` shape
    # (`{field: (filename, bytes, content_type)}`). Classic's `/submit`,
    # `/attach`, and `/icon` take multipart bodies; `data` fields ride alongside
    # the file parts, matching classic's form.
    files: Optional[Dict[str, Any]] = None
    # For mutation cases: after issuing the request, read this GET path back and
    # compare the post-state instead of the mutation response.
    readback: Optional[str] = None
    # The category to use for the readback comparison, when it differs from the
    # mutation's own category (the mutation body is often a plain-text ack while
    # the read-back state is SBOL or JSON).
    readback_category: Optional[str] = None
    # Whether this case mutates server state. The driver does not read this, but
    # a caller can partition a case list into read-only and mutating subsets.
    mutating: bool = False
    # Set when this endpoint is not expected to be byte-equal because the two
    # implementations differ by design: either classic has a defect and sbol-db
    # is correct, or both are valid but differ (native search vs SBOLExplorer,
    # verbatim vs libSBOLj URI minting/denormalization). The value is the reason.
    # Such cases are reported separately from the byte-equal tier: forcing a
    # match would mean replicating a classic bug or abandoning a sound design, so
    # they are neither counted as equal nor as failures.
    expected_divergence: Optional[str] = None

    def issue(self, target: Target) -> requests.Response:
        response = target.request(
            self.method,
            self.path,
            params=self.params,
            data=self.data,
            headers=self.headers,
            files=self.files,
        )
        if self.readback is not None:
            return target.request("GET", self.readback)
        return response

    def compare_category(self) -> str:
        """The category the driver compares under: the readback's category when a
        readback overrides it, otherwise the case's own category."""
        if self.readback is not None and self.readback_category is not None:
            return self.readback_category
        return self.category


@dataclass
class TargetResult:
    target: str
    equal: bool
    detail: str
    status_reference: int
    status_subject: int
    context: Dict[str, Any] = field(default_factory=dict)


@dataclass
class CaseResult:
    case: str
    category: str
    results: List[TargetResult]

    @property
    def passed(self) -> bool:
        return all(r.equal for r in self.results)

    def to_dict(self) -> Dict[str, Any]:
        return {
            "case": self.case,
            "category": self.category,
            "passed": self.passed,
            "targets": [
                {
                    "target": r.target,
                    "equal": r.equal,
                    "status_reference": r.status_reference,
                    "status_subject": r.status_subject,
                    "detail": r.detail if not r.equal else "",
                    "context": r.context,
                }
                for r in self.results
            ],
        }


def compare_responses(category: str, reference: requests.Response, subject: requests.Response) -> compare.Diff:
    """Compare one reference response against one subject response using the
    comparator for the category. A status-code mismatch is a diff on its own."""
    if reference.status_code != subject.status_code:
        return compare.Diff(
            False,
            f"status mismatch: reference={reference.status_code} subject={subject.status_code}",
        )
    if category not in COMPARATORS:
        raise KeyError(f"no comparator for category {category!r}")
    comparator, extract = COMPARATORS[category]
    return comparator(extract(reference), extract(subject))


def run_case(case: Case, reference: Target, subjects: List[Target]) -> CaseResult:
    """Fan a single case out to the reference and every subject and compare."""
    reference_response = case.issue(reference)
    results: List[TargetResult] = []
    category = case.compare_category()
    for subject in subjects:
        subject_response = case.issue(subject)
        diff = compare_responses(category, reference_response, subject_response)
        results.append(
            TargetResult(
                target=subject.name,
                equal=diff.equal,
                detail=diff.detail,
                status_reference=reference_response.status_code,
                status_subject=subject_response.status_code,
                context=diff.context,
            )
        )
    return CaseResult(case=case.name, category=case.category, results=results)


def run_cases(
    cases: List[Case],
    reference: Target,
    subjects: List[Target],
    login: Optional[Callable[[Target], None]] = None,
) -> List[CaseResult]:
    """Run a list of cases. Auth cases mint a token on each target (via `login`)
    before the request and drop it afterward, so tokens never leak across
    cases."""
    outcomes: List[CaseResult] = []
    for case in cases:
        if case.auth and login is not None:
            login(reference)
            for subject in subjects:
                login(subject)
        outcomes.append(run_case(case, reference, subjects))
        if case.auth:
            reference.logout()
            for subject in subjects:
                subject.logout()
    return outcomes
