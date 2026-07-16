//! Classic SynBioHub SPARQL templates, rebuilt as typed string builders.
//!
//! These reproduce the query shapes in `synbiohub/sparql/*.sparql` so the
//! SPARQL engine (and its accelerator) recognizes them and emits the same
//! `head.vars`/`results.bindings` JSON classic did. The caller's authorized
//! [`GraphScope`](sbol_db_sparql::GraphScope) is enforced by the engine, so no
//! `FROM` clause is injected: naming a graph the caller cannot read is a scope
//! decision, not a query-text one.

use sbol_db_app::{DateField, FacetedSearch};

/// The prefix block classic prepends to `search.sparql`; carried on every
/// template so a facet predicate curie resolves the same way.
const PREFIXES: &str = "\
PREFIX sbol2: <http://sbols.org/v2#>
PREFIX dcterms: <http://purl.org/dc/terms/>
PREFIX ncbi: <http://www.ncbi.nlm.nih.gov#>
PREFIX synbiohub: <http://synbiohub.org#>
PREFIX sbh: <http://wiki.synbiohub.org/wiki/Terms/synbiohub#>
PREFIX igem: <http://wiki.synbiohub.org/wiki/Terms/igem#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
PREFIX prov: <http://www.w3.org/ns/prov#>
PREFIX dc: <http://purl.org/dc/elements/1.1/>
PREFIX cello: <http://cellocad.org/Terms/cello#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX purl: <http://purl.obolibrary.org/obo/>
PREFIX biopax: <http://www.biopax.org/release/biopax-level3.owl#>
PREFIX so: <http://identifiers.org/so/>
PREFIX bench: <http://wiki.synbiohub.org/wiki/Terms/benchling#>
PREFIX genbank: <http://www.ncbi.nlm.nih.gov/genbank#>
";

/// The `Count.sparql` shape: distinct top-level objects of one SBOL2 type. The
/// short name is substituted after being sanitized to a bare local name.
pub fn count(type_short_name: &str) -> String {
    let ty = sanitize_local_name(type_short_name);
    format!(
        "{PREFIXES}\nSELECT (COUNT(DISTINCT ?subject) AS ?count) WHERE {{\n    ?subject a sbol2:{ty} .\n}}"
    )
}

/// `RootCollectionMetadata.sparql`: every Collection that is not a member of
/// another Collection.
pub fn root_collections() -> String {
    format!(
        "{PREFIXES}
SELECT ?Collection ?name ?description ?displayId ?version WHERE {{
    ?Collection a sbol2:Collection .
    FILTER NOT EXISTS {{ ?otherCollection sbol2:member ?Collection }}
    OPTIONAL {{ ?Collection dcterms:title ?name . }}
    OPTIONAL {{ ?Collection sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?Collection dcterms:description ?description . }}
    OPTIONAL {{ ?Collection sbol2:version ?version }}
}}"
    )
}

/// `SubCollectionMetadata.sparql`: the Collections directly under `parent_uri`.
pub fn sub_collections(parent_uri: &str) -> String {
    let parent = iri(parent_uri);
    format!(
        "{PREFIXES}
SELECT ?Collection ?name ?description ?displayId ?version WHERE {{
    ?Collection a sbol2:Collection .
    {parent} sbol2:member ?Collection .
    OPTIONAL {{ ?Collection dcterms:title ?name . }}
    OPTIONAL {{ ?Collection sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?Collection dcterms:description ?description . }}
    OPTIONAL {{ ?Collection sbol2:version ?version }}
    FILTER NOT EXISTS {{ {parent} sbol2:member ?otherCollection . ?otherCollection sbol2:member ?Collection }}
}}"
    )
}

/// `GetTopLevelMetadata.sparql`: the metadata of one object.
pub fn metadata(uri: &str) -> String {
    let subject = iri(uri);
    format!(
        "{PREFIXES}
SELECT DISTINCT ?persistentIdentity ?displayId ?version ?name ?description ?type WHERE {{
    {subject} a ?type .
    OPTIONAL {{ {subject} sbol2:persistentIdentity ?persistentIdentity . }}
    OPTIONAL {{ {subject} sbol2:displayId ?displayId . }}
    OPTIONAL {{ {subject} sbol2:version ?version . }}
    OPTIONAL {{ {subject} dcterms:title ?name . }}
    OPTIONAL {{ {subject} dcterms:description ?description . }}
}}"
    )
}

/// The metadata of an explicit set of subjects, backing `/shared`. An empty
/// set yields an empty `VALUES` block and so no rows.
pub fn metadata_of(subjects: &[String]) -> String {
    let values = subjects
        .iter()
        .map(|s| iri(s))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{PREFIXES}
SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type WHERE {{
    VALUES ?subject {{ {values} }}
    ?subject a ?type .
    OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?subject sbol2:version ?version . }}
    OPTIONAL {{ ?subject dcterms:title ?name . }}
    OPTIONAL {{ ?subject dcterms:description ?description . }}
}}"
    )
}

/// `findOwnedBy.sparql`: the top-level objects owned by a user, backing
/// `/manage`. `user_uri` is the caller's user graph IRI.
pub fn owned_by(user_uri: &str) -> String {
    let owner = iri(user_uri);
    format!(
        "{PREFIXES}
SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type WHERE {{
    ?subject sbh:ownedBy {owner} .
    ?subject sbh:topLevel ?subject .
    ?subject a ?type .
    OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?subject sbol2:version ?version . }}
    OPTIONAL {{ ?subject dcterms:title ?name . }}
    OPTIONAL {{ ?subject dcterms:description ?description . }}
}}"
    )
}

