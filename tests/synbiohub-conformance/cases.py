"""The V1 SynBioHub differential case list.

Each `Case` issues one identical request to the classic SynBioHub reference and
to every sbol-db subject; the driver diffs the two responses with the comparator
its `category` selects (see compare.py / conformance.py). This module enumerates
every V1 endpoint the sbol-db adapter serves
(`crates/sbol-db-server/src/synbiohub/`) and mirrors classic's request shape and
its response contract (`~/git/SynBioHub/synbiohub/lib/app.js` + `lib/api`,
`lib/views`, `lib/actions`).

Classic response contracts that fix the comparison category:

* The count family (`/:type/count`, `/searchCount`, `/usesCount`, `/twinsCount`,
  `/similarCount`) is a bare integer in a `text/plain` body -> `plaintext`.
* The metadata / collection query family (`<uri>/metadata`, `/rootCollections`,
  `<uri>/subCollections`) and the ranked query family
  (`/search`, `<uri>/uses`, `<uri>/twins`, `<uri>/similar`) is a JSON array of
  row objects from `sparql.queryJson` / the search view's non-HTML branch ->
  `json`.
* `/sparql` is SPARQL 1.1 results JSON on both sides -> `sparql`.
* Downloads: `/sbol` + `/sbolnr` are RDF/XML -> `sbol`; `/gff` -> `gff`;
  `/fasta` -> `fasta`; `/gb` -> `genbank`; `/omex` -> `omex`; `/summary` is a
  JSON document -> `json`.
* Auth bodies (`/login` token, `/register` + `/logout` + `/resetPassword` acks)
  differ per side by design, so they compare on status only -> `status`; the
  account read-back (`GET /profile`) is JSON -> `json`.
* Mutations return a plain-text ack (or, on sbol-db today, JSON), so a mutating
  case never diffs the mutation body: it reads the post-state back on both sides
  and diffs that (`readback` + `readback_category`).

Grouping: `read_only_cases()` never touches server state; `mutating_cases()`
runs a submit-then-edit-then-read sequence against a scratch collection and MUST
run in order; `admin_cases()` needs an administrator token on both sides.
`all_cases()` is read-only followed by mutating.

The read-only cases key on two objects seeded into the public graph: the
`smoke-corpus.nt` Collection (`smoke_collection`) and its member
ComponentDefinition (`pSmoke`, which carries a Sequence, so it converts to
FASTA/GenBank/GFF). The mutating cases key on a scratch collection the seed
submits into `testuser`'s namespace.
"""

from __future__ import annotations

from pathlib import Path
from typing import List

from conformance import Case

CORPUS_PATH = Path(__file__).resolve().parent / "fixtures" / "smoke-corpus.nt"

# The seeded public object and collection, addressed by the V1 path grammar
# `/public/<collectionId>/<displayId>/<version>`.
OBJECT_PATH = "/public/smoke/pSmoke/1"
COLLECTION_PATH = "/public/smoke/smoke_collection/1"

# The scratch collection the mutating suite submits into `testuser`'s namespace
# and then edits. `SCRATCH_ID` is the submission id; the submission mints a
# Collection at `<id>_collection` whose member is the uploaded object.
SCRATCH_ID = "scratch"
SCRATCH_COLLECTION = f"/user/testuser/{SCRATCH_ID}/{SCRATCH_ID}_collection/1"
SCRATCH_OBJECT = f"/user/testuser/{SCRATCH_ID}/pScratch/1"
SCRATCH_COLLECTION_URI = f"http://synbiohub.org/user/testuser/{SCRATCH_ID}/{SCRATCH_ID}_collection/1"
SCRATCH_OBJECT_URI = f"http://synbiohub.org/user/testuser/{SCRATCH_ID}/pScratch/1"

