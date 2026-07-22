#!/usr/bin/env python3
"""Correct known SBOLExplorer/SynBioHub indexing defects in the conformance
reference, in place, inside the running ``explorer`` container.

The differential suite treats classic SynBioHub as the reference of record, so
the reference must itself be correct: where classic or SBOLExplorer emits garbage
we fix the reference rather than teach sbol-db to reproduce the bug. Each patch
below targets a specific upstream defect and is idempotent, so ``cycle.sh`` can
re-run it on every bring-up.

The script edits ``/SBOLExplorer/flask/search.py`` and
``/SBOLExplorer/flask/cluster.py`` and exits non-zero if an expected anchor is
missing (a signal the pinned image changed and the patch needs review).

Defects corrected:

1. Placeholder search projection (search.py). Classic SynBioHub's Java
   ``PrepareSubmissionJob.incrementallyUpdateSBOLExplorer`` pushes freshly
   submitted top levels to Elasticsearch with ``type = "TODO"`` hardcoded and no
   ``sbol2:type`` field at all. SBOLExplorer's ``create_bindings`` trusts that
   ``_source`` verbatim, so an empty-string ``/search`` surfaces ``type:"TODO"``
   rows and a null ``sbolType`` for every incrementally indexed part. The fix
   resolves the real RDF type and ``sbol2:type`` from the triplestore (the same
   source the full-reindex path and the criteria-search path already use), so
   the projection carries real values.

2. vsearch ``-sort length`` (cluster.py). ``-cluster_fast`` sorts by length
   internally; vsearch rejects a redundant explicit ``-sort length`` and aborts
   the whole clustering run, leaving ``/similar`` with empty clusters. usearch
   tolerated the flag, so it was never caught upstream.

3. Unclusterable sequences (cluster.py). vsearch aborts a ``-cluster_fast`` run
   when the FASTA holds an empty sequence, a gap character (``-``/``.``), or
   non-nucleotide (degenerate/protein) residues. Skipping those sequences lets
   the reference cluster exactly the nucleotide parts sbol-db's native aligner
   clusters.
"""
from __future__ import annotations

import sys

SEARCH = "/SBOLExplorer/flask/search.py"
CLUSTER = "/SBOLExplorer/flask/cluster.py"

# --- search.py: authoritative type / sbol2:type projection ----------------- #

SEARCH_ANCHOR_SETUP = (
    "    bindings = []\n"
    "    cluster_duplicates = set()\n"
    "\n"
    "    allowed_subjects_set = set(allowed_subjects) if allowed_subjects else None\n"
)
SEARCH_PATCH_SETUP = SEARCH_ANCHOR_SETUP + (
    "\n"
    "    # Classic SynBioHub's incremental indexer pushes freshly submitted parts\n"
    "    # to Elasticsearch with type='TODO' and no sbol2:type, so the ES _source\n"
    "    # is not authoritative for those fields. Resolve them from the triplestore,\n"
    "    # matching the criteria-search path (create_criteria_bindings).\n"
    "    authoritative = {p['subject']: p for p in query.query_parts(parse_allowed_graphs(allowed_graphs))}\n"
)

SEARCH_ANCHOR_CALL = (
    "        binding = create_binding(\n"
    "            subject,\n"
    "            _source.get('displayId'),\n"
    "            _source.get('version'),\n"
    "            _source.get('name'),\n"
    "            _source.get('description'),\n"
    "            _source.get('type'),\n"
    "            _source.get('role'),\n"
    "            _source.get('sboltype'),\n"
    "            _score\n"
    "        )\n"
)
SEARCH_PATCH_CALL = (
    "        _auth = authoritative.get(subject, {})\n"
    "        binding = create_binding(\n"
    "            subject,\n"
    "            _source.get('displayId'),\n"
    "            _source.get('version'),\n"
    "            _source.get('name'),\n"
    "            _source.get('description'),\n"
    "            _auth.get('type', _source.get('type')),\n"
    "            _source.get('role'),\n"
    "            _auth.get('sboltype', _source.get('sboltype')),\n"
    "            _score\n"
    "        )\n"
)

