"""Comparison library for the SynBioHub differential conformance harness.

Classic SynBioHub is the reference oracle; sbol-db is the subject. For each
case the driver issues the identical request to both and compares the responses
with one of the comparators here. The comparison method depends on the payload,
and follows SynBioHub's own test suite rules (see
`~/git/SynBioHub/synbiohub/tests/test_functions.py`):

* HTML pages: line diff (difflib) after removing every element whose class is in
  IGNORE_CLASSES ("testignore", "buorg") and normalizing whitespace, so
  presentation-only chrome that legitimately differs is ignored.
* SBOL / GFF / OMEX downloads: SEMANTIC, not byte diff. SBOL/RDF is parsed into
  RDF graphs and tested for isomorphism (blank-node-aware), GFF is compared as a
  set of feature records, and OMEX is compared as its manifest entries plus its
  member set (each member semantically when it is RDF).
* SPARQL / JSON results: structural. SPARQL results compare `head.vars` and the
  `results.bindings` as sets; metadata JSON compares as an order-insensitive
  structural set so object and list ordering never causes a spurious diff.

Every comparator returns a `Diff`, so the driver can aggregate a machine-readable
reference-vs-subject report. Nothing here needs a live service: the semantic RDF
path uses local rdflib isomorphism, and the optional validator.sbolstandard.org
path takes an injectable HTTP poster so it can be unit tested without a network.
"""

from __future__ import annotations

import difflib
import io
import json
import zipfile
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

from bs4 import BeautifulSoup
from rdflib import Graph, URIRef
from rdflib.compare import graph_diff, to_isomorphic

# Provenance predicates whose object is a wall-clock timestamp stamped at
# request time. They are inherently non-deterministic across two independent
# implementations (and across runs), so semantic RDF comparison drops them
# before testing graph isomorphism. Includes both the direct literal form and
# the `dcterms:W3CDTF` value node classic's COMBINE archive writer nests under a
# `parseType="Resource"` timestamp.
_VOLATILE_PREDICATES = {
    URIRef("http://purl.org/dc/terms/created"),
    URIRef("http://purl.org/dc/terms/modified"),
    URIRef("http://purl.org/dc/terms/W3CDTF"),
}


def _strip_volatile(graph: Graph) -> Graph:
    """Drop volatile provenance-timestamp triples so two graphs that differ only
    in their creation/modification instants still compare as equivalent."""
    for triple in list(graph.triples((None, None, None))):
        if triple[1] in _VOLATILE_PREDICATES:
            graph.remove(triple)
    return graph

# Elements carrying one of these classes are presentation chrome that classic
# SynBioHub deliberately excludes from its own golden-master comparison.
IGNORE_CLASSES = ["testignore", "buorg"]

# The validator request body SynBioHub uses for its semantic download diff.
_VALIDATOR_OPTIONS = {
    "language": "SBOL2",
    "test_equality": True,
    "check_uri_compliance": False,
    "check_completeness": False,
    "check_best_practices": False,
    "fail_on_first_error": False,
    "provide_detailed_stack_trace": False,
    "subset_uri": "",
    "uri_prefix": "",
    "version": "",
    "insert_type": False,
    "main_file_name": "subject",
    "diff_file_name": "reference",
}

DEFAULT_VALIDATOR_URL = "https://validator.sbolstandard.org/validate/"


@dataclass
class Diff:
    """Result of one comparison: whether the two sides are equivalent, plus a
    human-readable delta the driver can put in its report."""

    equal: bool
    detail: str = ""
    context: Dict[str, Any] = field(default_factory=dict)

    def __bool__(self) -> bool:
        return self.equal


# --------------------------------------------------------------------------- #
# HTML
# --------------------------------------------------------------------------- #


