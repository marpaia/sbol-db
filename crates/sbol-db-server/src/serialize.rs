//! Render an SBOL object closure to a requested download format.
//!
//! The `Downloader` (sbol-db-app) crawls the ACL-scoped triplestore to the
//! transitive closure of an object; this module turns that closure into the
//! byte stream a download route returns. [`serialize_closure`] is the single
//! entry that maps a requested format plus SBOL version to bytes and a content
//! type.
//!
//! RDF/XML, Turtle, JSON-LD, and N-Triples come straight from the triple set as
//! SBOL3, or through [`crate::export::downgrade_sbol3_ntriples`] when the caller
//! asks for SBOL2 (`?version=sbol2`). GenBank and FASTA are sequence-centric
//! exchange formats rendered from the parsed SBOL3 document; they carry no SBOL
//! version marker, so the version flag does not apply to them.
//!
//! `sbol-genbank` and `sbol-fasta` are import-only (they parse GenBank/FASTA
//! into an SBOL document); the reverse direction lives here. GenBank is written
//! through `gb_io`, the same GenBank engine `sbol-genbank` parses with, so the
//! output round-trips through that parser. FASTA is written directly.
//!
//! Two net-new download formats have no analogue in [`SerializationFormat`] and
//! are exposed as their own entries: [`serialize_gff3`] projects the closure's
//! features and locations to GFF3, and [`serialize_omex`] packs the closure's
//! RDF into a COMBINE (OMEX) archive. GFF3 is derived here from the parsed SBOL3
//! document (self-contained; an alternative is a dedicated `sbol-gff3` exporter
//! in the `sbol-rs` workspace, mirroring `sbol-genbank`).

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};

use gb_io::seq::{After, Before, Feature, Location, Qualifier, Seq, Topology};
use sbol::v3::constants::{SBO_PROTEIN, SBO_RNA, SO_CIRCULAR};
use sbol::v3::{Component, Document, Range, Sequence, SequenceFeature, SubComponent};
use sbol_db_core::{DomainError, ObjectTerm, SerializationFormat, SubjectTerm, Triple};
use sbol_db_rdf::triples_to_rdf;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::export::downgrade_sbol3_ntriples;

/// FASTA sequence lines wrap at this column, the conventional width.
const FASTA_LINE_WIDTH: usize = 70;

/// The GFF3 `source` column. libSBOLj (classic SynBioHub's oracle) leaves it
/// empty (`.`), so the projection matches it rather than stamping a tool name.
const GFF3_SOURCE: &str = ".";
/// The GFF3 `score` column: features carry no score, so it is always empty.
const GFF3_SCORE: &str = ".";
/// The GFF3 `phase` column. libSBOLj emits `0` for every feature line, so the
/// projection follows suit rather than the spec's `.` for non-CDS features.
const GFF3_PHASE: &str = "0";

/// The COMBINE-manifest format URI naming the OMEX archive itself.
const OMEX_FORMAT: &str = "http://identifiers.org/combine.specifications/omex";
/// The COMBINE-manifest format URI for the manifest entry.
const OMEX_MANIFEST_FORMAT: &str = "http://identifiers.org/combine.specifications/omex-manifest";
/// The COMBINE-manifest format URI for the SBOL2 RDF member. Classic SynBioHub
/// always downgrades to SBOL2 before archiving; the SBOL3 case uses the
/// version-agnostic SBOL URI.
const OMEX_SBOL2_FORMAT: &str = "http://identifiers.org/combine.specifications/sbol.version-2";
/// The COMBINE-manifest format URI for an SBOL3 RDF member.
const OMEX_SBOL3_FORMAT: &str = "http://identifiers.org/combine.specifications/sbol";
/// The COMBINE-manifest format URI for the OMEX metadata member.
const OMEX_METADATA_FORMAT: &str = "http://identifiers.org/combine.specifications/omex-metadata";
/// The archive-relative name of the SBOL RDF member.
const OMEX_SBOL_ENTRY: &str = "sbol.rdf";
/// The archive-relative name of the OMEX metadata member.
const OMEX_METADATA_ENTRY: &str = "metadata.rdf";

/// A serialized closure ready to become an HTTP response body.
pub struct Serialized {
    /// The rendered document bytes.
    pub bytes: Vec<u8>,
    /// The MIME type the download route sets on the response.
    pub content_type: &'static str,
}

/// Render the closure `triples` to `format`. When `sbol2` is set the RDF
/// formats are downgraded to SBOL2; GenBank and FASTA ignore it, being
/// version-agnostic sequence formats.
pub fn serialize_closure(
    triples: &[Triple],
    format: SerializationFormat,
    sbol2: bool,
) -> Result<Serialized, DomainError> {
    match format {
        SerializationFormat::RdfXml
        | SerializationFormat::Turtle
        | SerializationFormat::JsonLd
        | SerializationFormat::NTriples => {
            let text = if sbol2 {
                let ntriples = triples_to_rdf(triples, SerializationFormat::NTriples)?;
                downgrade_sbol3_ntriples(&ntriples, format)?
            } else {
                triples_to_rdf(triples, format)?
            };
            Ok(Serialized {
                bytes: text.into_bytes(),
                content_type: rdf_content_type(format),
            })
        }
        SerializationFormat::GenBank => {
            let document = parse_document(triples)?;
            Ok(Serialized {
                bytes: to_genbank(&document)?,
                content_type: "chemical/x-genbank",
            })
        }
        SerializationFormat::Fasta => {
            let document = parse_document(triples)?;
            Ok(Serialized {
                bytes: to_fasta(&document).into_bytes(),
                content_type: "chemical/x-fasta",
            })
        }
        other => Err(DomainError::InvalidInput(format!(
            "cannot serialize a download as {other:?}"
        ))),
    }
}

/// The MIME type for an RDF serialization format.
fn rdf_content_type(format: SerializationFormat) -> &'static str {
    match format {
        SerializationFormat::RdfXml => "application/rdf+xml",
        SerializationFormat::Turtle => "text/turtle",
        SerializationFormat::JsonLd => "application/ld+json",
        SerializationFormat::NTriples => "application/n-triples",
        _ => "application/octet-stream",
    }
}

/// Parse the closure's triples into an SBOL3 [`Document`] the sequence
/// exporters read. The triples are first rendered to N-Triples, the lossless
/// wire form the SBOL reader consumes, matching what `export.rs` does.
///
/// The sequence-centric exporters read the SBOL3 typed model, but SynBioHub's
/// stored view is SBOL2. A closure carrying only SBOL2 vocabulary is upgraded
/// to SBOL3 first (`ComponentDefinition` becomes `Component`, `Sequence` keeps
/// its residues); a native SBOL3 closure is read directly.
fn parse_document(triples: &[Triple]) -> Result<Document, DomainError> {
    let ntriples = triples_to_rdf(triples, SerializationFormat::NTriples)?;
    if is_sbol2(triples) {
        let (document, _report) =
            sbol::convert::upgrade_from_sbol2(&ntriples, sbol::RdfFormat::NTriples)
                .map_err(|e| DomainError::Parse(e.to_string()))?;
        Ok(document)
    } else {
        Document::read(&ntriples, sbol::RdfFormat::NTriples)
            .map_err(|e| DomainError::Parse(e.to_string()))
    }
}

/// Whether a closure is expressed in SBOL2 rather than SBOL3. SBOL3 evidence
/// wins: any triple in the SBOL3 namespace marks the closure SBOL3, so a
/// document is treated as SBOL2 only when it carries SBOL2 vocabulary and no
/// SBOL3 vocabulary at all.
fn is_sbol2(triples: &[Triple]) -> bool {
    const SBOL2_NS: &str = "http://sbols.org/v2#";
    const SBOL3_NS: &str = "http://sbols.org/v3#";
    let mut saw_sbol2 = false;
    for triple in triples {
        if triple.predicate.as_str().starts_with(SBOL3_NS) {
            return false;
        }
        if triple.predicate.as_str().starts_with(SBOL2_NS) {
            saw_sbol2 = true;
        }
        if let ObjectTerm::Iri(iri) = &triple.object {
            if iri.as_str().starts_with(SBOL3_NS) {
                return false;
            }
            if iri.as_str().starts_with(SBOL2_NS) {
                saw_sbol2 = true;
            }
        }
    }
    saw_sbol2
}