# The single-object SBOL2 document the scratch submit uploads. A minimal
# compliant ComponentDefinition so the submit succeeds on libSBOLj (reference)
# and sbol-rs (subject) alike.
SCRATCH_SBOL = """<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:sbol="http://sbols.org/v2#"
         xmlns:dcterms="http://purl.org/dc/terms/">
  <sbol:ComponentDefinition rdf:about="http://examples.org/ComponentDefinition/pScratch/1">
    <sbol:displayId>pScratch</sbol:displayId>
    <sbol:persistentIdentity rdf:resource="http://examples.org/ComponentDefinition/pScratch"/>
    <sbol:version>1</sbol:version>
    <dcterms:title>Scratch part</dcterms:title>
    <sbol:type rdf:resource="http://www.biopax.org/release/biopax-level3.owl#DnaRegion"/>
    <sbol:role rdf:resource="http://identifiers.org/so/SO:0000804"/>
  </sbol:ComponentDefinition>
</rdf:RDF>
"""

# The credentials the seed configures on both sides (see seed_both.py): the
# reference setup admin and the subject's registered `testuser` share them, so
# one login form authenticates both targets.
LOGIN_EMAIL = "test@user.synbiohub"
LOGIN_PASSWORD = "test"

_JSON = {"Accept": "application/json"}
_PLAIN = {"Accept": "text/plain"}
_SPARQL_JSON = {"Accept": "application/sparql-results+json"}


def load_corpus() -> str:
    """The N-Triples corpus every read-only run seeds into the public graph."""
    return CORPUS_PATH.read_text(encoding="utf-8")


# --------------------------------------------------------------------------- #
# Read-only: the ES-independent triplestore subset (kept for the partial-stack
# differential in test_differential_subset.py).
# --------------------------------------------------------------------------- #


def read_subset_cases() -> List[Case]:
    """The read/metadata/SPARQL/download cases that answer straight from the
    triplestore, so they run even when SBOLExplorer / Elasticsearch is down."""
    return [
        Case("object-metadata", "json", path=f"{OBJECT_PATH}/metadata", headers=_JSON),
        Case("collection-metadata", "json", path=f"{COLLECTION_PATH}/metadata", headers=_JSON),
        Case("componentdefinition-count", "plaintext", path="/ComponentDefinition/count", headers=_PLAIN),
        Case("collection-count", "plaintext", path="/Collection/count", headers=_PLAIN),
        Case("root-collections", "json", path="/rootCollections", headers=_JSON),
        Case(
            "sparql-ask",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": "ASK {}"},
            headers=_SPARQL_JSON,
        ),
        Case(
            "sparql-count",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": ("SELECT (COUNT(*) AS ?c) " "FROM <http://synbiohub.org/public> WHERE { ?s ?p ?o }")},
            headers=_SPARQL_JSON,
        ),
        Case("sbol-download", "sbol", path=f"{OBJECT_PATH}/sbol"),
        Case("gff-download", "gff", path=f"{OBJECT_PATH}/gff"),
    ]


# --------------------------------------------------------------------------- #
# Read-only: auth
# --------------------------------------------------------------------------- #


def auth_read_cases() -> List[Case]:
    """Auth endpoints that do not mutate state: minting a token and reading the
    caller's own profile. `/login` mints a distinct token per target, so it
    compares on status; `GET /profile` returns the account as JSON."""
    return [
        Case(
            "login",
            "status",
            method="POST",
            path="/login",
            data={"email": LOGIN_EMAIL, "password": LOGIN_PASSWORD},
            headers=_PLAIN,
        ),
        Case(
            "login-bad-credentials",
            "status",
            method="POST",
            path="/login",
            data={"email": LOGIN_EMAIL, "password": "wrong"},
            headers=_PLAIN,
        ),
        Case("profile", "json", path="/profile", headers=_JSON, auth=True),
        Case("logout", "status", method="POST", path="/logout", headers=_PLAIN, auth=True),
    ]


# --------------------------------------------------------------------------- #
# Read-only: query
# --------------------------------------------------------------------------- #