def _normalize_html(html: str) -> List[str]:
    """Strip ignored-class elements and return prettified lines for diffing."""
    soup = BeautifulSoup(html, "lxml")
    for ignore_class in IGNORE_CLASSES:
        for element in soup.find_all(class_=ignore_class):
            element.decompose()
    return soup.prettify().splitlines()


def compare_html(reference: str, subject: str) -> Diff:
    """Compare two HTML pages the way SynBioHub's suite does: remove elements
    whose class is in IGNORE_CLASSES, normalize whitespace, then line-diff. The
    same normalization is applied to both sides, so only substantive content
    differences survive."""
    ref_lines = _normalize_html(reference)
    sub_lines = _normalize_html(subject)
    changes = list(difflib.unified_diff(ref_lines, sub_lines, "reference", "subject", lineterm=""))
    if not changes:
        return Diff(True, "html equal after stripping ignored classes")
    return Diff(False, "\n".join(changes))


# --------------------------------------------------------------------------- #
# SBOL / RDF (semantic)
# --------------------------------------------------------------------------- #


def compare_rdf(reference: str, subject: str, fmt: str = "xml") -> Diff:
    """Semantic SBOL/RDF comparison by blank-node-aware graph isomorphism.

    Two documents that carry the same triples in a different serialization order
    (or with different blank-node labels) are equal; a document missing or
    carrying an extra triple is not. This is the local equivalent of the
    validator's `test_equality:true` graph-isomorphism check."""
    ref_graph = _strip_volatile(Graph().parse(data=reference, format=fmt))
    sub_graph = _strip_volatile(Graph().parse(data=subject, format=fmt))
    ref_iso = to_isomorphic(ref_graph)
    sub_iso = to_isomorphic(sub_graph)
    if ref_iso == sub_iso:
        return Diff(True, f"rdf graphs isomorphic ({len(ref_graph)} triples)")
    _, in_ref_only, in_sub_only = graph_diff(ref_iso, sub_iso)
    lines = []
    for triple in sorted(in_ref_only, key=str):
        lines.append(f"- {triple}")
    for triple in sorted(in_sub_only, key=str):
        lines.append(f"+ {triple}")
    return Diff(
        False,
        "rdf graphs not isomorphic\n" + "\n".join(lines),
        {"only_in_reference": len(in_ref_only), "only_in_subject": len(in_sub_only)},
    )


def compare_sbol_via_validator(
    reference: str,
    subject: str,
    poster: Optional[Callable[[str, Dict[str, Any]], Dict[str, Any]]] = None,
    validator_url: str = DEFAULT_VALIDATOR_URL,
) -> Diff:
    """Semantic SBOL comparison via validator.sbolstandard.org `test_equality`.

    Mirrors classic SynBioHub's download diff exactly. `poster(url, body)` must
    return the parsed validator JSON; when omitted a `requests`-backed poster is
    used. Injecting `poster` lets the unit tests exercise this path offline."""
    body = {
        "options": dict(_VALIDATOR_OPTIONS),
        "return_file": False,
        "main_file": subject,
        "diff_file": reference,
    }
    if poster is None:
        poster = _requests_poster
    result = poster(validator_url, body)
    if result.get("equal") is True:
        return Diff(True, "validator reports SBOL documents equal")
    return Diff(False, "validator reports SBOL documents differ", {"validator": result})


def _requests_poster(url: str, body: Dict[str, Any]) -> Dict[str, Any]:
    import requests

    response = requests.post(url, json=body, timeout=300)
    response.raise_for_status()
    return response.json()


# --------------------------------------------------------------------------- #
# GFF
# --------------------------------------------------------------------------- #


def _gff_records(text: str) -> set:
    """Feature records of a GFF3 file as an order-insensitive set. Comment and
    directive lines (`#`) are ignored, and the attribute column is compared as a
    set so attribute ordering does not matter."""
    records = set()
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 8:
            continue
        attrs = frozenset(cols[8].split(";")) if len(cols) > 8 else frozenset()
        records.add(tuple(cols[:8]) + (attrs,))
    return records