/// Write the document's sequences as FASTA records, wrapped at
/// [`FASTA_LINE_WIDTH`]. A sequence owned by a [`Component`] is headed by the
/// owning component's name (or display id) and an ordinal
/// (`>{owner} sequence {n} ({len} {units})`), matching classic SynBioHub's
/// ComponentDefinition FASTA; a sequence with no owning component in the closure
/// is headed by its own name (`>{name} ({len} {units})`), matching classic's
/// bare-Sequence FASTA.
fn to_fasta(document: &Document) -> String {
    use sbol::v3::SbolIdentified;

    let sequences: HashMap<&str, &Sequence> = document
        .sequences()
        .filter_map(|seq| seq.identity.as_iri().map(|iri| (iri.as_str(), seq)))
        .collect();

    let mut out = String::new();
    let mut owned: HashSet<&str> = HashSet::new();

    for component in document.components() {
        let owner = component
            .name()
            .or_else(|| component.display_id())
            .or_else(|| component.identity.as_iri().map(|iri| iri.as_str()))
            .unwrap_or("component");
        let units = fasta_length_units(component);
        let mut index = 0usize;
        for (iri, sequence) in component
            .sequences
            .iter()
            .filter_map(|resource| resource.as_iri())
            .filter_map(|iri| sequences.get(iri.as_str()).map(|seq| (iri.as_str(), *seq)))
        {
            let Some(elements) = sequence.elements.as_deref().filter(|e| !e.is_empty()) else {
                continue;
            };
            index += 1;
            owned.insert(iri);
            let header = format!("{owner} sequence {index} ({} {units})", elements.len());
            write_fasta_record(&mut out, &header, elements);
        }
    }

    for sequence in document.sequences() {
        let Some(iri) = sequence.identity.as_iri().map(|i| i.as_str()) else {
            continue;
        };
        if owned.contains(iri) {
            continue;
        }
        let Some(elements) = sequence.elements.as_deref().filter(|e| !e.is_empty()) else {
            continue;
        };
        let name = sequence
            .name()
            .or_else(|| sequence.display_id())
            .unwrap_or(iri);
        let header = format!("{name} ({} bp)", elements.len());
        write_fasta_record(&mut out, &header, elements);
    }
    out
}

/// The FASTA length-unit token for a component's sequence: `aa` for a protein,
/// `bp` for nucleic acids, matching classic SynBioHub's unit labels.
fn fasta_length_units(component: &Component) -> &'static str {
    if molecule_type(component) == "AA" {
        "aa"
    } else {
        "bp"
    }
}

/// Append one FASTA record: a `>`-prefixed header line followed by the residues
/// wrapped at [`FASTA_LINE_WIDTH`].
fn write_fasta_record(out: &mut String, header: &str, elements: &str) {
    out.push('>');
    out.push_str(header);
    out.push('\n');
    for chunk in elements.as_bytes().chunks(FASTA_LINE_WIDTH) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push('\n');
    }
}

/// Write every [`Component`] that carries a resolvable sequence as one GenBank
/// record, mapping its `SequenceFeature`s to GenBank features. Rendering goes
/// through `gb_io` so the output parses back through the same engine
/// `sbol-genbank` imports with.
fn to_genbank(document: &Document) -> Result<Vec<u8>, DomainError> {
    use sbol::v3::SbolIdentified;

    let sequences: HashMap<&str, &Sequence> = document
        .sequences()
        .filter_map(|seq| seq.identity.as_iri().map(|iri| (iri.as_str(), seq)))
        .collect();
    let features: HashMap<&str, &SequenceFeature> = document
        .sequence_features()
        .filter_map(|feat| feat.identity.as_iri().map(|iri| (iri.as_str(), feat)))
        .collect();
    let ranges: HashMap<&str, &Range> = document
        .ranges()
        .filter_map(|range| range.identity.as_iri().map(|iri| (iri.as_str(), range)))
        .collect();

    let mut buffer = Vec::new();
    for component in document.components() {
        let Some(elements) = component
            .sequences
            .iter()
            .filter_map(|resource| resource.as_iri())
            .filter_map(|iri| sequences.get(iri.as_str()))
            .find_map(|seq| seq.elements.as_deref().filter(|e| !e.is_empty()))
        else {
            continue;
        };

        let mut record = Seq::empty();
        record.name = component.display_id().map(sanitize_locus_name).or_else(|| {
            component
                .identity
                .as_iri()
                .map(|iri| iri.as_str().to_owned())
        });
        record.definition = component.name().map(ToOwned::to_owned);
        record.molecule_type = Some(molecule_type(component).to_owned());
        record.topology = topology(component);
        record.seq = elements.as_bytes().to_vec();
        record.features = component_features(component, &features, &ranges);

        record
            .write(Cursor::new(&mut buffer))
            .map_err(|e| DomainError::Serialization(e.to_string()))?;
    }
    Ok(buffer)
}

/// Map a component's [`SequenceFeature`]s to GenBank features, resolving each
/// feature's `Range` locations. A feature with no resolvable range is dropped,
/// as GenBank has nowhere to place it.
fn component_features(
    component: &Component,
    features: &HashMap<&str, &SequenceFeature>,
    ranges: &HashMap<&str, &Range>,
) -> Vec<Feature> {
    use sbol::v3::SbolIdentified;

    let mut out = Vec::new();
    for feature in component
        .features
        .iter()
        .filter_map(|resource| resource.as_iri())
        .filter_map(|iri| features.get(iri.as_str()))
    {
        let spans: Vec<Location> = feature
            .locations
            .iter()
            .filter_map(|resource| resource.as_iri())
            .filter_map(|iri| ranges.get(iri.as_str()))
            .filter_map(|range| location_for(range))
            .collect();
        let location = match spans.len() {
            0 => continue,
            1 => spans.into_iter().next().expect("one span"),
            _ => Location::Join(spans),
        };

        let mut qualifiers: Vec<Qualifier> = Vec::new();
        if let Some(label) = feature.name().or_else(|| feature.display_id()) {
            qualifiers.push(("label".into(), Some(label.to_owned())));
        }
        out.push(Feature {
            kind: "misc_feature".into(),
            location,
            qualifiers,
        });
    }
    out
}

/// Convert an SBOL [`Range`] (1-based, inclusive) to a `gb_io` location
/// (0-based start, exclusive end).
fn location_for(range: &Range) -> Option<Location> {
    let (start, end) = (range.start?, range.end?);
    Some(Location::Range(
        (start - 1, Before(false)),
        (end, After(false)),
    ))
}

/// The GenBank LOCUS topology for a component, read from its SBOL types.
fn topology(component: &Component) -> Topology {
    if component
        .types
        .iter()
        .any(|iri| iri.as_str() == SO_CIRCULAR.as_str())
    {
        Topology::Circular
    } else {
        Topology::Linear
    }
}

/// The GenBank molecule type for a component, read from its SBOL biological
/// types. Nucleic acids default to DNA; a protein component is written as `AA`.
fn molecule_type(component: &Component) -> &'static str {
    let types: Vec<&str> = component.types.iter().map(|iri| iri.as_str()).collect();
    if types.contains(&SBO_PROTEIN.as_str()) {
        "AA"
    } else if types.contains(&SBO_RNA.as_str()) {
        "RNA"
    } else {
        "DNA"
    }
}

/// Trim a display id to the GenBank LOCUS name, which allows no whitespace.
fn sanitize_locus_name(display_id: &str) -> String {
    display_id.replace(char::is_whitespace, "_")
}

// ----- GFF3 ----------------------------------------------------------------

