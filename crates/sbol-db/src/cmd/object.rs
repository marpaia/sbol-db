//! `sbol-db object` — operations on stored objects.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use sbol_db_core::{GraphId, ObjectId, SerializationFormat};
use sbol_db_postgres::{ListObjectsFilter, SbolObjectService};

use crate::cli::ObjectAction;
use crate::format::parse_format;
use crate::output::{print_json, write_jsonl};

pub async fn run(service: Arc<SbolObjectService>, action: ObjectAction) -> Result<()> {
    match action {
        ObjectAction::Get { iri, json } => {
            let obj = service
                .objects()
                .get_by_iri(&iri)
                .await?
                .ok_or_else(|| anyhow!("not found: {iri}"))?;
            if json {
                print_json(&obj.data)
            } else {
                print_json(&obj)
            }
        }
        ObjectAction::Export { iri, format } => {
            let format =
                parse_format(&format).ok_or_else(|| anyhow!("unknown format: {format}"))?;
            let obj = service
                .objects()
                .get_by_iri(&iri)
                .await?
                .ok_or_else(|| anyhow!("not found: {iri}"))?;
            let body = render_subgraph(service.clone(), obj.id, format).await?;
            print!("{body}");
            Ok(())
        }
        ObjectAction::ExportAll {
            sbol_class,
            role,
            graph_id,
            page_size,
        } => export_all(service, sbol_class, role, graph_id, page_size).await,
    }
}

async fn render_subgraph(
    service: Arc<SbolObjectService>,
    object_id: ObjectId,
    format: SerializationFormat,
) -> Result<String> {
    let iri = service
        .objects()
        .get_iri_by_id(object_id)
        .await?
        .ok_or_else(|| anyhow!("object id not found"))?;
    let body = sbol_db_server::export_subject_rdf(service.triples(), &iri, format).await?;
    Ok(body)
}

async fn export_all(
    service: Arc<SbolObjectService>,
    sbol_class: Option<String>,
    role: Option<String>,
    graph_id: Option<uuid::Uuid>,
    page_size: u32,
) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut cursor: Option<String> = None;
    let limit = page_size.clamp(1, 5000);
    loop {
        let filter = ListObjectsFilter {
            sbol_class: sbol_class.clone(),
            role: role.clone(),
            graph_id: graph_id.map(GraphId),
            after_iri: cursor.clone(),
            limit,
        };
        let page = service.objects().list(&filter).await?;
        let page_len = page.len();
        for record in &page {
            write_jsonl(&mut out, record)?;
        }
        if (page_len as u32) < limit {
            break;
        }
        cursor = page.last().map(|r| r.iri.as_str().to_owned());
        if cursor.is_none() {
            break;
        }
    }
    Ok(())
}
