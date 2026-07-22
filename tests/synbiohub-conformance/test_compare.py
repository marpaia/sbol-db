"""Unit tests for the conformance comparison library.

These need no live services: the semantic RDF path uses local rdflib
isomorphism and the validator path takes an injected poster. Run with:

    tests/synbiohub-conformance/.venv/bin/pytest tests/synbiohub-conformance/test_compare.py
"""

from __future__ import annotations

import io
import zipfile

import compare

# --------------------------------------------------------------------------- #
# SBOL / RDF semantic isomorphism
# --------------------------------------------------------------------------- #

# The same three triples, serialized in two different statement orders. A
# semantic comparison must treat these as equal.
SBOL_A = """
@prefix ex: <http://example.org/> .
ex:c1 a ex:Component ;
    ex:displayId "c1" ;
    ex:sequence ex:s1 .
ex:s1 a ex:Sequence ;
    ex:elements "atcg" .
"""

SBOL_B = """
@prefix ex: <http://example.org/> .
ex:s1 ex:elements "atcg" ;
    a ex:Sequence .
ex:c1 ex:sequence ex:s1 ;
    a ex:Component ;
    ex:displayId "c1" .
"""

# SBOL_A minus the ex:elements triple: a missing triple must compare NOT equal.
SBOL_MISSING = """
@prefix ex: <http://example.org/> .
ex:c1 a ex:Component ;
    ex:displayId "c1" ;
    ex:sequence ex:s1 .
ex:s1 a ex:Sequence .
"""


def test_isomorphic_reordered_sbol_is_equal():
    result = compare.compare_rdf(SBOL_A, SBOL_B, fmt="turtle")
    assert result.equal, result.detail


def test_missing_triple_is_not_equal():
    result = compare.compare_rdf(SBOL_A, SBOL_MISSING, fmt="turtle")
    assert not result.equal
    assert result.context["only_in_reference"] == 1


def test_blank_node_relabeling_is_equal():
    # Same graph shape, different blank-node labels: isomorphism must ignore the
    # labels and report equal.
    doc_a = '@prefix ex: <http://example.org/> . ex:c1 ex:has _:x . _:x ex:v "1" .'
    doc_b = '@prefix ex: <http://example.org/> . ex:c1 ex:has _:y . _:y ex:v "1" .'
    assert compare.compare_rdf(doc_a, doc_b, fmt="turtle").equal


# --------------------------------------------------------------------------- #
# SPARQL / JSON results
# --------------------------------------------------------------------------- #


def _binding(s, o):
    return {"s": {"type": "uri", "value": s}, "o": {"type": "literal", "value": o}}


def test_reordered_bindings_are_equal():
    ref = {
        "head": {"vars": ["s", "o"]},
        "results": {"bindings": [_binding("http://a", "1"), _binding("http://b", "2")]},
    }
    subj = {
        "head": {"vars": ["o", "s"]},
        "results": {"bindings": [_binding("http://b", "2"), _binding("http://a", "1")]},
    }
    result = compare.compare_sparql(ref, subj)
    assert result.equal, result.detail


def test_differing_binding_set_is_not_equal():
    ref = {
        "head": {"vars": ["s", "o"]},
        "results": {"bindings": [_binding("http://a", "1"), _binding("http://b", "2")]},
    }
    subj = {
        "head": {"vars": ["s", "o"]},
        "results": {"bindings": [_binding("http://a", "1"), _binding("http://c", "3")]},
    }
    assert not compare.compare_sparql(ref, subj).equal


def test_differing_head_vars_is_not_equal():
    ref = {"head": {"vars": ["s", "o"]}, "results": {"bindings": []}}
    subj = {"head": {"vars": ["s"]}, "results": {"bindings": []}}
    assert not compare.compare_sparql(ref, subj).equal


def test_ask_results():
    ref = {"head": {}, "boolean": True}
    assert compare.compare_sparql(ref, {"head": {}, "boolean": True}).equal
    assert not compare.compare_sparql(ref, {"head": {}, "boolean": False}).equal


def test_json_setequal_order_insensitive():
    ref = [{"uri": "a", "name": "A"}, {"uri": "b", "name": "B"}]
    subj = [{"name": "B", "uri": "b"}, {"name": "A", "uri": "a"}]
    assert compare.compare_json_setequal(ref, subj).equal