/// Project the closure's components, features, and locations to GFF3. Each
/// component with a sequence emits a `##sequence-region` header; every located
/// feature under it becomes one tab-delimited line per `Range`, with the
/// feature's Sequence Ontology role as the `type` column, the range's 1-based
/// inclusive bounds (which GFF3 shares) as `start`/`end`, the orientation as
/// `strand`, and `ID`/`Name` attributes from the feature's display id and name.
pub fn serialize_gff3(triples: &[Triple]) -> Result<Serialized, DomainError> {
    let document = parse_document(triples)?;
    Ok(Serialized {
        bytes: to_gff3(&document).into_bytes(),
        content_type: "text/plain; charset=utf-8",
    })
}

/// Render the document's features to a GFF3 string. `SequenceFeature`s carry
/// their role and display id directly; a `SubComponent` borrows the role and
/// naming of the `Component` it instantiates, matching how classic SynBioHub's
/// oracle labels sub-part features.
fn to_gff3(document: &Document) -> String {
    use sbol::v3::SbolIdentified;

    let sequences: HashMap<&str, &Sequence> = document
        .sequences()
        .filter_map(|seq| seq.identity.as_iri().map(|iri| (iri.as_str(), seq)))
        .collect();
    let components: HashMap<&str, &Component> = document
        .components()
        .filter_map(|c| c.identity.as_iri().map(|iri| (iri.as_str(), c)))
        .collect();
    let features: HashMap<&str, &SequenceFeature> = document
        .sequence_features()
        .filter_map(|f| f.identity.as_iri().map(|iri| (iri.as_str(), f)))
        .collect();
    let sub_components: HashMap<&str, &SubComponent> = document
        .sub_components()
        .filter_map(|s| s.identity.as_iri().map(|iri| (iri.as_str(), s)))
        .collect();
    let ranges: HashMap<&str, &Range> = document
        .ranges()
        .filter_map(|r| r.identity.as_iri().map(|iri| (iri.as_str(), r)))
        .collect();

    let mut out = String::from("##gff-version 3\n");
    for component in document.components() {
        let seqid = component
            .display_id()
            .map(ToOwned::to_owned)
            .or_else(|| {
                component
                    .identity
                    .as_iri()
                    .map(|iri| iri.as_str().to_owned())
            })
            .unwrap_or_default();

        if let Some(len) = component_sequence_len(component, &sequences) {
            out.push_str(&format!("##sequence-region {seqid} 1 {len}\n"));
        }

        for feature_iri in component.features.iter().filter_map(|r| r.as_iri()) {
            let iri = feature_iri.as_str();
            if let Some(feature) = features.get(iri) {
                let gtype = gff3_type(&feature.feature.roles);
                let feat_orientation = feature.feature.orientation.as_ref().map(|i| i.as_str());
                let id = feature.display_id().unwrap_or(iri);
                let name = feature.name();
                write_gff3_lines(
                    &mut out,
                    &seqid,
                    &gtype,
                    id,
                    name,
                    &feature.locations,
                    feat_orientation,
                    &ranges,
                );
            } else if let Some(sub) = sub_components.get(iri) {
                let definition = sub
                    .instance_of
                    .as_ref()
                    .and_then(|r| r.as_iri())
                    .and_then(|def| components.get(def.as_str()).copied());
                let empty: Vec<sbol::Iri> = Vec::new();
                let roles = definition.map(|c| &c.roles).unwrap_or(&empty);
                let gtype = gff3_type(roles);
                let feat_orientation = sub.feature.orientation.as_ref().map(|i| i.as_str());
                let id = definition
                    .and_then(|c| c.display_id())
                    .or_else(|| sub.display_id())
                    .unwrap_or(iri);
                let name = definition.and_then(|c| c.name()).or_else(|| sub.name());
                write_gff3_lines(
                    &mut out,
                    &seqid,
                    &gtype,
                    id,
                    name,
                    &sub.locations,
                    feat_orientation,
                    &ranges,
                );
            }
        }
    }
    out
}

/// The length of a component's resolvable sequence, for the `##sequence-region`
/// header, or `None` when the component carries no sequence.
fn component_sequence_len(
    component: &Component,
    sequences: &HashMap<&str, &Sequence>,
) -> Option<usize> {
    component
        .sequences
        .iter()
        .filter_map(|resource| resource.as_iri())
        .filter_map(|iri| sequences.get(iri.as_str()))
        .find_map(|seq| seq.elements.as_deref().filter(|e| !e.is_empty()))
        .map(str::len)
}

/// Emit one GFF3 line per `Range` location of a feature. A location's own
/// orientation wins over the feature's; a location that is not a `Range` (a
/// `Cut`, say) has no span and is skipped.
#[allow(clippy::too_many_arguments)]
fn write_gff3_lines(
    out: &mut String,
    seqid: &str,
    gtype: &str,
    id: &str,
    name: Option<&str>,
    locations: &[sbol::Resource],
    feature_orientation: Option<&str>,
    ranges: &HashMap<&str, &Range>,
) {
    let attributes = match name {
        Some(name) => format!("ID={id};Name={name}"),
        None => format!("ID={id}"),
    };
    for range in locations
        .iter()
        .filter_map(|r| r.as_iri())
        .filter_map(|iri| ranges.get(iri.as_str()))
    {
        let (Some(start), Some(end)) = (range.start, range.end) else {
            continue;
        };
        let orientation = range
            .location
            .orientation
            .as_ref()
            .map(|i| i.as_str())
            .or(feature_orientation);
        let strand = gff3_strand(orientation);
        out.push_str(&format!(
            "{seqid}\t{GFF3_SOURCE}\t{gtype}\t{start}\t{end}\t{GFF3_SCORE}\t{strand}\t{GFF3_PHASE}\t{attributes}\n"
        ));
    }
}

/// The GFF3 `type` for a feature: the Sequence Ontology term named by its first
/// role, resolved to the SO term name when known and left as the bare accession
/// otherwise. A feature with no SO role is a generic `sequence_feature`.
fn gff3_type(roles: &[sbol::Iri]) -> String {
    for role in roles {
        if let Some(accession) = so_accession(role.as_str()) {
            return so_term_name(&accession).unwrap_or(accession);
        }
    }
    "sequence_feature".to_owned()
}

/// Extract a Sequence Ontology accession (`SO:0000316`) from a role IRI, which
/// may spell it with either the `SO:` (identifiers.org) or `SO_` (OBO) form.
fn so_accession(iri: &str) -> Option<String> {
    for separator in ["SO:", "SO_"] {
        if let Some(pos) = iri.rfind(separator) {
            let digits: String = iri[pos + separator.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                return Some(format!("SO:{digits}"));
            }
        }
    }
    None
}

/// The SO term name for the accessions synthetic-biology parts use as feature
/// roles. Unknown accessions fall back to the bare accession, which is a valid
/// GFF3 `type`.
fn so_term_name(accession: &str) -> Option<String> {
    let name = match accession {
        "SO:0000167" => "promoter",
        "SO:0000316" => "CDS",
        "SO:0000139" => "ribosome_entry_site",
        "SO:0000141" => "terminator",
        "SO:0000057" => "operator",
        "SO:0000704" => "gene",
        "SO:0000234" => "mRNA",
        "SO:0000280" => "engineered_gene",
        "SO:0000804" => "engineered_region",
        "SO:0000110" => "sequence_feature",
        _ => return None,
    };
    Some(name.to_owned())
}

/// The GFF3 strand for an orientation IRI: reverse-complement is `-`, any other
/// stated orientation is `+`, and an absent orientation is unstranded (`.`).
fn gff3_strand(orientation: Option<&str>) -> &'static str {
    match orientation {
        Some(iri) if iri.ends_with("reverseComplement") || iri.ends_with("SO:0001031") => "-",
        Some(_) => "+",
        None => ".",
    }
}

// ----- OMEX (COMBINE archive) ----------------------------------------------