def query_cases() -> List[Case]:
    """The V1 query surface: free-text and faceted search, counts, the count
    family, root/sub collections, object relations (uses/twins/similar),
    metadata, and raw SPARQL. Classic serves counts as `text/plain` integers and
    everything else as a JSON array (`sparql.queryJson` / the search view's
    non-HTML branch); `/sparql` is SPARQL results JSON on both."""
    return [
        # Free-text and faceted search.
        Case("search-empty", "json", path="/search", headers=_JSON),
        Case("search-freetext", "json", path="/search/plasmid", headers=_JSON),
        Case(
            "search-faceted-type",
            "json",
            path="/search/objectType=ComponentDefinition",
            headers=_JSON,
        ),
        Case("search-count-empty", "plaintext", path="/searchCount", headers=_PLAIN),
        Case("search-count-freetext", "plaintext", path="/searchCount/plasmid", headers=_PLAIN),
        # Type counts (text/plain integer on classic).
        Case("componentdefinition-count", "plaintext", path="/ComponentDefinition/count", headers=_PLAIN),
        Case("collection-count", "plaintext", path="/Collection/count", headers=_PLAIN),
        Case("sequence-count", "plaintext", path="/Sequence/count", headers=_PLAIN),
        # Collection navigation.
        Case("root-collections", "json", path="/rootCollections", headers=_JSON),
        Case("sub-collections", "json", path=f"{COLLECTION_PATH}/subCollections", headers=_JSON),
        # Object relations.
        Case("uses", "json", path=f"{OBJECT_PATH}/uses", headers=_JSON),
        Case("uses-count", "plaintext", path=f"{OBJECT_PATH}/usesCount", headers=_PLAIN),
        Case("twins", "json", path=f"{OBJECT_PATH}/twins", headers=_JSON),
        Case("twins-count", "plaintext", path=f"{OBJECT_PATH}/twinsCount", headers=_PLAIN),
        Case("similar", "json", path=f"{OBJECT_PATH}/similar", headers=_JSON),
        Case("similar-count", "plaintext", path=f"{OBJECT_PATH}/similarCount", headers=_PLAIN),
        # Metadata.
        Case("object-metadata", "json", path=f"{OBJECT_PATH}/metadata", headers=_JSON),
        Case("collection-metadata", "json", path=f"{COLLECTION_PATH}/metadata", headers=_JSON),
        # Identity-scoped listings (require a token; empty for a fresh account).
        Case("manage", "json", path="/manage", headers=_JSON, auth=True),
        Case("shared", "json", path="/shared", headers=_JSON, auth=True),
        # Raw SPARQL.
        Case(
            "sparql-ask",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": "ASK {}"},
            headers=_SPARQL_JSON,
        ),
        Case(
            "sparql-select-count",
            "sparql",
            method="POST",
            path="/sparql",
            data={"query": ("SELECT (COUNT(*) AS ?c) " "FROM <http://synbiohub.org/public> WHERE { ?s ?p ?o }")},
            headers=_SPARQL_JSON,
        ),
    ]


# --------------------------------------------------------------------------- #
# Read-only: downloads
# --------------------------------------------------------------------------- #


def download_cases() -> List[Case]:
    """The object closure rendered in each exchange format. RDF formats compare
    by graph isomorphism, sequence formats as record sets, OMEX by manifest +
    members, and `/summary` as structural JSON. The `?version=sbol2` variant
    exercises the RDF version negotiation the download routes honor."""
    return [
        Case("sbol", "sbol", path=f"{OBJECT_PATH}/sbol"),
        Case("sbol-version-sbol2", "sbol", path=f"{OBJECT_PATH}/sbol", params={"version": "sbol2"}),
        Case("sbolnr", "sbol", path=f"{OBJECT_PATH}/sbolnr"),
        Case("gff", "gff", path=f"{OBJECT_PATH}/gff"),
        Case("fasta", "fasta", path=f"{OBJECT_PATH}/fasta"),
        Case("genbank", "genbank", path=f"{OBJECT_PATH}/gb"),
        Case("omex", "omex", path=f"{OBJECT_PATH}/omex"),
        Case("summary", "json", path=f"{OBJECT_PATH}/summary", headers=_JSON),
    ]