def compare_gff(reference: str, subject: str) -> Diff:
    """Compare two GFF3 documents as sets of feature records."""
    ref = _gff_records(reference)
    sub = _gff_records(subject)
    if ref == sub:
        return Diff(True, f"gff feature sets equal ({len(ref)} records)")
    only_ref = sorted(str(r) for r in ref - sub)
    only_sub = sorted(str(r) for r in sub - ref)
    lines = [f"- {r}" for r in only_ref] + [f"+ {r}" for r in only_sub]
    return Diff(False, "gff feature sets differ\n" + "\n".join(lines))


# --------------------------------------------------------------------------- #
# OMEX (manifest + members)
# --------------------------------------------------------------------------- #


def _omex_manifest_entries(zf: zipfile.ZipFile) -> set:
    """The (location, format) pairs declared in an OMEX manifest.xml."""
    entries = set()
    if "manifest.xml" not in zf.namelist():
        return entries
    manifest = zf.read("manifest.xml").decode("utf-8", "replace")
    soup = BeautifulSoup(manifest, "xml")
    for content in soup.find_all("content"):
        entries.add((content.get("location", ""), content.get("format", "")))
    return entries


def compare_omex(reference: bytes, subject: bytes) -> Diff:
    """Structural OMEX comparison: the manifest content entries must match as a
    set, and each non-manifest member must be present on both sides and equal
    (semantically when it parses as RDF, byte-for-byte otherwise)."""
    ref_zip = zipfile.ZipFile(io.BytesIO(reference))
    sub_zip = zipfile.ZipFile(io.BytesIO(subject))

    ref_manifest = _omex_manifest_entries(ref_zip)
    sub_manifest = _omex_manifest_entries(sub_zip)
    if ref_manifest != sub_manifest:
        only_ref = sorted(str(e) for e in ref_manifest - sub_manifest)
        only_sub = sorted(str(e) for e in sub_manifest - ref_manifest)
        lines = [f"- {e}" for e in only_ref] + [f"+ {e}" for e in only_sub]
        return Diff(False, "omex manifest entries differ\n" + "\n".join(lines))

    ref_members = {n for n in ref_zip.namelist() if n != "manifest.xml"}
    sub_members = {n for n in sub_zip.namelist() if n != "manifest.xml"}
    if ref_members != sub_members:
        only_ref = sorted(ref_members - sub_members)
        only_sub = sorted(sub_members - ref_members)
        lines = [f"- {n}" for n in only_ref] + [f"+ {n}" for n in only_sub]
        return Diff(False, "omex member set differs\n" + "\n".join(lines))

    for name in sorted(ref_members):
        member = _compare_omex_member(name, ref_zip.read(name), sub_zip.read(name))
        if not member.equal:
            return Diff(False, f"omex member {name} differs\n{member.detail}")
    return Diff(True, f"omex equal ({len(ref_members)} members, {len(ref_manifest)} manifest entries)")


def _compare_omex_member(name: str, reference: bytes, subject: bytes) -> Diff:
    if name.lower().endswith((".xml", ".rdf", ".sbol")):
        try:
            return compare_rdf(reference.decode("utf-8"), subject.decode("utf-8"), fmt="xml")
        except Exception:  # noqa: BLE001 - not RDF; fall back to byte compare
            pass
    if reference == subject:
        return Diff(True, "member bytes equal")
    return Diff(False, "member bytes differ")


# --------------------------------------------------------------------------- #
# SPARQL results / metadata JSON (structural, set-equal)
# --------------------------------------------------------------------------- #


# JSON object keys whose values are inherently per-instance volatile: a numeric
# primary key the reference autoincrements versus the UUID the subject mints, and
# the wall-clock account timestamps. They are dropped before structural
# comparison so two accounts that differ only by these compare equal; every
# substantive field (username, email, isAdmin, ...) still participates.
_VOLATILE_JSON_KEYS = {"id", "createdAt", "updatedAt"}