/// One non-SBOL member of a COMBINE archive, i.e. an attachment blob. The bytes
/// originate in the blob store; [`OmexAttachmentSource`] is the seam through
/// which [`serialize_omex`] pulls them.
#[derive(Clone)]
pub struct OmexAttachment {
    /// Archive-relative name for the entry.
    pub filename: String,
    /// COMBINE-manifest format URI describing the entry.
    pub format: String,
    /// The entry's raw bytes.
    pub bytes: Vec<u8>,
}

/// Supplies a closure's attachment blobs as COMBINE members. The download route
/// resolves each attachment in the closure against the blob store and implements
/// this so `/omex` includes the attachment payloads; calling [`serialize_omex`]
/// with `None` yields an archive of just the manifest and the SBOL RDF.
pub trait OmexAttachmentSource {
    /// The attachment members belonging to the objects in `triples`.
    fn attachments_for(&self, triples: &[Triple]) -> Result<Vec<OmexAttachment>, DomainError>;
}

/// Pack the closure into a COMBINE (OMEX) archive: a zip of `manifest.xml`,
/// `sbol.rdf` (the closure as RDF/XML, downgraded to SBOL2 when `sbol2` is set,
/// marked as the archive master), and `metadata.rdf` (the OMEX metadata
/// describing the SBOL member). When an [`OmexAttachmentSource`] is supplied,
/// each attachment blob is added as an archive entry and listed in the manifest.
pub fn serialize_omex(
    triples: &[Triple],
    sbol2: bool,
    attachments: Option<&dyn OmexAttachmentSource>,
) -> Result<Serialized, DomainError> {
    let sbol_rdf = serialize_closure(triples, SerializationFormat::RdfXml, sbol2)?.bytes;
    let members = match attachments {
        Some(source) => source.attachments_for(triples)?,
        None => Vec::new(),
    };
    let manifest = omex_manifest(sbol2, &members);
    let metadata = omex_metadata();

    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        // COMBINE members are stored uncompressed, so no deflate backend is
        // needed and the archive opens with any zip reader.
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let map_err = |e: zip::result::ZipError| DomainError::Serialization(e.to_string());

        zip.start_file("manifest.xml", options).map_err(map_err)?;
        zip.write_all(manifest.as_bytes())
            .map_err(|e| DomainError::Io(e.to_string()))?;

        zip.start_file(OMEX_SBOL_ENTRY, options).map_err(map_err)?;
        zip.write_all(&sbol_rdf)
            .map_err(|e| DomainError::Io(e.to_string()))?;

        zip.start_file(OMEX_METADATA_ENTRY, options)
            .map_err(map_err)?;
        zip.write_all(metadata.as_bytes())
            .map_err(|e| DomainError::Io(e.to_string()))?;

        for member in &members {
            zip.start_file(member.filename.clone(), options)
                .map_err(map_err)?;
            zip.write_all(&member.bytes)
                .map_err(|e| DomainError::Io(e.to_string()))?;
        }
        zip.finish().map_err(map_err)?;
    }

    Ok(Serialized {
        bytes: cursor.into_inner(),
        content_type: "application/zip",
    })
}

/// Build the COMBINE manifest listing the archive itself, the manifest, the
/// SBOL RDF master, the OMEX metadata, and any attachment members.
fn omex_manifest(sbol2: bool, members: &[OmexAttachment]) -> String {
    let sbol_format = if sbol2 {
        OMEX_SBOL2_FORMAT
    } else {
        OMEX_SBOL3_FORMAT
    };
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(
        "<omexManifest xmlns=\"http://identifiers.org/combine.specifications/omex-manifest\">\n",
    );
    xml.push_str(&format!(
        "  <content location=\".\" format=\"{OMEX_FORMAT}\"/>\n"
    ));
    xml.push_str(&format!(
        "  <content location=\"./manifest.xml\" format=\"{OMEX_MANIFEST_FORMAT}\"/>\n"
    ));
    xml.push_str(&format!(
        "  <content location=\"./{OMEX_SBOL_ENTRY}\" format=\"{sbol_format}\" master=\"true\"/>\n"
    ));
    xml.push_str(&format!(
        "  <content location=\"./{OMEX_METADATA_ENTRY}\" format=\"{OMEX_METADATA_FORMAT}\"/>\n"
    ));
    for member in members {
        xml.push_str(&format!(
            "  <content location=\"./{}\" format=\"{}\"/>\n",
            xml_escape(&member.filename),
            xml_escape(&member.format)
        ));
    }
    xml.push_str("</omexManifest>\n");
    xml
}

/// Build the OMEX metadata member describing the SBOL RDF entry with its
/// creation and modification timestamps, mirroring the `metadata.rdf` classic
/// SynBioHub's COMBINE archive writer emits.
fn omex_metadata() -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" \
         xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:vCard=\"http://www.w3.org/2006/vcard/ns#\">\n\
         \x20 <rdf:Description rdf:about=\"./{OMEX_SBOL_ENTRY}\">\n\
         \x20   <dcterms:created rdf:parseType=\"Resource\">\n\
         \x20     <dcterms:W3CDTF>{now}</dcterms:W3CDTF>\n\
         \x20   </dcterms:created>\n\
         \x20   <dcterms:modified rdf:parseType=\"Resource\">\n\
         \x20     <dcterms:W3CDTF>{now}</dcterms:W3CDTF>\n\
         \x20   </dcterms:modified>\n\
         \x20 </rdf:Description>\n\
         </rdf:RDF>\n"
    )
}

/// Escape the XML attribute metacharacters in a manifest value.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ----- /summary (libSBOLj serializeJSON) -----------------------------------

/// Render the closure as classic SynBioHub's `/summary`: the JSON object
/// libSBOLj's `SBOLDocument.serializeJSON` produces.
///
/// The object is keyed by top-level type collection (`componentDefinitions`,
/// `sequences`, `collections`, ...), each an array of the objects of that type,
/// with child objects (components, sequence annotations, locations, ...) nested
/// inside their parents. The projection reads the stored SBOL2 triples directly,
/// reproducing libSBOLj's field selection quirks so the two implementations'
/// output compares structurally: `dcterms:title`/`sbol:name` is consumed as the
/// object name and then dropped (libSBOLj serializes it to an always-undefined
/// `title`), a collection's members are emitted under the `models` key, and any
/// predicate outside the recognized core set becomes an `annotations` entry.
pub fn serialize_summary(triples: &[Triple]) -> Result<Serialized, DomainError> {
    let summary = Summary::new(triples);
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(summary.document()))
        .map_err(|e| DomainError::Serialization(e.to_string()))?;
    Ok(Serialized {
        bytes: text.into_bytes(),
        content_type: "application/json",
    })
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SBOL2_NS: &str = "http://sbols.org/v2#";

/// The predicates every `Identified` consumes into its own fields (and so never
/// emits as an annotation). `dcterms:title`/`sbol:name` and
/// `dcterms:description`/`sbol:description` map to the object's name and
/// description; the name is then dropped, matching libSBOLj.
const IDENTIFIED_CORE: &[&str] = &[
    RDF_TYPE,
    "http://purl.org/dc/terms/title",
    "http://purl.org/dc/terms/description",
    "http://sbols.org/v2#displayId",
    "http://sbols.org/v2#persistentIdentity",
    "http://sbols.org/v2#version",
    "http://sbols.org/v2#name",
    "http://sbols.org/v2#description",
    "http://sbols.org/v2#attachment",
    "http://www.w3.org/ns/prov#wasDerivedFrom",
    "http://www.w3.org/ns/prov#wasGeneratedBy",
];