def read_only_cases() -> List[Case]:
    """Every case that leaves server state untouched: auth reads, the query
    surface, and the download surface."""
    return auth_read_cases() + query_cases() + download_cases()


# --------------------------------------------------------------------------- #
# Mutating: submission -> edit -> permission, verified by read-back.
# --------------------------------------------------------------------------- #


def _scratch_file() -> dict:
    """The `requests` multipart `files=` mapping for the scratch SBOL upload."""
    return {"file": ("scratch.xml", SCRATCH_SBOL.encode("utf-8"), "application/xml")}


def mutating_cases() -> List[Case]:
    """The write surface, ordered so each case's precondition is the prior
    case's post-state: submit a scratch collection, edit its fields and
    membership, grant/revoke an owner, attach an external URL, then tear it down
    with replace/removeCollection/remove. Every case reads the post-state back
    on both sides and diffs that, never the mutation's own (per-side) body.

    Runs against `testuser`'s namespace, so every case needs an authenticated
    token (`auth=True`). The submit and attach cases carry multipart bodies.
    """
    return [
        # Submit the scratch collection; read the minted collection's SBOL back.
        Case(
            "submit",
            "status",
            method="POST",
            path="/submit",
            data={
                "id": SCRATCH_ID,
                "version": "1",
                "name": "Scratch",
                "description": "conformance scratch collection",
                "citations": "",
                "overwrite_merge": "0",
            },
            files=_scratch_file(),
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_COLLECTION}/sbol",
            readback_category="sbol",
        ),
        # Mutable text fields (uri in body).
        Case(
            "update-mutable-description",
            "status",
            method="POST",
            path="/updateMutableDescription",
            data={"uri": SCRATCH_OBJECT_URI, "value": "an edited description"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        Case(
            "update-mutable-notes",
            "status",
            method="POST",
            path="/updateMutableNotes",
            data={"uri": SCRATCH_OBJECT_URI, "value": "an edited note"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        Case(
            "update-mutable-source",
            "status",
            method="POST",
            path="/updateMutableSource",
            data={"uri": SCRATCH_OBJECT_URI, "value": "https://example.org/source"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        Case(
            "update-citations",
            "status",
            method="POST",
            path="/updateCitations",
            data={"uri": SCRATCH_OBJECT_URI, "value": "12345678"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        # Generic field edit/add/remove (uri in path, verb + :field).
        Case(
            "edit-title",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/edit/title",
            data={"object": "Edited Scratch Title"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        Case(
            "add-role",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/add/role",
            data={"object": "http://identifiers.org/so/SO:0000167"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/sbol",
            readback_category="sbol",
        ),
        Case(
            "remove-role",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/remove/role",
            data={"object": "http://identifiers.org/so/SO:0000167"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/sbol",
            readback_category="sbol",
        ),
        Case(
            "add-annotation",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/add/annotation",
            data={"object": "annotation value", "pred": "http://example.org/terms/note"},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/sbol",
            readback_category="sbol",
        ),
        # Collection membership.
        Case(
            "remove-membership",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/removeMembership",
            data={"member": SCRATCH_OBJECT_URI},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_COLLECTION}/metadata",
            readback_category="json",
        ),
        Case(
            "add-to-collection",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/addToCollection",
            data={"member": SCRATCH_OBJECT_URI},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_COLLECTION}/metadata",
            readback_category="json",
        ),
        # Object sharing (no read-back surface for the ACL; verify by status).
        Case(
            "add-owner",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/addOwner",
            data={"user": LOGIN_EMAIL},
            auth=True,
            mutating=True,
        ),
        Case(
            "remove-owner",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/removeOwner/testuser",
            auth=True,
            mutating=True,
        ),
        # Attach an external URL, then confirm it appears on the object.
        Case(
            "attach-url",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/attachURL",
            data={
                "url": "https://example.org/data.txt",
                "name": "data.txt",
                "type": "http://purl.org/NET/mediatypes/text/plain",
            },
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        # Attach an uploaded file (multipart), then confirm the blob downloads.
        Case(
            "attach-file",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/attach",
            data={"id": "attachment"},
            files={"file": ("data.txt", b"conformance attachment payload\n", "text/plain")},
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="json",
        ),
        # Publish the scratch collection, then read the public copy's SBOL.
        Case(
            "make-public",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/makePublic",
            data={"id": SCRATCH_ID, "version": "1", "tabState": "new"},
            auth=True,
            mutating=True,
            readback=f"/public/{SCRATCH_ID}/{SCRATCH_ID}_collection/1/sbol",
            readback_category="sbol",
        ),
        # Tear-down: removing the object leaves an identical 404 on both sides.
        Case(
            "replace",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/replace",
            auth=True,
            mutating=True,
        ),
        Case(
            "remove-collection",
            "status",
            method="POST",
            path=f"{SCRATCH_COLLECTION}/removeCollection",
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_COLLECTION}/metadata",
            readback_category="status",
        ),
        Case(
            "remove",
            "status",
            method="POST",
            path=f"{SCRATCH_OBJECT}/remove",
            auth=True,
            mutating=True,
            readback=f"{SCRATCH_OBJECT}/metadata",
            readback_category="status",
        ),
    ]


# --------------------------------------------------------------------------- #
# Account lifecycle mutations (create/reset). Kept out of the ordered scratch
# sequence because they touch the identity store rather than the corpus.
# --------------------------------------------------------------------------- #


def account_cases() -> List[Case]:
    """Account-lifecycle writes: public signup, password reset request, and the
    set-new-password consume. Each returns a per-side ack, so they compare on
    status. `register` uses a throwaway username; a second run collides
    identically on both sides."""
    return [
        Case(
            "register",
            "status",
            method="POST",
            path="/register",
            data={
                "username": "conformance_tmp",
                "name": "Conformance Temp",
                "email": "conformance_tmp@example.org",
                "affiliation": "",
                "password1": "s3cret-passphrase",
                "password2": "s3cret-passphrase",
            },
            headers=_PLAIN,
            mutating=True,
        ),
        Case(
            "reset-password",
            "status",
            method="POST",
            path="/resetPassword",
            data={"email": LOGIN_EMAIL},
            headers=_PLAIN,
            mutating=True,
        ),
        Case(
            "set-new-password",
            "status",
            method="POST",
            path="/setNewPassword",
            data={"token": "not-a-real-reset-link", "password1": "x", "password2": "x"},
            headers=_PLAIN,
            mutating=True,
        ),
        Case(
            "update-profile",
            "status",
            method="POST",
            path="/profile",
            data={"name": "Test User Edited", "affiliation": "Conformance Lab"},
            headers=_PLAIN,
            auth=True,
            mutating=True,
        ),
    ]


# --------------------------------------------------------------------------- #
# Admin (representative). Requires an administrator token on BOTH targets.
# --------------------------------------------------------------------------- #


def admin_cases() -> List[Case]:
    """A representative slice of the admin surface. The bodies differ by design
    (classic renders HTML, sbol-db returns JSON), so these compare on status:
    the invariant is that an administrator reaches the endpoint identically on
    both sides. Every case needs an admin token (`auth=True`)."""
    return [
        Case("admin-dashboard", "status", path="/admin", auth=True),
        Case("admin-graphs", "status", path="/admin/graphs", auth=True),
        Case(
            "admin-sparql",
            "status",
            path="/admin/sparql",
            params={"query": "ASK {}"},
            auth=True,
        ),
        Case("admin-registries", "status", path="/admin/registries", auth=True),
        Case("admin-remotes", "status", path="/admin/remotes", auth=True),
        Case("admin-plugins", "status", path="/admin/plugins", auth=True),
    ]


def all_cases() -> List[Case]:
    """Read-only cases first, then the ordered mutating sequence."""
    return read_only_cases() + mutating_cases()