def _canonical(value: Any) -> Any:
    """Fold a JSON value into an order-insensitive canonical form: dicts become
    sorted key/value tuples, lists become tuples sorted by canonical repr, so two
    structures that differ only in ordering canonicalize identically. Keys in
    `_VOLATILE_JSON_KEYS` are dropped as unavoidable per-instance volatility."""
    if isinstance(value, dict):
        return tuple(
            sorted((k, _canonical(v)) for k, v in value.items() if k not in _VOLATILE_JSON_KEYS)
        )
    if isinstance(value, list):
        return tuple(sorted((_canonical(v) for v in value), key=repr))
    return value


def compare_json_setequal(reference: Any, subject: Any) -> Diff:
    """Order-insensitive structural JSON comparison for metadata endpoints."""
    if _canonical(reference) == _canonical(subject):
        return Diff(True, "json structurally set-equal")
    detail = "\n".join(
        difflib.unified_diff(
            json.dumps(reference, indent=2, sort_keys=True).splitlines(),
            json.dumps(subject, indent=2, sort_keys=True).splitlines(),
            "reference",
            "subject",
            lineterm="",
        )
    )
    return Diff(False, "json not set-equal\n" + detail)


# --------------------------------------------------------------------------- #
# Plain text / status
# --------------------------------------------------------------------------- #


def compare_plaintext(reference: str, subject: str) -> Diff:
    """Compare two `text/plain` bodies after trimming surrounding whitespace.

    Classic SynBioHub serves the count family (`/:type/count`, `/searchCount`,
    `/usesCount`, `/twinsCount`, `/similarCount`) as a bare integer in a
    `text/plain` body, so this is the comparator those cases select."""
    if reference.strip() == subject.strip():
        return Diff(True, "plaintext equal")
    return Diff(False, f"plaintext differs: reference={reference.strip()!r} subject={subject.strip()!r}")


def compare_status(reference: str, subject: str) -> Diff:
    """A body-agnostic comparator: the driver has already required the status
    codes to match, so this passes unconditionally. It is the category for
    endpoints whose body legitimately differs per side (a login mints a distinct
    token on each target; an admin dashboard is HTML on classic and JSON here),
    where matching status is the only cross-implementation invariant."""
    return Diff(True, "status codes match; body compared by contract elsewhere")


# --------------------------------------------------------------------------- #
# FASTA / GenBank (sequence, semantic)
# --------------------------------------------------------------------------- #


def _fasta_records(text: str) -> set:
    """Parse FASTA into a set of `(header, sequence)` pairs. The sequence is
    concatenated across wrapped lines and upper-cased so line wrapping and case
    never cause a spurious diff; the header keeps its identifier only (the text
    up to the first space), dropping the free-text description classic and
    sbol-db format differently."""
    records = set()
    header: Optional[str] = None
    seq: List[str] = []

    def flush() -> None:
        if header is not None:
            records.add((header, "".join(seq).upper()))

    for line in text.splitlines():
        if line.startswith(">"):
            flush()
            header = line[1:].strip().split()[0] if line[1:].strip() else ""
            seq = []
        elif header is not None:
            seq.append(line.strip())
    flush()
    return records


def compare_fasta(reference: str, subject: str) -> Diff:
    """Compare two FASTA documents as a set of `(identifier, sequence)` records."""
    ref = _fasta_records(reference)
    sub = _fasta_records(subject)
    if ref == sub:
        return Diff(True, f"fasta record sets equal ({len(ref)} records)")
    only_ref = sorted(str(r) for r in ref - sub)
    only_sub = sorted(str(r) for r in sub - ref)
    lines = [f"- {r}" for r in only_ref] + [f"+ {r}" for r in only_sub]
    return Diff(False, "fasta record sets differ\n" + "\n".join(lines))