/// The type-specific predicates each SBOL type consumes into its own fields,
/// excluded from the object's `annotations`.
fn type_core(local: &str) -> &'static [&'static str] {
    match local {
        "ComponentDefinition" => &[
            "http://sbols.org/v2#type",
            "http://sbols.org/v2#role",
            "http://sbols.org/v2#component",
            "http://sbols.org/v2#sequenceAnnotation",
            "http://sbols.org/v2#sequenceConstraint",
            "http://sbols.org/v2#sequence",
        ],
        "Sequence" => &[
            "http://sbols.org/v2#elements",
            "http://sbols.org/v2#encoding",
        ],
        "Collection" => &["http://sbols.org/v2#member"],
        "ModuleDefinition" => &[
            "http://sbols.org/v2#role",
            "http://sbols.org/v2#module",
            "http://sbols.org/v2#functionalComponent",
            "http://sbols.org/v2#interaction",
            "http://sbols.org/v2#model",
        ],
        "Model" => &[
            "http://sbols.org/v2#source",
            "http://sbols.org/v2#language",
            "http://sbols.org/v2#framework",
        ],
        "Attachment" => &[
            "http://sbols.org/v2#source",
            "http://sbols.org/v2#format",
            "http://sbols.org/v2#size",
            "http://sbols.org/v2#hash",
        ],
        "Implementation" => &["http://sbols.org/v2#built"],
        "Component" => &[
            "http://sbols.org/v2#role",
            "http://sbols.org/v2#access",
            "http://sbols.org/v2#roleIntegration",
            "http://sbols.org/v2#definition",
            "http://sbols.org/v2#mapsTo",
        ],
        "SequenceAnnotation" => &[
            "http://sbols.org/v2#location",
            "http://sbols.org/v2#component",
            "http://sbols.org/v2#role",
        ],
        "SequenceConstraint" => &[
            "http://sbols.org/v2#restriction",
            "http://sbols.org/v2#subject",
            "http://sbols.org/v2#object",
        ],
        "Range" => &[
            "http://sbols.org/v2#start",
            "http://sbols.org/v2#end",
            "http://sbols.org/v2#orientation",
        ],
        "Cut" => &["http://sbols.org/v2#at", "http://sbols.org/v2#orientation"],
        "GenericLocation" => &["http://sbols.org/v2#orientation"],
        "MapsTo" => &[
            "http://sbols.org/v2#refinement",
            "http://sbols.org/v2#remote",
            "http://sbols.org/v2#local",
        ],
        "Module" => &[
            "http://sbols.org/v2#definition",
            "http://sbols.org/v2#mapsTo",
        ],
        "FunctionalComponent" => &[
            "http://sbols.org/v2#access",
            "http://sbols.org/v2#direction",
            "http://sbols.org/v2#definition",
            "http://sbols.org/v2#mapsTo",
        ],
        "Interaction" => &[
            "http://sbols.org/v2#type",
            "http://sbols.org/v2#participation",
        ],
        "Participation" => &[
            "http://sbols.org/v2#role",
            "http://sbols.org/v2#participant",
        ],
        _ => &[],
    }
}

/// The top-level `serializeJSON` key an SBOL type is bucketed under, or `None`
/// for a child object that is only ever nested inside its parent.
fn top_level_key(local: &str) -> Option<&'static str> {
    match local {
        "Collection" => Some("collections"),
        "ModuleDefinition" => Some("moduleDefinitions"),
        "Model" => Some("models"),
        "Implementation" => Some("implementations"),
        "ComponentDefinition" => Some("componentDefinitions"),
        "Sequence" => Some("sequences"),
        "GenericTopLevel" => Some("genericTopLevels"),
        "Attachment" => Some("attachments"),
        _ => None,
    }
}

/// The closure indexed by subject, projecting the stored SBOL2 triples into
/// libSBOLj's `serializeJSON` shape.
struct Summary<'a> {
    by_subject: HashMap<&'a str, Vec<(&'a str, &'a ObjectTerm)>>,
}

impl<'a> Summary<'a> {
    fn new(triples: &'a [Triple]) -> Self {
        let mut by_subject: HashMap<&str, Vec<(&str, &ObjectTerm)>> = HashMap::new();
        for triple in triples {
            if let SubjectTerm::Iri(subject) = &triple.subject {
                by_subject
                    .entry(subject.as_str())
                    .or_default()
                    .push((triple.predicate.as_str(), &triple.object));
            }
        }
        Summary { by_subject }
    }

    /// Bucket every top-level object by its `serializeJSON` collection key.
    fn document(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut subjects: Vec<&str> = self.by_subject.keys().copied().collect();
        subjects.sort_unstable();

        let mut buckets: std::collections::BTreeMap<&str, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for iri in subjects {
            let Some(local) = self.type_of(iri) else {
                continue;
            };
            if let Some(key) = top_level_key(local) {
                buckets.entry(key).or_default().push(self.object(iri));
            }
        }

        let mut out = serde_json::Map::new();
        for (key, values) in buckets {
            out.insert(key.to_owned(), serde_json::Value::Array(values));
        }
        out
    }