SEARCH_SENTINEL = "authoritative = {p['subject']: p for p in query.query_parts(parse_allowed_graphs(allowed_graphs))}"

# --- cluster.py: drop -sort length ----------------------------------------- #

CLUSTER_ANCHOR_SORT = (
    "    args = [usearch_binary_filename, '-cluster_fast', sequences_filename, "
    "'-id', uclust_identity, '-sort', 'length', '-uc', uclust_results_filename]"
)
CLUSTER_PATCH_SORT = (
    "    args = [usearch_binary_filename, '-cluster_fast', sequences_filename, "
    "'-id', uclust_identity, '-uc', uclust_results_filename]"
)

# --- cluster.py: skip unclusterable sequences ------------------------------ #

CLUSTER_ANCHOR_FASTA = (
    "def write_fasta(sequences):\n"
    "    with open(sequences_filename, 'w') as f:\n"
    "        for sequence in sequences:\n"
    "            f.write(f\">{sequence['subject']}\\n{sequence['sequence']}\\n\")\n"
)
CLUSTER_PATCH_FASTA = (
    "def _is_clusterable_sequence(elements):\n"
    "    # vsearch -cluster_fast aborts on empty sequences, gap characters\n"
    "    # ('-'/'.'), and non-nucleotide (degenerate/protein) residues. Only emit\n"
    "    # non-empty IUPAC nucleotide sequences so the reference clusters the same\n"
    "    # parts sbol-db's native aligner does.\n"
    "    if not elements:\n"
    "        return False\n"
    "    seq = elements.strip().upper()\n"
    "    if not seq:\n"
    "        return False\n"
    "    return all(base in 'ACGTURYSWKMBDHVN' for base in seq)\n"
    "\n"
    "def write_fasta(sequences):\n"
    "    with open(sequences_filename, 'w') as f:\n"
    "        for sequence in sequences:\n"
    "            if not _is_clusterable_sequence(sequence.get('sequence')):\n"
    "                continue\n"
    "            f.write(f\">{sequence['subject']}\\n{sequence['sequence']}\\n\")\n"
)


def _patch_file(path: str, edits: list[tuple[str, str, str]]) -> list[str]:
    """Apply (sentinel, anchor, replacement) edits to ``path``.

    Each edit is skipped when ``sentinel`` is already present (idempotent) and
    fails hard when the anchor is missing on an unpatched file.
    """
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    notes = []
    for sentinel, anchor, replacement in edits:
        if sentinel in text:
            notes.append(f"{path}: already patched ({sentinel[:32]}...)")
            continue
        if anchor not in text:
            print(f"[ERROR] {path}: anchor not found; image may have changed:\n{anchor[:120]}", file=sys.stderr)
            raise SystemExit(2)
        text = text.replace(anchor, replacement, 1)
        notes.append(f"{path}: applied ({sentinel[:32]}...)")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    return notes


def main() -> int:
    notes = []
    notes += _patch_file(
        SEARCH,
        [
            (SEARCH_SENTINEL, SEARCH_ANCHOR_SETUP, SEARCH_PATCH_SETUP),
            ("_auth = authoritative.get(subject, {})", SEARCH_ANCHOR_CALL, SEARCH_PATCH_CALL),
        ],
    )
    notes += _patch_file(
        CLUSTER,
        [
            ("'-id', uclust_identity, '-uc', uclust_results_filename]", CLUSTER_ANCHOR_SORT, CLUSTER_PATCH_SORT),
            ("def _is_clusterable_sequence(elements):", CLUSTER_ANCHOR_FASTA, CLUSTER_PATCH_FASTA),
        ],
    )
    for note in notes:
        print(note)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
