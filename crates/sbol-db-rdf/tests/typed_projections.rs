use sbol::{Document, RdfFormat};
use sbol_db_core::SequenceAlphabet;
use sbol_db_rdf::document_to_projections;

const NESTED: &str = include_str!("../../sbol-db-postgres/tests/fixtures/nested_construct.ttl");

#[test]
fn extracts_components_with_types_roles_and_sequences() {
    let doc = Document::read(NESTED, RdfFormat::Turtle).expect("parse");
    let projections = document_to_projections(&doc);

    assert_eq!(projections.components.len(), 2, "B0015 + i13504");
    let i13504 = projections
        .components
        .iter()
        .find(|c| c.iri.as_str() == "https://synbiohub.org/public/igem/i13504")
        .expect("i13504");
    assert!(i13504
        .types
        .contains(&"https://identifiers.org/SBO:0000251".to_owned()));
    assert!(i13504
        .roles
        .contains(&"https://identifiers.org/SO:0000704".to_owned()));
    assert!(i13504
        .sequence_iris
        .contains(&"https://synbiohub.org/public/igem/i13504_Sequence1".to_owned()));
    assert!(i13504
        .feature_iris
        .contains(&"https://synbiohub.org/public/igem/i13504/SubComponent1".to_owned()));
}

#[test]
fn extracts_dna_sequences_and_infers_alphabet() {
    let doc = Document::read(NESTED, RdfFormat::Turtle).expect("parse");
    let projections = document_to_projections(&doc);

    assert_eq!(projections.sequences.len(), 2);
    for seq in &projections.sequences {
        assert_eq!(seq.alphabet, Some(SequenceAlphabet::Dna));
        assert!(seq.elements.is_some());
    }
}

#[test]
fn extracts_subcomponent_feature_with_parent_and_instance_of() {
    let doc = Document::read(NESTED, RdfFormat::Turtle).expect("parse");
    let projections = document_to_projections(&doc);

    assert_eq!(projections.features.len(), 1);
    let f = &projections.features[0];
    assert_eq!(f.feature_kind, "SubComponent");
    assert_eq!(
        f.parent_component_iri.as_ref().unwrap().as_str(),
        "https://synbiohub.org/public/igem/i13504"
    );
    assert_eq!(
        f.instance_of_iri.as_ref().unwrap().as_str(),
        "https://synbiohub.org/public/igem/B0015"
    );
}

#[test]
fn extracts_range_location_with_feature_parent_and_positions() {
    let doc = Document::read(NESTED, RdfFormat::Turtle).expect("parse");
    let projections = document_to_projections(&doc);

    assert_eq!(projections.locations.len(), 1);
    let loc = &projections.locations[0];
    assert_eq!(loc.location_kind, "Range");
    assert_eq!(loc.start_pos, Some(1));
    assert_eq!(loc.end_pos, Some(80));
    assert_eq!(
        loc.feature_iri.as_ref().unwrap().as_str(),
        "https://synbiohub.org/public/igem/i13504/SubComponent1"
    );
}