    /// The local name of a subject's SBOL2 `rdf:type`, or `None` when it carries
    /// no type in the SBOL2 namespace.
    fn type_of(&self, iri: &str) -> Option<&'a str> {
        self.objects(iri, RDF_TYPE).into_iter().find_map(|object| {
            if let ObjectTerm::Iri(term) = object {
                term.as_str().strip_prefix(SBOL2_NS)
            } else {
                None
            }
        })
    }

    /// Every object-position term for `iri`'s `predicate`.
    fn objects(&self, iri: &str, predicate: &str) -> Vec<&'a ObjectTerm> {
        self.by_subject
            .get(iri)
            .map(|props| {
                props
                    .iter()
                    .filter(|(p, _)| *p == predicate)
                    .map(|(_, o)| *o)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The first object-position term for `iri`'s `predicate`, as a string.
    fn first_string(&self, iri: &str, predicate: &str) -> Option<String> {
        self.objects(iri, predicate).first().map(|o| term_string(o))
    }

    /// Every object-position term for `iri`'s `predicate`, as strings.
    fn string_array(&self, iri: &str, predicate: &str) -> Vec<String> {
        self.objects(iri, predicate)
            .into_iter()
            .map(term_string)
            .collect()
    }

    /// Serialize one object (top-level or nested) to its `serializeJSON` form.
    fn object(&self, iri: &str) -> serde_json::Value {
        let local = self.type_of(iri).unwrap_or("");
        let mut out = serde_json::Map::new();
        self.identified(iri, local, &mut out);
        match local {
            "ComponentDefinition" => {
                self.string_field(&mut out, "types", iri, "http://sbols.org/v2#type");
                self.string_field(&mut out, "roles", iri, "http://sbols.org/v2#role");
                self.nested_field(&mut out, "components", iri, "http://sbols.org/v2#component");
                self.nested_field(
                    &mut out,
                    "sequenceAnnotations",
                    iri,
                    "http://sbols.org/v2#sequenceAnnotation",
                );
                self.nested_field(
                    &mut out,
                    "sequenceConstraints",
                    iri,
                    "http://sbols.org/v2#sequenceConstraint",
                );
                self.string_field(&mut out, "sequences", iri, "http://sbols.org/v2#sequence");
            }
            "Sequence" => {
                out.insert(
                    "elements".into(),
                    self.scalar(iri, "http://sbols.org/v2#elements"),
                );
                out.insert(
                    "encoding".into(),
                    self.scalar(iri, "http://sbols.org/v2#encoding"),
                );
            }
            "Collection" => {
                // libSBOLj's serializeCollection emits members under `models`.
                self.string_field(&mut out, "models", iri, "http://sbols.org/v2#member");
            }
            "ModuleDefinition" => {
                self.string_field(&mut out, "roles", iri, "http://sbols.org/v2#role");
                self.nested_field(&mut out, "modules", iri, "http://sbols.org/v2#module");
                self.nested_field(
                    &mut out,
                    "functionalComponents",
                    iri,
                    "http://sbols.org/v2#functionalComponent",
                );
                self.nested_field(
                    &mut out,
                    "interactions",
                    iri,
                    "http://sbols.org/v2#interaction",
                );
                self.string_field(&mut out, "models", iri, "http://sbols.org/v2#model");
            }
            "Model" => {
                out.insert(
                    "source".into(),
                    self.scalar(iri, "http://sbols.org/v2#source"),
                );
                out.insert(
                    "language".into(),
                    self.scalar(iri, "http://sbols.org/v2#language"),
                );
                out.insert(
                    "framework".into(),
                    self.scalar(iri, "http://sbols.org/v2#framework"),
                );
            }
            "Attachment" => {
                out.insert(
                    "source".into(),
                    self.scalar(iri, "http://sbols.org/v2#source"),
                );
                out.insert(
                    "format".into(),
                    self.scalar(iri, "http://sbols.org/v2#format"),
                );
                if let Some(size) = self.number(iri, "http://sbols.org/v2#size") {
                    out.insert("size".into(), size);
                }
                if let Some(hash) = self.first_string(iri, "http://sbols.org/v2#hash") {
                    out.insert("hash".into(), serde_json::Value::from(hash));
                }
            }
            "Implementation" => {
                out.insert(
                    "built".into(),
                    self.scalar(iri, "http://sbols.org/v2#built"),
                );
            }
            "Component" => {
                self.string_field(&mut out, "roles", iri, "http://sbols.org/v2#role");
                self.optional(&mut out, "access", iri, "http://sbols.org/v2#access");
                self.optional(
                    &mut out,
                    "roleIntegration",
                    iri,
                    "http://sbols.org/v2#roleIntegration",
                );
                out.insert(
                    "definition".into(),
                    self.scalar(iri, "http://sbols.org/v2#definition"),
                );
                self.nested_field(&mut out, "mapsTos", iri, "http://sbols.org/v2#mapsTo");
            }
            "SequenceAnnotation" => {
                self.nested_field(&mut out, "locations", iri, "http://sbols.org/v2#location");
                self.optional(&mut out, "component", iri, "http://sbols.org/v2#component");
                self.string_field(&mut out, "roles", iri, "http://sbols.org/v2#role");
            }
            "SequenceConstraint" => {
                out.insert(
                    "restriction".into(),
                    self.scalar(iri, "http://sbols.org/v2#restriction"),
                );
                out.insert(
                    "subject".into(),
                    self.scalar(iri, "http://sbols.org/v2#subject"),
                );
                out.insert(
                    "object".into(),
                    self.scalar(iri, "http://sbols.org/v2#object"),
                );
            }
            "Range" => {
                if let Some(start) = self.number(iri, "http://sbols.org/v2#start") {
                    out.insert("start".into(), start);
                }
                if let Some(end) = self.number(iri, "http://sbols.org/v2#end") {
                    out.insert("end".into(), end);
                }
                self.optional(
                    &mut out,
                    "orientation",
                    iri,
                    "http://sbols.org/v2#orientation",
                );
            }
            "Cut" => {
                self.optional(&mut out, "at", iri, "http://sbols.org/v2#at");
                self.optional(
                    &mut out,
                    "orientation",
                    iri,
                    "http://sbols.org/v2#orientation",
                );
            }
            "GenericLocation" => {
                self.optional(
                    &mut out,
                    "orientation",
                    iri,
                    "http://sbols.org/v2#orientation",
                );
            }
            "MapsTo" => {
                // libSBOLj's serializeMapping emits refinement under `access`.
                self.optional(&mut out, "access", iri, "http://sbols.org/v2#refinement");
                out.insert(
                    "remote".into(),
                    self.scalar(iri, "http://sbols.org/v2#remote"),
                );
                out.insert(
                    "local".into(),
                    self.scalar(iri, "http://sbols.org/v2#local"),
                );
            }
            "Module" => {
                out.insert(
                    "definition".into(),
                    self.scalar(iri, "http://sbols.org/v2#definition"),
                );
                self.nested_field(&mut out, "mapsTos", iri, "http://sbols.org/v2#mapsTo");
            }
            "FunctionalComponent" => {
                self.optional(&mut out, "access", iri, "http://sbols.org/v2#access");
                // libSBOLj's serializeFunctionalComponent emits direction under
                // `access`, overwriting the access value.
                self.optional(&mut out, "access", iri, "http://sbols.org/v2#direction");
                out.insert(
                    "definition".into(),
                    self.scalar(iri, "http://sbols.org/v2#definition"),
                );
                self.nested_field(&mut out, "mapsTos", iri, "http://sbols.org/v2#mapsTo");
            }
            "Interaction" => {
                self.string_field(&mut out, "types", iri, "http://sbols.org/v2#type");
                self.nested_field(
                    &mut out,
                    "participations",
                    iri,
                    "http://sbols.org/v2#participation",
                );
            }
            "Participation" => {
                self.string_field(&mut out, "roles", iri, "http://sbols.org/v2#role");
                out.insert(
                    "participant".into(),
                    self.scalar(iri, "http://sbols.org/v2#participant"),
                );
            }
            _ => {}
        }
        serde_json::Value::Object(out)
    }

    /// Fill the `Identified` fields common to every object.
    fn identified(
        &self,
        iri: &str,
        local: &str,
        out: &mut serde_json::Map<String, serde_json::Value>,
    ) {
        out.insert("uri".into(), serde_json::Value::from(iri));
        self.optional(
            out,
            "persistentIdentity",
            iri,
            "http://sbols.org/v2#persistentIdentity",
        );
        self.optional(out, "displayId", iri, "http://sbols.org/v2#displayId");
        self.optional(out, "version", iri, "http://sbols.org/v2#version");
        self.string_field(
            out,
            "wasDerivedFroms",
            iri,
            "http://www.w3.org/ns/prov#wasDerivedFrom",
        );
        self.string_field(
            out,
            "wasGeneratedBys",
            iri,
            "http://www.w3.org/ns/prov#wasGeneratedBy",
        );
        let description = self
            .first_string(iri, "http://purl.org/dc/terms/description")
            .or_else(|| self.first_string(iri, "http://sbols.org/v2#description"));
        if let Some(description) = description {
            out.insert("description".into(), serde_json::Value::from(description));
        }
        let annotations = self.annotations(iri, local);
        if !annotations.is_empty() {
            out.insert("annotations".into(), serde_json::Value::Array(annotations));
        }
        self.string_field(out, "attachments", iri, "http://sbols.org/v2#attachment");
    }

    /// The `annotations` array: every predicate not consumed as a core field,
    /// each `{type, name, value}` with `uri` for an IRI object and `string` for
    /// a literal, matching libSBOLj.
    fn annotations(&self, iri: &str, local: &str) -> Vec<serde_json::Value> {
        let type_core = type_core(local);
        let mut out = Vec::new();
        if let Some(props) = self.by_subject.get(iri) {
            for entry in props {
                let predicate = entry.0;
                if IDENTIFIED_CORE.contains(&predicate) || type_core.contains(&predicate) {
                    continue;
                }
                let (kind, value) = match entry.1 {
                    ObjectTerm::Iri(term) => ("uri", term.as_str().to_owned()),
                    ObjectTerm::BlankNode(id) => ("uri", id.clone()),
                    ObjectTerm::Literal { value, .. } => ("string", value.clone()),
                };
                out.push(serde_json::json!({
                    "type": kind,
                    "name": predicate,
                    "value": value,
                }));
            }
        }
        out
    }

    /// Insert an array field of the string values of `predicate`, omitting it
    /// when empty (libSBOLj only emits non-empty collections).
    fn string_field(
        &self,
        out: &mut serde_json::Map<String, serde_json::Value>,
        key: &str,
        iri: &str,
        predicate: &str,
    ) {
        let values = self.string_array(iri, predicate);
        if !values.is_empty() {
            out.insert(
                key.to_owned(),
                serde_json::Value::Array(values.into_iter().map(serde_json::Value::from).collect()),
            );
        }
    }

    /// Insert an array field of the nested serialization of each object
    /// referenced by `predicate`, omitting it when empty.
    fn nested_field(
        &self,
        out: &mut serde_json::Map<String, serde_json::Value>,
        key: &str,
        iri: &str,
        predicate: &str,
    ) {
        let values: Vec<serde_json::Value> = self
            .objects(iri, predicate)
            .into_iter()
            .filter_map(|object| match object {
                ObjectTerm::Iri(term) => Some(self.object(term.as_str())),
                _ => None,
            })
            .collect();
        if !values.is_empty() {
            out.insert(key.to_owned(), serde_json::Value::Array(values));
        }
    }

    /// Insert a scalar string field only when `predicate` is present.
    fn optional(
        &self,
        out: &mut serde_json::Map<String, serde_json::Value>,
        key: &str,
        iri: &str,
        predicate: &str,
    ) {
        if let Some(value) = self.first_string(iri, predicate) {
            out.insert(key.to_owned(), serde_json::Value::from(value));
        }
    }

    /// A scalar string field's value, or the empty string when absent (matching
    /// libSBOLj's `toString()` of an unset property).
    fn scalar(&self, iri: &str, predicate: &str) -> serde_json::Value {
        serde_json::Value::from(self.first_string(iri, predicate).unwrap_or_default())
    }

    /// The first value of `predicate` as a JSON number when it parses as an
    /// integer, else as a string, or `None` when absent.
    fn number(&self, iri: &str, predicate: &str) -> Option<serde_json::Value> {
        let value = self.first_string(iri, predicate)?;
        match value.parse::<i64>() {
            Ok(number) => Some(serde_json::Value::from(number)),
            Err(_) => Some(serde_json::Value::from(value)),
        }
    }
}

/// An object-position term as the string libSBOLj's JSON serializer emits: an
/// IRI or blank node as its identifier, a literal as its lexical value.
fn term_string(term: &ObjectTerm) -> String {
    match term {
        ObjectTerm::Iri(iri) => iri.as_str().to_owned(),
        ObjectTerm::BlankNode(id) => id.clone(),
        ObjectTerm::Literal { value, .. } => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use sbol::v3::constants::{
        EDAM_IUPAC_DNA, ORIENTATION_INLINE, ORIENTATION_REVERSE_COMPLEMENT, SBO_DNA, SO_CDS,
        SO_PROMOTER,
    };
    use sbol::v3::SbolObject;

    use super::*;

    const NS: &str = "https://example.org/lab";

    /// Build a one-component, one-sequence SBOL3 document and return its
    /// N-Triples so the serializers can read it back as a closure would arrive.
    fn dna_document_triples(display_id: &str, elements: &str) -> Vec<Triple> {
        let sequence = Sequence::builder(NS, format!("{display_id}_sequence"))
            .expect("sequence builder")
            .elements(elements)
            .encoding(EDAM_IUPAC_DNA)
            .build()
            .expect("build sequence");
        let component = Component::builder(NS, display_id)
            .expect("component builder")
            .types([SBO_DNA])
            .name("Test part")
            .add_sequence(sequence.identity.clone())
            .build()
            .expect("build component");
        let document = Document::from_objects(vec![
            SbolObject::Component(component),
            SbolObject::Sequence(sequence),
        ])
        .expect("assemble document");
        let ntriples = document
            .write(sbol::RdfFormat::NTriples)
            .expect("write ntriples");
        let graph =
            sbol_rdf::Graph::parse(&ntriples, sbol_rdf::RdfFormat::NTriples).expect("parse");
        let placeholder = sbol_db_core::IriString::unchecked("");
        sbol_db_rdf::rdf_graph_to_triples(&graph, &placeholder)
    }

    #[test]
    fn fasta_round_trips_the_sequence() {
        let elements = "atgcaaatttcccgggtttaaaccc";
        let triples = dna_document_triples("bba_r0040", elements);

        let out = serialize_closure(&triples, SerializationFormat::Fasta, false).expect("fasta");
        assert_eq!(out.content_type, "chemical/x-fasta");
        let text = String::from_utf8(out.bytes).expect("utf8");

        // Re-import through the FASTA reader and confirm the residues survive.
        let (document, _report) = sbol_fasta::FastaImporter::new(NS)
            .expect("importer")
            .read_str(&text)
            .expect("re-import fasta");
        let sequence = document.sequences().next().expect("one sequence");
        assert_eq!(
            sequence.elements.as_deref().map(str::to_ascii_lowercase),
            Some(elements.to_ascii_lowercase()),
            "fasta export must round-trip the sequence elements"
        );
    }

    #[test]
    fn genbank_round_trips_the_sequence() {
        let elements = "atgcaaatttcccgggtttaaacccggg";
        let triples = dna_document_triples("bba_r0040", elements);

        let out =
            serialize_closure(&triples, SerializationFormat::GenBank, false).expect("genbank");
        assert_eq!(out.content_type, "chemical/x-genbank");
        let text = String::from_utf8(out.bytes).expect("utf8");

        // Re-import through the GenBank reader and confirm the residues survive.
        let (document, _report) = sbol_genbank::GenbankImporter::new(NS)
            .expect("importer")
            .read_str(&text)
            .expect("re-import genbank");
        let sequence = document.sequences().next().expect("one sequence");
        assert_eq!(
            sequence.elements.as_deref().map(str::to_ascii_lowercase),
            Some(elements.to_ascii_lowercase()),
            "genbank export must round-trip the sequence elements"
        );
    }

    #[test]
    fn rdf_xml_is_emitted_as_sbol3() {
        let triples = dna_document_triples("bba_r0040", "atgc");
        let out = serialize_closure(&triples, SerializationFormat::RdfXml, false).expect("rdfxml");
        assert_eq!(out.content_type, "application/rdf+xml");
        let text = String::from_utf8(out.bytes).expect("utf8");
        assert!(text.contains("RDF"), "expected an RDF/XML document: {text}");
    }

    #[test]
    fn rdf_xml_downgrades_to_sbol2_when_requested() {
        let triples = dna_document_triples("bba_r0040", "atgc");
        let out = serialize_closure(&triples, SerializationFormat::RdfXml, true).expect("sbol2");
        let text = String::from_utf8(out.bytes).expect("utf8");
        // The SBOL2 namespace appears only after the downgrade.
        assert!(
            text.contains("sbols.org/v2#"),
            "expected an SBOL2 document: {text}"
        );
    }

    /// A component with a `+`-strand promoter at 10..40 and a `-`-strand CDS at
    /// 50..100, returned as closure triples.
    fn feature_document_triples() -> Vec<Triple> {
        use sbol::v3::SbolObject::{Component as Comp, Range as Rng, SequenceFeature as SeqFeat};

        use sbol::v3::SbolObject::Sequence as Seq;

        let sequence = Sequence::builder(NS, "cassette_sequence")
            .expect("sequence builder")
            .elements("a".repeat(100))
            .encoding(EDAM_IUPAC_DNA)
            .build()
            .expect("build sequence");

        let seed = Component::builder(NS, "cassette")
            .expect("component seed")
            .types([SBO_DNA])
            .build()
            .expect("build seed");
        let parent = seed.identity.clone();

        let prom_range = Range::builder(&parent, "prom_range")
            .expect("range builder")
            .start(10)
            .end(40)
            .orientation(ORIENTATION_INLINE)
            .sequence(sequence.identity.clone())
            .build()
            .expect("build promoter range");
        let promoter = SequenceFeature::builder(&parent, "prom")
            .expect("feature builder")
            .roles([SO_PROMOTER])
            .name("Promoter")
            .add_location(prom_range.identity.clone())
            .build()
            .expect("build promoter");

        let cds_range = Range::builder(&parent, "cds_range")
            .expect("range builder")
            .start(50)
            .end(100)
            .orientation(ORIENTATION_REVERSE_COMPLEMENT)
            .sequence(sequence.identity.clone())
            .build()
            .expect("build cds range");
        let cds = SequenceFeature::builder(&parent, "cds")
            .expect("feature builder")
            .roles([SO_CDS])
            .name("LuxR")
            .add_location(cds_range.identity.clone())
            .build()
            .expect("build cds");

        let component = Component::builder(NS, "cassette")
            .expect("component builder")
            .types([SBO_DNA])
            .add_sequence(sequence.identity.clone())
            .add_feature(promoter.identity.clone())
            .add_feature(cds.identity.clone())
            .build()
            .expect("build component");

        let document = Document::from_objects(vec![
            Comp(component),
            Seq(sequence),
            SeqFeat(promoter),
            SeqFeat(cds),
            Rng(prom_range),
            Rng(cds_range),
        ])
        .expect("assemble document");
        let ntriples = document
            .write(sbol::RdfFormat::NTriples)
            .expect("write ntriples");
        let graph =
            sbol_rdf::Graph::parse(&ntriples, sbol_rdf::RdfFormat::NTriples).expect("parse");
        let placeholder = sbol_db_core::IriString::unchecked("");
        sbol_db_rdf::rdf_graph_to_triples(&graph, &placeholder)
    }

    #[test]
    fn gff3_projects_ranges_with_strand_and_type() {
        let triples = feature_document_triples();
        let out = serialize_gff3(&triples).expect("gff3");
        assert_eq!(out.content_type, "text/plain; charset=utf-8");
        let text = String::from_utf8(out.bytes).expect("utf8");

        assert!(
            text.starts_with("##gff-version 3\n"),
            "GFF3 must open with the version pragma: {text}"
        );
        assert!(
            text.contains("cassette\t.\tpromoter\t10\t40\t.\t+\t0\tID=prom;Name=Promoter"),
            "expected the promoter feature line: {text}"
        );
        assert!(
            text.contains("cassette\t.\tCDS\t50\t100\t.\t-\t0\tID=cds;Name=LuxR"),
            "expected the reverse-strand CDS feature line: {text}"
        );
    }

    #[test]
    fn omex_contains_manifest_sbol_and_metadata() {
        let triples = dna_document_triples("bba_r0040", "atgcaaatttccc");
        // Classic SynBioHub always downgrades to SBOL2 before archiving.
        let out = serialize_omex(&triples, true, None).expect("omex");
        assert_eq!(out.content_type, "application/zip");

        let mut archive =
            zip::ZipArchive::new(Cursor::new(out.bytes)).expect("open the produced zip");

        let mut manifest = String::new();
        archive
            .by_name("manifest.xml")
            .expect("manifest.xml entry")
            .read_to_string(&mut manifest)
            .expect("read manifest");
        assert!(
            manifest.contains(&format!(
                "location=\"./{OMEX_SBOL_ENTRY}\" format=\"{OMEX_SBOL2_FORMAT}\" master=\"true\""
            )),
            "manifest must list sbol.rdf as the SBOL2 master: {manifest}"
        );
        assert!(
            manifest.contains(&format!(
                "location=\"./{OMEX_METADATA_ENTRY}\" format=\"{OMEX_METADATA_FORMAT}\""
            )),
            "manifest must list the OMEX metadata member: {manifest}"
        );

        let mut sbol = String::new();
        archive
            .by_name(OMEX_SBOL_ENTRY)
            .expect("sbol.rdf entry")
            .read_to_string(&mut sbol)
            .expect("read sbol.rdf");
        let (document, _report) = sbol::convert::upgrade_from_sbol2(&sbol, sbol::RdfFormat::RdfXml)
            .expect("sbol.rdf re-parses as SBOL2");
        assert!(
            document.sequences().next().is_some(),
            "the archived SBOL should round-trip the sequence"
        );

        let mut metadata = String::new();
        archive
            .by_name(OMEX_METADATA_ENTRY)
            .expect("metadata.rdf entry")
            .read_to_string(&mut metadata)
            .expect("read metadata.rdf");
        assert!(
            metadata.contains(OMEX_SBOL_ENTRY) && metadata.contains("dcterms:created"),
            "metadata.rdf must describe the SBOL member's provenance: {metadata}"
        );
    }

    /// Build the SBOL2 triples of a `ComponentDefinition` carrying a `Sequence`,
    /// mirroring the shape SynBioHub stores verbatim.
    fn smoke_sbol2_triples() -> Vec<Triple> {
        fn iri(value: &str) -> sbol_db_core::IriString {
            sbol_db_core::IriString::unchecked(value)
        }
        fn triple(subject: &str, predicate: &str, object: ObjectTerm) -> Triple {
            Triple {
                graph_iri: None,
                subject: SubjectTerm::Iri(iri(subject)),
                predicate: iri(predicate),
                object,
            }
        }
        fn res(value: &str) -> ObjectTerm {
            ObjectTerm::Iri(iri(value))
        }
        fn lit(value: &str) -> ObjectTerm {
            ObjectTerm::Literal {
                value: value.to_owned(),
                datatype: iri("http://www.w3.org/2001/XMLSchema#string"),
                language: None,
            }
        }

        let cd = "http://synbiohub.org/public/smoke/pSmoke/1";
        let seq = "http://synbiohub.org/public/smoke/pSmoke_seq/1";
        vec![
            triple(cd, RDF_TYPE, res("http://sbols.org/v2#ComponentDefinition")),
            triple(cd, "http://sbols.org/v2#displayId", lit("pSmoke")),
            triple(cd, "http://sbols.org/v2#version", lit("1")),
            triple(
                cd,
                "http://sbols.org/v2#persistentIdentity",
                res("http://synbiohub.org/public/smoke/pSmoke"),
            ),
            triple(cd, "http://purl.org/dc/terms/title", lit("pSmoke promoter")),
            triple(
                cd,
                "http://purl.org/dc/terms/description",
                lit("a smoke-test promoter"),
            ),
            triple(
                cd,
                "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel",
                res(cd),
            ),
            triple(
                cd,
                "http://sbols.org/v2#type",
                res("http://www.biopax.org/release/biopax-level3.owl#DnaRegion"),
            ),
            triple(
                cd,
                "http://sbols.org/v2#role",
                res("http://identifiers.org/so/SO:0000167"),
            ),
            triple(cd, "http://sbols.org/v2#sequence", res(seq)),
            triple(seq, RDF_TYPE, res("http://sbols.org/v2#Sequence")),
            triple(seq, "http://sbols.org/v2#displayId", lit("pSmoke_seq")),
            triple(seq, "http://sbols.org/v2#elements", lit("ttgacg")),
            triple(
                seq,
                "http://sbols.org/v2#encoding",
                res("http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html"),
            ),
        ]
    }

    #[test]
    fn summary_matches_serialize_json_shape() {
        let out = serialize_summary(&smoke_sbol2_triples()).expect("summary");
        assert_eq!(out.content_type, "application/json");
        let value: serde_json::Value = serde_json::from_slice(&out.bytes).expect("json");

        let cds = value["componentDefinitions"]
            .as_array()
            .expect("componentDefinitions array");
        assert_eq!(cds.len(), 1);
        let cd = &cds[0];
        assert_eq!(cd["displayId"], "pSmoke");
        assert_eq!(cd["version"], "1");
        assert_eq!(cd["description"], "a smoke-test promoter");
        // dcterms:title is consumed as the object name and dropped by libSBOLj,
        // so it appears neither as a field nor as an annotation.
        assert!(cd.get("title").is_none(), "title must not be emitted: {cd}");
        assert_eq!(
            cd["types"][0],
            "http://www.biopax.org/release/biopax-level3.owl#DnaRegion"
        );
        assert_eq!(cd["roles"][0], "http://identifiers.org/so/SO:0000167");
        assert_eq!(
            cd["sequences"][0],
            "http://synbiohub.org/public/smoke/pSmoke_seq/1"
        );
        let annotations = cd["annotations"].as_array().expect("annotations");
        assert_eq!(annotations.len(), 1, "only the topLevel annotation: {cd}");
        assert_eq!(annotations[0]["type"], "uri");
        assert_eq!(
            annotations[0]["name"],
            "http://wiki.synbiohub.org/wiki/Terms/synbiohub#topLevel"
        );

        let sequences = value["sequences"].as_array().expect("sequences array");
        assert_eq!(sequences.len(), 1);
        assert_eq!(sequences[0]["elements"], "ttgacg");
        assert_eq!(
            sequences[0]["encoding"],
            "http://www.chem.qmul.ac.uk/iubmb/misc/naseq.html"
        );
    }
}
