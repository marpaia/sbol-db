//! Graph neighborhood traversal over `sbol_quads`.
//!
//! Uses a recursive CTE bounded by depth and an optional node-count cap.
//! Forward, backward, and bidirectional walks share one query; an additional
//! pass joins back to `sbol_objects` for human-friendly node metadata.

use std::collections::{BTreeMap, BTreeSet};

use sbol_db_core::{
    Direction, DomainError, EdgeInfo, EdgeObject, IriString, NeighborhoodQuery, NeighborhoodResult,
    NodeInfo,
};
use sqlx::Row;

use crate::repo::db_err;
use crate::PgPool;

#[derive(Clone)]
pub struct NeighborhoodRepository {
    pool: PgPool,
}

impl NeighborhoodRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn walk(&self, query: &NeighborhoodQuery) -> Result<NeighborhoodResult, DomainError> {
        let depth_cap = i32::try_from(query.depth).unwrap_or(i32::MAX);
        let max_nodes_cap = query
            .max_nodes
            .map(|m| i32::try_from(m).unwrap_or(i32::MAX))
            .unwrap_or(i32::MAX);
        let predicate_filter: Option<Vec<String>> = if query.predicate_allowlist.is_empty() {
            None
        } else {
            Some(
                query
                    .predicate_allowlist
                    .iter()
                    .map(|i| i.as_str().to_owned())
                    .collect(),
            )
        };

        // The CTE walks resource-position edges only (literals never widen the
        // frontier). `dir` selects which direction we follow on each step.
        // `Both` is modelled as UNIONing the forward and backward expansions.
        let dir_tag = match query.direction {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
            Direction::Both => "both",
        };

        let rows = sqlx::query(SQL_WALK)
            .bind(query.root_iri.as_str())
            .bind(depth_cap)
            .bind(predicate_filter.as_ref())
            .bind(dir_tag)
            .bind(max_nodes_cap)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;

        let mut nodes_by_id: BTreeMap<String, NodeInfo> = BTreeMap::new();
        let mut edges: Vec<EdgeInfo> = Vec::new();
        let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut max_depth = 0u32;

        // The walk SELECT returns one row per reached node *and* the edge that
        // led to it (subject, predicate, object position). The root has
        // depth=0 and no edge.
        for row in &rows {
            let depth: i32 = row.try_get("depth").map_err(db_err)?;
            let depth_u: u32 = depth.max(0) as u32;
            max_depth = max_depth.max(depth_u);

            let node_id: String = row.try_get("node_id").map_err(db_err)?;
            let is_blank: bool = row.try_get("node_is_blank").map_err(db_err)?;
            nodes_by_id
                .entry(node_id.clone())
                .or_insert_with(|| NodeInfo {
                    id: node_id.clone(),
                    depth: depth_u,
                    is_blank,
                    sbol_class: None,
                    display_id: None,
                    name: None,
                });

            // The seed row has no edge — depth=0, edge_subject NULL.
            let edge_subject: Option<String> = row.try_get("edge_subject").map_err(db_err)?;
            if let Some(subject) = edge_subject {
                let subject_blank: bool = row.try_get("edge_subject_is_blank").map_err(db_err)?;
                let predicate: String = row.try_get("edge_predicate").map_err(db_err)?;
                let object_iri: Option<String> = row.try_get("edge_object_iri").map_err(db_err)?;
                let object_blank: Option<String> =
                    row.try_get("edge_object_blank").map_err(db_err)?;
                let object_literal: Option<String> =
                    row.try_get("edge_object_literal").map_err(db_err)?;
                let object_datatype: Option<String> =
                    row.try_get("edge_object_datatype").map_err(db_err)?;
                let object_language: Option<String> =
                    row.try_get("edge_object_language").map_err(db_err)?;
                let object = if let Some(iri) = object_iri.clone() {
                    EdgeObject::Iri { value: iri }
                } else if let Some(blank) = object_blank.clone() {
                    EdgeObject::BlankNode { value: blank }
                } else if let Some(lit) = object_literal.clone() {
                    EdgeObject::Literal {
                        value: lit,
                        datatype: object_datatype.unwrap_or_else(|| {
                            "http://www.w3.org/2001/XMLSchema#string".to_owned()
                        }),
                        language: object_language,
                    }
                } else {
                    continue;
                };
                let object_key = match &object {
                    EdgeObject::Iri { value } | EdgeObject::BlankNode { value } => value.clone(),
                    EdgeObject::Literal { value, .. } => format!("\u{1}lit\u{1}{value}"),
                };
                if seen_edges.insert((subject.clone(), predicate.clone(), object_key)) {
                    edges.push(EdgeInfo {
                        subject,
                        subject_is_blank: subject_blank,
                        predicate,
                        object,
                        depth: depth_u,
                    });
                }
            }
        }

        // Optionally include literal-position edges by sweeping all literals
        // out of each visited subject. Done as a second query so the CTE
        // stays focused on resource walks.
        if query.include_literals {
            let ids: Vec<&str> = nodes_by_id.keys().map(String::as_str).collect();
            let literal_rows = sqlx::query(SQL_LITERAL_EDGES)
                .bind(&ids)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            for row in literal_rows {
                let subject: String = row.try_get("subject").map_err(db_err)?;
                let predicate: String = row.try_get("predicate").map_err(db_err)?;
                let value: String = row.try_get("value").map_err(db_err)?;
                let datatype: Option<String> = row.try_get("datatype").map_err(db_err)?;
                let language: Option<String> = row.try_get("language").map_err(db_err)?;
                let depth = nodes_by_id.get(&subject).map(|n| n.depth).unwrap_or(0);
                let key = (
                    subject.clone(),
                    predicate.clone(),
                    format!("\u{1}lit\u{1}{value}"),
                );
                if seen_edges.insert(key) {
                    edges.push(EdgeInfo {
                        subject,
                        subject_is_blank: false,
                        predicate,
                        object: EdgeObject::Literal {
                            value,
                            datatype: datatype.unwrap_or_else(|| {
                                "http://www.w3.org/2001/XMLSchema#string".to_owned()
                            }),
                            language,
                        },
                        depth,
                    });
                }
            }
        }

