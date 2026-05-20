//! `sbol-db ontology` — fetch and inspect loaded OBO ontologies.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use sbol_db_postgres::SbolObjectService;

use crate::cli::OntologyAction;
use crate::output::print_json;

pub async fn run(service: Arc<SbolObjectService>, action: OntologyAction) -> Result<()> {
    match action {
        OntologyAction::Fetch { prefix, url, name } => {
            let (resolved_url, resolved_name) = resolve_ontology_source(&prefix, url, name)?;
            let report = service
                .ontology()
                .load_from_url(&prefix.to_ascii_uppercase(), &resolved_name, &resolved_url)
                .await?;
            print_json(&report)
        }
        OntologyAction::List => {
            let rows = service.ontology().list_ontologies().await?;
            print_json(&rows)
        }
        OntologyAction::Descendants { iri_or_curie } => {
            let canonical = canonical_term_iri(&iri_or_curie);
            let descendants = service.ontology().descendants(&canonical).await?;
            print_json(&descendants)
        }
        OntologyAction::Term { iri_or_curie } => {
            let canonical = canonical_term_iri(&iri_or_curie);
            let term = service
                .ontology()
                .get_term(&canonical)
                .await?
                .ok_or_else(|| anyhow!("no term {canonical}"))?;
            print_json(&term)
        }
        OntologyAction::LoadFile { path, prefix, name } => {
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let prefix_upper = prefix.to_ascii_uppercase();
            let resolved_name = name.unwrap_or_else(|| prefix_upper.clone());
            let report = service
                .ontology()
                .load_from_text(
                    &prefix_upper,
                    &resolved_name,
                    Some(&path.display().to_string()),
                    &body,
                )
                .await?;
            print_json(&report)
        }
    }
}

fn resolve_ontology_source(
    prefix: &str,
    url: Option<String>,
    name: Option<String>,
) -> Result<(String, String)> {
    let prefix_lower = prefix.to_ascii_lowercase();
    match prefix_lower.as_str() {
        "so" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/so.obo".to_owned()),
            name.unwrap_or_else(|| "Sequence Ontology".to_owned()),
        )),
        "sbo" => Ok((
            url.unwrap_or_else(|| "http://purl.obolibrary.org/obo/sbo.obo".to_owned()),
            name.unwrap_or_else(|| "Systems Biology Ontology".to_owned()),
        )),
        _ => {
            let url = url.ok_or_else(|| {
                anyhow!("--url is required for ontology prefix {prefix} (no default known)")
            })?;
            let name = name.unwrap_or_else(|| prefix.to_owned());
            Ok((url, name))
        }
    }
}

pub(crate) fn canonical_term_iri(input: &str) -> String {
    if input.contains("://") {
        return input.to_owned();
    }
    if let Some((prefix, suffix)) = input.split_once(':') {
        let p = prefix.to_ascii_uppercase();
        return format!("http://purl.obolibrary.org/obo/{p}_{suffix}");
    }
    input.to_owned()
}