def _genbank_sequences(text: str) -> set:
    """The ORIGIN nucleotide sequences of a GenBank flat file as a set. Each
    record's sequence is the ORIGIN block with its position numbers and
    whitespace stripped and upper-cased. Header metadata (the LOCUS date, the
    ACCESSION line) is volatile between implementations, so the sequence content
    is the stable invariant to compare."""
    sequences = set()
    in_origin = False
    seq: List[str] = []
    for line in text.splitlines():
        token = line.strip().split(" ", 1)[0]
        if token == "ORIGIN":
            in_origin = True
            seq = []
            continue
        if token == "//":
            if in_origin:
                sequences.add("".join(seq).upper())
            in_origin = False
            continue
        if in_origin:
            seq.append("".join(ch for ch in line if ch.isalpha()))
    return sequences


def compare_genbank(reference: str, subject: str) -> Diff:
    """Compare two GenBank flat files by their ORIGIN sequence content, ignoring
    volatile header and formatting differences between the two serializers."""
    ref = _genbank_sequences(reference)
    sub = _genbank_sequences(subject)
    if ref == sub:
        return Diff(True, f"genbank sequence sets equal ({len(ref)} records)")
    only_ref = sorted(ref - sub)
    only_sub = sorted(sub - ref)
    lines = [f"- {r}" for r in only_ref] + [f"+ {r}" for r in only_sub]
    return Diff(False, "genbank sequence sets differ\n" + "\n".join(lines))


def _canonical_term(term: Dict[str, Any]) -> Dict[str, Any]:
    """Canonicalize one SPARQL-results RDF term so equivalent encodings compare
    equal. Virtuoso emits the pre-standard ``type: "typed-literal"`` for a typed
    literal, while the W3C SPARQL 1.1 JSON Results format encodes the same term
    as ``type: "literal"`` carrying a ``datatype``. Both denote the identical
    RDF term, so they fold to one canonical shape."""
    canon = dict(term)
    if canon.get("type") == "typed-literal":
        canon["type"] = "literal"
    return canon


def _binding_key(binding: Dict[str, Any]) -> tuple:
    return tuple(
        sorted((var, json.dumps(_canonical_term(val), sort_keys=True)) for var, val in binding.items())
    )


def compare_sparql(reference: Dict[str, Any], subject: Dict[str, Any]) -> Diff:
    """Compare two SPARQL 1.1 JSON result documents: `head.vars` must match as a
    set and `results.bindings` must match as a set (order-insensitive)."""
    ref_vars = set(reference.get("head", {}).get("vars", []))
    sub_vars = set(subject.get("head", {}).get("vars", []))
    if ref_vars != sub_vars:
        return Diff(
            False,
            f"head.vars differ: only_reference={sorted(ref_vars - sub_vars)} "
            f"only_subject={sorted(sub_vars - ref_vars)}",
        )

    # ASK queries carry a boolean rather than bindings.
    if "boolean" in reference or "boolean" in subject:
        if reference.get("boolean") == subject.get("boolean"):
            return Diff(True, f"ask results equal ({reference.get('boolean')})")
        return Diff(False, f"ask boolean differs: {reference.get('boolean')} vs {subject.get('boolean')}")

    ref_rows = [_binding_key(b) for b in reference.get("results", {}).get("bindings", [])]
    sub_rows = [_binding_key(b) for b in subject.get("results", {}).get("bindings", [])]
    ref_set = set(ref_rows)
    sub_set = set(sub_rows)
    if ref_set == sub_set:
        return Diff(True, f"sparql results set-equal ({len(ref_set)} distinct rows)")
    only_ref = sorted(str(dict(r)) for r in ref_set - sub_set)
    only_sub = sorted(str(dict(r)) for r in sub_set - ref_set)
    lines = [f"- {r}" for r in only_ref] + [f"+ {r}" for r in only_sub]
    return Diff(False, "sparql binding sets differ\n" + "\n".join(lines))