def test_json_setequal_detects_difference():
    ref = [{"uri": "a", "name": "A"}]
    subj = [{"uri": "a", "name": "CHANGED"}]
    assert not compare.compare_json_setequal(ref, subj).equal


# --------------------------------------------------------------------------- #
# HTML difflib with testignore stripping
# --------------------------------------------------------------------------- #


def test_html_differing_only_in_testignore_is_equal():
    ref = "<html><body><h1>Part</h1>" '<div class="testignore">build 123 at 10:00</div></body></html>'
    subj = "<html><body><h1>Part</h1>" '<div class="testignore">build 999 at 23:59</div></body></html>'
    result = compare.compare_html(ref, subj)
    assert result.equal, result.detail


def test_html_differing_in_real_content_is_not_equal():
    ref = "<html><body><h1>Part A</h1></body></html>"
    subj = "<html><body><h1>Part B</h1></body></html>"
    assert not compare.compare_html(ref, subj).equal


def test_html_buorg_class_also_stripped():
    ref = '<html><body><p>x</p><div class="buorg">upgrade your browser</div></body></html>'
    subj = "<html><body><p>x</p></body></html>"
    assert compare.compare_html(ref, subj).equal


# --------------------------------------------------------------------------- #
# GFF
# --------------------------------------------------------------------------- #

GFF_A = """##gff-version 3
chr1\tsbol\tCDS\t1\t100\t.\t+\t0\tID=f1;Name=one
chr1\tsbol\tpromoter\t200\t300\t.\t+\t.\tID=f2
"""

# Same two features, lines reversed and attribute order flipped.
GFF_B = """##gff-version 3
chr1\tsbol\tpromoter\t200\t300\t.\t+\t.\tID=f2
chr1\tsbol\tCDS\t1\t100\t.\t+\t0\tName=one;ID=f1
"""

GFF_MISSING = """##gff-version 3
chr1\tsbol\tCDS\t1\t100\t.\t+\t0\tID=f1;Name=one
"""


def test_gff_reordered_is_equal():
    assert compare.compare_gff(GFF_A, GFF_B).equal


def test_gff_missing_feature_is_not_equal():
    assert not compare.compare_gff(GFF_A, GFF_MISSING).equal


# --------------------------------------------------------------------------- #
# OMEX
# --------------------------------------------------------------------------- #

MANIFEST = """<?xml version="1.0" encoding="UTF-8"?>
<omexManifest xmlns="http://identifiers.org/combine.specifications/omex-manifest">
  <content location="." format="http://identifiers.org/combine.specifications/omex"/>
  <content location="model.xml" format="http://sbols.org/v2"/>
</omexManifest>
"""


def _omex(members):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as zf:
        zf.writestr("manifest.xml", MANIFEST)
        for name, data in members.items():
            zf.writestr(name, data)
    return buffer.getvalue()


def test_omex_semantically_equal_members():
    # The SBOL member is serialized in two triple orders; the archive comparison
    # must treat the two OMEX files as equal.
    xml_a = compare.Graph().parse(data=SBOL_A, format="turtle").serialize(format="xml")
    xml_b = compare.Graph().parse(data=SBOL_B, format="turtle").serialize(format="xml")
    assert compare.compare_omex(_omex({"model.xml": xml_a}), _omex({"model.xml": xml_b})).equal


def test_omex_extra_member_is_not_equal():
    xml_a = compare.Graph().parse(data=SBOL_A, format="turtle").serialize(format="xml")
    ref = _omex({"model.xml": xml_a})
    subj = _omex({"model.xml": xml_a, "extra.txt": "surprise"})
    assert not compare.compare_omex(ref, subj).equal


# --------------------------------------------------------------------------- #
# Validator-backed semantic path (poster injected, no network)
# --------------------------------------------------------------------------- #


def test_validator_equal_via_injected_poster():
    captured = {}

    def poster(url, body):
        captured["url"] = url
        captured["body"] = body
        return {"equal": True}

    result = compare.compare_sbol_via_validator(SBOL_A, SBOL_B, poster=poster)
    assert result.equal
    assert captured["body"]["options"]["test_equality"] is True
    assert captured["body"]["main_file"] == SBOL_B
    assert captured["body"]["diff_file"] == SBOL_A


def test_validator_not_equal_via_injected_poster():
    result = compare.compare_sbol_via_validator(SBOL_A, SBOL_MISSING, poster=lambda url, body: {"equal": False})
    assert not result.equal
