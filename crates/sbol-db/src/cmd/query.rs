//! `sbol-db query` — read paths against the stored graph.

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use sbol_db_core::{IriString, NeighborhoodQuery};
use sbol_db_postgres::{SbolObjectService, SequenceSearchOptions};
use sbol_db_sparql::{parse_query, ResultFormat, SparqlEngine, SparqlOptions};

use crate::cli::QueryAction;
use crate::format::{parse_direction, parse_format};
use crate::output::{print_json, write_jsonl};

pub async fn run(service: Arc<SbolObjectService>, action: QueryAction) -> Result<()> {
    match action {
        QueryAction::Sparql {
            source,
            format,
            timeout_secs,
            max_rows,
            max_query_size,
        } => {
            sparql(
                service,
                source,
                format,
                timeout_secs,
                max_rows,
                max_query_size,
            )
            .await
        }
        QueryAction::Explain { source } => {
            let query = read_query_source(&source)?;
            let parsed = parse_query(&query).map_err(|e| anyhow!("{e}"))?;
            print_json(&parsed)
        }
        QueryAction::Neighborhood {
            iri,
            depth,
            direction,
            predicates,
            max_nodes,
            literals,
            rdf,
        } => {
            neighborhood(
                service, iri, depth, direction, predicates, max_nodes, literals, rdf,
            )
            .await
        }
        QueryAction::SequenceSearch {
            pattern,
            max_hits,
            forward_only,
        } => {
            let matches = service
                .sequence_search()
                .search(
                    &pattern,
                    SequenceSearchOptions {
                        max_hits: Some(max_hits),
                        forward_only: if forward_only { Some(true) } else { None },
                    },
                )
                .await?;
            print_json(&matches)
        }
        QueryAction::SequenceBatch {
            source,
            max_hits,
            forward_only,
        } => sequence_batch(service, source, max_hits, forward_only).await,
    }
}

async fn sparql(
    service: Arc<SbolObjectService>,
    source: String,
    format: Option<String>,
    timeout_secs: u64,
    max_rows: usize,
    max_query_size: usize,
) -> Result<()> {
    let query = read_query_source(&source)?;
    let format = format
        .map(|f| f.parse::<ResultFormat>())
        .transpose()
        .map_err(|e| anyhow!("{e}"))?;
    let engine = SparqlEngine::new(Arc::new(service.triples().clone()));
    let options = SparqlOptions {
        timeout: Duration::from_secs(timeout_secs),
        max_rows,
        max_query_size,
        default_graph: None,
    };
    let outcome = engine
        .execute(&query, format, &options)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    std::io::stdout().write_all(&outcome.payload.body)?;
    if outcome.payload.truncated {
        eprintln!("(result truncated at --max-rows={max_rows})");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn neighborhood(
    service: Arc<SbolObjectService>,
    iri: String,
    depth: u32,
    direction: String,
    predicates: Vec<String>,
    max_nodes: u32,
    literals: bool,
    rdf: Option<String>,
) -> Result<()> {
    let direction = parse_direction(&direction)?;
    let query = NeighborhoodQuery {
        root_iri: IriString::new(&iri)?,
        depth,
        direction,
        predicate_allowlist: predicates.into_iter().map(IriString::unchecked).collect(),
        max_nodes: Some(max_nodes),
        include_literals: literals,
    };
    let result = service.neighborhood().walk(&query).await?;
    if let Some(fmt) = rdf {
        let fmt = parse_format(&fmt).ok_or_else(|| anyhow!("unknown format: {fmt}"))?;
        let body = sbol_db_rdf::neighborhood_to_rdf(&result, fmt)?;
        print!("{body}");
        Ok(())
    } else {
        print_json(&result)
    }
}

async fn sequence_batch(
    service: Arc<SbolObjectService>,
    source: String,
    max_hits: u32,
    forward_only: bool,
) -> Result<()> {
    let body = if source == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(&source).with_context(|| format!("reading {source}"))?
    };
    let patterns: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let opts = SequenceSearchOptions {
        max_hits: Some(max_hits),
        forward_only: if forward_only { Some(true) } else { None },
    };
    let results = service
        .sequence_search()
        .search_many(&patterns, opts)
        .await?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for row in &results {
        write_jsonl(&mut out, row)?;
    }
    Ok(())
}

fn read_query_source(source: &str) -> Result<String> {
    if source == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return Ok(buf);
    }
    let path = std::path::Path::new(source);
    std::fs::read_to_string(path).with_context(|| format!("reading SPARQL source {source}"))
}
