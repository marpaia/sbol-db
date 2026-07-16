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

use std::collections::HashMap;
use std::io::{Cursor, Write};

use gb_io::seq::{After, Before, Feature, Location, Qualifier, Seq, Topology};
use sbol::constants::{SBO_PROTEIN, SBO_RNA, SO_CIRCULAR};
use sbol::{Component, Document, Range, Sequence, SequenceFeature, SubComponent};
use sbol_db_core::{DomainError, ObjectTerm, SerializationFormat, Triple};
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
/// The archive-relative name of the SBOL RDF member.
const OMEX_SBOL_ENTRY: &str = "sbol.rdf";

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
            Document::upgrade_from_sbol2(&ntriples, sbol::RdfFormat::NTriples)
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

/// Write every [`Sequence`] in the document as a FASTA record: a `>` header
/// carrying the sequence's display id and name, followed by the residues
/// wrapped at [`FASTA_LINE_WIDTH`].
fn to_fasta(document: &Document) -> String {
    use sbol::SbolIdentified;

    let mut out = String::new();
    for sequence in document.sequences() {
        let elements = match sequence.elements.as_deref() {
            Some(elements) if !elements.is_empty() => elements,
            _ => continue,
        };
        let id = sequence
            .display_id()
            .or_else(|| sequence.identity.as_iri().map(|iri| iri.as_str()))
            .unwrap_or("sequence");
        out.push('>');
        out.push_str(id);
        if let Some(name) = sequence.name() {
            out.push(' ');
            out.push_str(name);
        }
        out.push('\n');
        for chunk in elements.as_bytes().chunks(FASTA_LINE_WIDTH) {
            out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
            out.push('\n');
        }
    }
    out
}

/// Write every [`Component`] that carries a resolvable sequence as one GenBank
/// record, mapping its `SequenceFeature`s to GenBank features. Rendering goes
/// through `gb_io` so the output parses back through the same engine
/// `sbol-genbank` imports with.
fn to_genbank(document: &Document) -> Result<Vec<u8>, DomainError> {
    use sbol::SbolIdentified;

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
    use sbol::SbolIdentified;

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
    use sbol::SbolIdentified;

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

/// Pack the closure into a COMBINE (OMEX) archive: a zip of `manifest.xml` and
/// `sbol.rdf` (the closure as RDF/XML, downgraded to SBOL2 when `sbol2` is set),
/// with the SBOL RDF marked as the archive master. When an
/// [`OmexAttachmentSource`] is supplied, each attachment blob is added as an
/// archive entry and listed in the manifest; with `None` the archive is valid
/// with just the manifest and the SBOL RDF.
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
/// SBOL RDF master, and any attachment members.
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

/// Escape the XML attribute metacharacters in a manifest value.
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use sbol::constants::{
        EDAM_IUPAC_DNA, ORIENTATION_INLINE, ORIENTATION_REVERSE_COMPLEMENT, SBO_DNA, SO_CDS,
        SO_PROMOTER,
    };
    use sbol::SbolObject;

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
        use sbol::SbolObject::{Component as Comp, Range as Rng, SequenceFeature as SeqFeat};

        use sbol::SbolObject::Sequence as Seq;

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
    fn omex_contains_manifest_and_sbol() {
        let triples = dna_document_triples("bba_r0040", "atgcaaatttccc");
        let out = serialize_omex(&triples, false, None).expect("omex");
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
                "location=\"./{OMEX_SBOL_ENTRY}\" format=\"{OMEX_SBOL3_FORMAT}\" master=\"true\""
            )),
            "manifest must list sbol.rdf as the SBOL master: {manifest}"
        );

        let mut sbol = String::new();
        archive
            .by_name(OMEX_SBOL_ENTRY)
            .expect("sbol.rdf entry")
            .read_to_string(&mut sbol)
            .expect("read sbol.rdf");
        let document =
            Document::read(&sbol, sbol::RdfFormat::RdfXml).expect("sbol.rdf re-parses as SBOL3");
        assert!(
            document.sequences().next().is_some(),
            "the archived SBOL should round-trip the sequence"
        );
    }
}