        // Decorate IRI nodes with sbol_class/display_id/name in one query.
        let iri_keys: Vec<&str> = nodes_by_id
            .values()
            .filter(|n| !n.is_blank)
            .map(|n| n.id.as_str())
            .collect();
        if !iri_keys.is_empty() {
            let meta_rows = sqlx::query(SQL_NODE_METADATA)
                .bind(&iri_keys)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            for row in meta_rows {
                let iri: String = row.try_get("iri").map_err(db_err)?;
                if let Some(node) = nodes_by_id.get_mut(&iri) {
                    node.sbol_class = row.try_get("sbol_class").map_err(db_err)?;
                    node.display_id = row.try_get("display_id").map_err(db_err)?;
                    node.name = row.try_get("name").map_err(db_err)?;
                }
            }
        }

        let nodes: Vec<NodeInfo> = nodes_by_id.into_values().collect();
        let truncated = nodes.len() as i32 >= max_nodes_cap;

        Ok(NeighborhoodResult {
            root_iri: query.root_iri.clone(),
            nodes,
            edges,
            max_depth_reached: max_depth,
            truncated,
        })
    }
}

/// The recursive walk. Postgres only allows one self-reference in the
/// recursive term, so we normalize both directions into a non-recursive
/// `edges` CTE and walk that. Each `edges` row is the *outgoing* view of a
/// single quad: `(from_id, predicate, to_id)` plus the original subject /
/// object position for surfacing the edge in the result.
///
/// `$1 = root iri, $2 = depth cap, $3 = predicate allowlist (nullable),
/// $4 = direction ('forward'|'backward'|'both'), $5 = node count cap.`
const SQL_WALK: &str = r#"
WITH RECURSIVE
edges AS (
    SELECT
        COALESCE(subject_iri::text, subject_blank) AS from_id,
        (subject_iri IS NULL AND subject_blank IS NOT NULL) AS from_is_blank,
        predicate_iri::text                        AS predicate,
        COALESCE(object_iri::text, object_blank)   AS to_id,
        (object_iri IS NULL AND object_blank IS NOT NULL) AS to_is_blank,
        subject_iri::text                          AS edge_subject_iri,
        subject_blank                              AS edge_subject_blank,
        object_iri::text                           AS edge_object_iri,
        object_blank                               AS edge_object_blank
    FROM sbol_quads
    WHERE $4 IN ('forward', 'both')
      AND (object_iri IS NOT NULL OR object_blank IS NOT NULL)

    UNION ALL

    SELECT
        COALESCE(object_iri::text, object_blank),
        (object_iri IS NULL AND object_blank IS NOT NULL),
        predicate_iri::text,
        COALESCE(subject_iri::text, subject_blank),
        (subject_iri IS NULL AND subject_blank IS NOT NULL),
        subject_iri::text,
        subject_blank,
        object_iri::text,
        object_blank
    FROM sbol_quads
    WHERE $4 IN ('backward', 'both')
      AND (object_iri IS NOT NULL OR object_blank IS NOT NULL)
),
walk AS (
    SELECT
        $1::text AS node_id,
        false    AS node_is_blank,
        0        AS depth,
        ARRAY[$1::text] AS path,
        NULL::text AS edge_subject,
        false      AS edge_subject_is_blank,
        NULL::text AS edge_predicate,
        NULL::text AS edge_object_iri,
        NULL::text AS edge_object_blank,
        NULL::text AS edge_object_literal,
        NULL::text AS edge_object_datatype,
        NULL::text AS edge_object_language

    UNION ALL

    SELECT
        e.to_id,
        e.to_is_blank,
        w.depth + 1,
        w.path || e.to_id,
        COALESCE(e.edge_subject_iri, e.edge_subject_blank),
        (e.edge_subject_iri IS NULL AND e.edge_subject_blank IS NOT NULL),
        e.predicate,
        e.edge_object_iri,
        e.edge_object_blank,
        NULL::text,
        NULL::text,
        NULL::text
    FROM walk w
    JOIN edges e
      ON e.from_id = w.node_id
     AND e.from_is_blank = w.node_is_blank
    WHERE w.depth < $2
      AND ($3::text[] IS NULL OR e.predicate = ANY($3))
      AND NOT e.to_id = ANY(w.path)
)
SELECT *
FROM walk
LIMIT $5
"#;

const SQL_LITERAL_EDGES: &str = r#"
SELECT
    COALESCE(subject_iri::text, subject_blank) AS subject,
    predicate_iri::text                        AS predicate,
    object_literal                             AS value,
    datatype_iri::text                         AS datatype,
    language                                   AS language
FROM sbol_quads
WHERE object_literal IS NOT NULL
  AND COALESCE(subject_iri::text, subject_blank) = ANY($1::text[])
"#;

const SQL_NODE_METADATA: &str = r#"
SELECT iri::text AS iri, sbol_class, display_id, name
FROM sbol_objects
WHERE iri::text = ANY($1::text[])
  AND is_deleted = false
"#;

#[allow(dead_code)]
fn opt_str(value: Option<&IriString>) -> Option<&str> {
    value.map(|i| i.as_str())
}