/// The `search.sparql` criteria for `/uses`: objects that reference `uri`
/// directly or through one intermediate, excluding the `sbh:topLevel` marker.
pub fn uses(uri: &str, count_only: bool) -> String {
    let target = iri(uri);
    let criteria = format!(
        "{{ ?subject ?p {target} }} UNION {{ ?subject ?p ?use . ?use ?useP {target} }} .\n    \
         FILTER(?useP != <http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel>)"
    );
    search_template(&criteria, count_only, None, None)
}

/// The `search.sparql` criteria for `/twins`: other ComponentDefinitions whose
/// Sequence shares `uri`'s `sbol2:elements`.
pub fn twins(uri: &str, count_only: bool) -> String {
    let target = iri(uri);
    let criteria = format!(
        "?subject sbol2:sequence ?seq .\n    \
         ?seq sbol2:elements ?elements .\n    \
         {target} a sbol2:ComponentDefinition .\n    \
         {target} sbol2:sequence ?seq2 .\n    \
         ?seq2 sbol2:elements ?elements2 .\n    \
         FILTER(?subject != {target} && ?elements = ?elements2)"
    );
    search_template(&criteria, count_only, None, None)
}

/// A purely faceted `search.sparql` query built from a [`FacetedSearch`]. Used
/// when the query carries no free text, so relevance ranking is not involved.
pub fn faceted(query: &FacetedSearch, count_only: bool) -> String {
    let criteria = faceted_criteria(query);
    let (limit, offset) = if count_only {
        (None, None)
    } else {
        (Some(query.effective_limit()), Some(query.offset))
    };
    search_template(&criteria, count_only, limit, offset)
}

/// Build the SPARQL criteria block for a faceted query, mirroring `lucene()`.
fn faceted_criteria(query: &FacetedSearch) -> String {
    let mut criteria = String::new();

    if let Some(class) = &query.class {
        criteria.push_str(&format!("    {} a {} .\n", "?subject", iri(class)));
    }
    if let Some(collection) = &query.collection_member {
        criteria.push_str(&format!(
            "    {} sbol2:member ?subject .\n",
            iri(collection)
        ));
    }
    for (predicate, object) in &query.predicate_eq {
        criteria.push_str(&format!(
            "    ?subject {} {} .\n",
            term(predicate),
            term(object)
        ));
    }
    if let Some(field) = query.date_field {
        let (var, predicate) = match field {
            DateField::Created => ("?cdate", "dcterms:created"),
            DateField::Modified => ("?mdate", "dcterms:modified"),
        };
        criteria.push_str(&format!("    ?subject {predicate} {var} .\n"));
        if let Some(before) = &query.date_before {
            criteria.push_str(&format!(
                "    FILTER (xsd:dateTime({var}) <= \"{before}T23:59:59Z\"^^xsd:dateTime)\n"
            ));
        }
        if let Some(after) = &query.date_after {
            criteria.push_str(&format!(
                "    FILTER (xsd:dateTime({var}) >= \"{after}T00:00:00Z\"^^xsd:dateTime)\n"
            ));
        }
    }
    criteria
}

/// The shared `search.sparql` / `searchCount.sparql` body, parameterized by the
/// criteria block and paging. `count_only` selects the aggregate form.
fn search_template(
    criteria: &str,
    count_only: bool,
    limit: Option<usize>,
    offset: Option<usize>,
) -> String {
    if count_only {
        return format!(
            "{PREFIXES}
SELECT (COUNT(DISTINCT ?subject) AS ?count) WHERE {{
    {criteria}
    ?subject a ?type .
    ?subject sbh:topLevel ?subject .
    OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?subject sbol2:version ?version . }}
    OPTIONAL {{ ?subject dcterms:title ?name . }}
    OPTIONAL {{ ?subject dcterms:description ?description . }}
}}"
        );
    }

    let limit = limit.map(|l| format!("\nLIMIT {l}")).unwrap_or_default();
    let offset = offset
        .filter(|o| *o > 0)
        .map(|o| format!("\nOFFSET {o}"))
        .unwrap_or_default();
    format!(
        "{PREFIXES}
SELECT DISTINCT ?subject ?displayId ?version ?name ?description ?type ?sbolType ?role WHERE {{
    {criteria}
    ?subject a ?type .
    ?subject sbh:topLevel ?subject .
    OPTIONAL {{ ?subject sbol2:displayId ?displayId . }}
    OPTIONAL {{ ?subject sbol2:version ?version . }}
    OPTIONAL {{ ?subject dcterms:title ?name . }}
    OPTIONAL {{ ?subject dcterms:description ?description . }}
    OPTIONAL {{ ?subject sbol2:type ?sbolType . FILTER(STRSTARTS(str(?sbolType),'http://www.biopax.org/release/biopax-level3.owl')) }}
    OPTIONAL {{ ?subject sbol2:role ?role . FILTER(STRSTARTS(str(?role),'http://identifiers.org/so/')) }}
}}{limit}{offset}"
    )
}

/// Wrap a full IRI as a SPARQL IRI reference.
fn iri(value: &str) -> String {
    format!("<{value}>")
}

/// Render a wire term as SPARQL: a full IRI as `<iri>`, anything else (a
/// prefixed name or a literal) verbatim, matching `lucene()`'s handling.
fn term(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        iri(value)
    } else {
        value.to_owned()
    }
}

/// Reduce a type short name to a bare local name so it cannot inject SPARQL.
/// Keeps only characters valid in an SBOL2 class name.
fn sanitize_local_name(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}
