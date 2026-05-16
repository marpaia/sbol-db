use std::collections::BTreeMap;

use sbol::{Document, Iri, Resource, SbolClass, Term, Triple};
use sbol_db_core::{IriString, ObjectSummary};
use serde_json::{json, Value};

use crate::hash::content_hash;
use crate::vocab::{RDF_TYPE, SBOL_ROLE, SBOL_TYPE};

/// Per-object slice of the document graph. The summary feeds `sbol_objects`,
/// the `triples` feed `sbol_quads`.
#[derive(Clone, Debug)]
pub struct ObjectQuads {
    pub summary: ObjectSummary,
    pub triples: Vec<Triple>,
}

/// Build one `ObjectQuads` per top-level + typed SBOL object in the document.
///
/// Objects whose identity is a blank node are skipped because the schema's
/// `sbol_iri` domain rejects `_:b...` — their triples still land in `sbol_quads`
/// via [`document_to_quads`](crate::document_to_quads).
pub fn document_to_summaries(doc: &Document) -> Vec<ObjectQuads> {
    let triples_by_subject = group_triples_by_subject(doc);
    let mut out = Vec::with_capacity(doc.typed_objects().len());

    for obj in doc.typed_objects() {
        let identity = obj.identity();
        let iri = match identity.as_iri() {
            Some(iri) => iri,
            None => continue,
        };
        let triples = triples_by_subject
            .get(identity)
            .cloned()
            .unwrap_or_default();
        let summary = build_summary(obj, iri, &triples);
        out.push(ObjectQuads { summary, triples });
    }
    out
}

fn group_triples_by_subject(doc: &Document) -> BTreeMap<Resource, Vec<Triple>> {
    let mut map: BTreeMap<Resource, Vec<Triple>> = BTreeMap::new();
    for triple in doc.rdf_graph().triples() {
        map.entry(triple.subject.clone())
            .or_default()
            .push(triple.clone());
    }
    map
}

fn build_summary(obj: &sbol::SbolObject, iri: &Iri, triples: &[Triple]) -> ObjectSummary {
    let display_id = identified_field(obj, |o| o.display_id().map(str::to_owned));
    let name = identified_field(obj, |o| o.name().map(str::to_owned));
    let description = identified_field(obj, |o| o.description().map(str::to_owned));

    let types = collect_iris_for_predicate(triples, SBOL_TYPE);
    let roles = collect_iris_for_predicate(triples, SBOL_ROLE);

    ObjectSummary {
        iri: IriString::unchecked(iri.as_str()),
        sbol_class: class_to_iri(obj.class()),
        display_id,
        name,
        description,
        types,
        roles,
        data: triples_to_json(iri.as_str(), triples),
        content_hash: content_hash(triples),
    }
}

fn class_to_iri(class: SbolClass) -> String {
    class.iri().to_owned()
}

/// Read identified-trait fields from `SbolObject` without unwrapping every
/// variant manually. Uses prelude trait methods.
fn identified_field<F>(obj: &sbol::SbolObject, f: F) -> Option<String>
where
    F: Fn(&dyn IdentifiedView) -> Option<String>,
{
    use sbol::prelude::SbolIdentified;

    macro_rules! by_variant {
        ($($variant:ident),+ $(,)?) => {
            match obj {
                $(sbol::SbolObject::$variant(o) => f(&IdentifiedAdapter(o as &dyn SbolIdentified)),)+
                _ => None,
            }
        };
    }

    by_variant!(
        Attachment,
        Collection,
        CombinatorialDerivation,
        Component,
        ComponentReference,
        Constraint,
        Cut,
        EntireSequence,
        Experiment,
        ExperimentalData,
        ExternallyDefined,
        Implementation,
        Interaction,
        Interface,
        LocalSubComponent,
        Model,
        Participation,
        Range,
        Sequence,
        SequenceFeature,
        SubComponent,
        VariableFeature,
        Activity,
        Agent,
        Association,
        Plan,
        Usage,
        Measure,
        Unit,
        SingularUnit,
        CompoundUnit,
        UnitDivision,
        UnitExponentiation,
        UnitMultiplication,
        PrefixedUnit,
        Prefix,
        SIPrefix,
        BinaryPrefix,
        IdentifiedExtension,
    )
}

/// Thin trait so the dispatcher above can call the same set of accessors
/// regardless of the concrete typed struct.
trait IdentifiedView {
    fn display_id(&self) -> Option<&str>;
    fn name(&self) -> Option<&str>;
    fn description(&self) -> Option<&str>;
}

struct IdentifiedAdapter<'a>(&'a dyn sbol::prelude::SbolIdentified);

impl IdentifiedView for IdentifiedAdapter<'_> {
    fn display_id(&self) -> Option<&str> {
        self.0.display_id()
    }
    fn name(&self) -> Option<&str> {
        self.0.name()
    }
    fn description(&self) -> Option<&str> {
        self.0.description()
    }
}

fn collect_iris_for_predicate(triples: &[Triple], predicate: &str) -> Vec<String> {
    triples
        .iter()
        .filter(|t| t.predicate.as_str() == predicate)
        .filter_map(|t| match &t.object {
            Term::Resource(Resource::Iri(iri)) => Some(iri.as_str().to_owned()),
            _ => None,
        })
        .collect()
}

/// Encode the per-object triple slice as a compact JSON property bag suitable
/// for round-trip and inspection. This is *not* JSON-LD; it's a deterministic
/// JSON serialization keyed by predicate IRI. JSON-LD round-trip is a future
/// enhancement when the upstream `sbol-rdf` exposes a per-resource exporter.
fn triples_to_json(subject_iri: &str, triples: &[Triple]) -> Value {
    let mut props: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut rdf_types: Vec<String> = Vec::new();
    for triple in triples {
        let predicate = triple.predicate.as_str();
        if predicate == RDF_TYPE {
            if let Term::Resource(Resource::Iri(iri)) = &triple.object {
                rdf_types.push(iri.as_str().to_owned());
                continue;
            }
        }
        props
            .entry(predicate.to_owned())
            .or_default()
            .push(term_to_json(&triple.object));
    }
    rdf_types.sort();
    rdf_types.dedup();
    let mut value = serde_json::Map::new();
    value.insert("@id".to_owned(), Value::String(subject_iri.to_owned()));
    value.insert("@type".to_owned(), json!(rdf_types));
    for (k, mut v) in props {
        v.sort_by_key(|x| x.to_string());
        value.insert(k, Value::Array(v));
    }
    Value::Object(value)
}

fn term_to_json(term: &Term) -> Value {
    match term {
        Term::Resource(Resource::Iri(iri)) => json!({ "@id": iri.as_str() }),
        Term::Resource(Resource::BlankNode(node)) => {
            json!({ "@id": format!("_:{}", node.as_str()) })
        }
        Term::Literal(literal) => match literal.language() {
            Some(lang) => json!({
                "@value": literal.value(),
                "@language": lang,
            }),
            None => json!({
                "@value": literal.value(),
                "@type": literal.datatype().as_str(),
            }),
        },
        Term::Resource(other) => json!({ "@id": format!("{other}") }),
        _ => Value::Null,
    }
}
