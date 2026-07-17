//! Classic SynBioHub response shaping for the V1 query surface.
//!
//! Classic SynBioHub never returns raw SPARQL-results JSON to a V1 client. Its
//! `sparql.queryJson` folds a SPARQL 1.1 results document into a plain array of
//! row objects (`sparql-results-to-array.js`), and each API/view handler then
//! projects that array into its own wire object. The count family is served as
//! a bare integer in a `text/plain` body. This module reproduces those
//! projections so the adapter's bodies match classic byte-shape (JSON array of
//! objects, or a plain integer) rather than the engine's native SPARQL JSON.

use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Map, Value};

/// Flatten one SPARQL binding node to classic's scalar form, mirroring
/// `flattenBinding`: a URI or literal collapses to its bare string value, an
/// `xsd:integer`/`xsd:boolean` literal is coerced to a number/bool, and any
/// other node is passed through unchanged.
fn flatten_binding(node: &Value) -> Value {
    let Some(obj) = node.as_object() else {
        return node.clone();
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("uri") => obj.get("value").cloned().unwrap_or(Value::Null),
        Some("literal") | Some("typed-literal") => {
            let value = obj.get("value").and_then(Value::as_str).unwrap_or("");
            match obj.get("datatype").and_then(Value::as_str) {
                Some("http://www.w3.org/2001/XMLSchema#boolean") => Value::Bool(value == "true"),
                Some("http://www.w3.org/2001/XMLSchema#integer") => value
                    .parse::<i64>()
                    .map(|n| json!(n))
                    .unwrap_or(Value::Null),
                _ => Value::String(value.to_owned()),
            }
        }
        _ => node.clone(),
    }
}

/// Transform a SPARQL-results JSON document into classic's array of row objects:
/// one object per binding carrying every `head.var` as a key (null when the row
/// leaves it unbound), with each bound value flattened. Mirrors
/// `sparqlResultsToArray`.
pub fn results_to_array(results: &Value) -> Vec<Map<String, Value>> {
    let vars: Vec<String> = results
        .get("head")
        .and_then(|h| h.get("vars"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let empty = Vec::new();
    let bindings = results
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    bindings
        .iter()
        .map(|binding| {
            let mut row = Map::new();
            for var in &vars {
                let value = binding.get(var).map(flatten_binding).unwrap_or(Value::Null);
                row.insert(var.clone(), value);
            }
            row
        })
        .collect()
}

/// A JSON array body with the classic `application/json` content type.
fn json_array(rows: Vec<Value>) -> Response {
    (
        [(CONTENT_TYPE, "application/json")],
        Json(Value::Array(rows)),
    )
        .into_response()
}

/// A field value coerced with classic's `value || ''` fallback: a non-null
/// string passes through, anything else becomes the empty string.
fn string_or_empty(row: &Map<String, Value>, key: &str) -> Value {
    match row.get(key) {
        Some(Value::String(s)) => Value::String(s.clone()),
        _ => Value::String(String::new()),
    }
}

/// A field value coerced with classic's `value || null` fallback: a non-empty
/// string passes through, anything else becomes null.
fn string_or_null(row: &Map<String, Value>, key: &str) -> Value {
    match row.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Value::String(s.clone()),
        _ => Value::Null,
    }
}

/// `<uri>/metadata`: classic emits `sparqlResultsToArray` of the top-level
/// metadata query verbatim (an array of row objects keyed by the query's vars).
pub fn metadata_response(results: &Value) -> Response {
    let rows = results_to_array(results)
        .into_iter()
        .map(Value::Object)
        .collect();
    json_array(rows)
}

/// `/rootCollections` and `<uri>/subCollections`: classic maps each Collection
/// row to `{uri, name, description, displayId, version}`, renaming the
/// `?Collection` binding to `uri` and defaulting every text field to `''`.
pub fn collections_response(results: &Value) -> Response {
    let rows = results_to_array(results)
        .into_iter()
        .map(|row| {
            json!({
                "uri": row.get("Collection").cloned().unwrap_or(Value::Null),
                "name": string_or_empty(&row, "name"),
                "description": string_or_empty(&row, "description"),
                "displayId": string_or_empty(&row, "displayId"),
                "version": string_or_empty(&row, "version"),
            })
        })
        .collect();
    json_array(rows)
}

/// `/search`, `<uri>/uses`, `<uri>/twins`, `<uri>/similar` (and sequence
/// search): classic's search view projects each row to a fixed 11-key object,
/// renaming `?subject` to `uri`, filling an empty `name` from the `displayId`,
/// and defaulting the alignment columns. Text fields default to `''`; `sbolType`
/// and `role` default to null.
pub fn search_response(results: &Value) -> Response {
    let rows = results_to_array(results)
        .into_iter()
        .map(|row| search_row(&row))
        .collect();
    json_array(rows)
}

/// Project one search-result row into classic's 11-key wire object.
fn search_row(row: &Map<String, Value>) -> Value {
    let display_id = string_or_empty(row, "displayId");
    let mut name = string_or_empty(row, "name");
    if name == Value::String(String::new()) {
        name = display_id.clone();
    }
    json!({
        "type": string_or_empty(row, "type"),
        "uri": string_or_empty(row, "subject"),
        "name": name,
        "description": string_or_empty(row, "description"),
        "displayId": display_id,
        "version": string_or_empty(row, "version"),
        "sbolType": string_or_null(row, "sbolType"),
        "role": string_or_null(row, "role"),
        "percentMatch": string_or_empty(row, "percentMatch"),
        "strandAlignment": string_or_empty(row, "strandAlignment"),
        "CIGAR": string_or_empty(row, "CIGAR"),
    })
}

/// The count family (`/:type/count`, `/searchCount`, `/usesCount`,
/// `/twinsCount`, `/similarCount`): classic sends the bare integer in a
/// `text/plain` body.
pub fn count_response(results: &Value) -> Response {
    (
        [(CONTENT_TYPE, "text/plain")],
        count_value(results).to_string(),
    )
        .into_response()
}

/// The integer from a single-row `COUNT(...) AS ?count` result, or `0` when the
/// result carries no count binding.
fn count_value(results: &Value) -> i64 {
    results
        .get("results")
        .and_then(|r| r.get("bindings"))
        .and_then(Value::as_array)
        .and_then(|b| b.first())
        .and_then(|row| row.get("count"))
        .and_then(|c| c.get("value"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparql(vars: &[&str], bindings: Value) -> Value {
        json!({
            "head": { "vars": vars },
            "results": { "bindings": bindings },
        })
    }

    #[test]
    fn results_to_array_keys_every_var_and_flattens() {
        let doc = sparql(
            &["subject", "displayId", "version"],
            json!([
                {
                    "subject": { "type": "uri", "value": "http://ex/a" },
                    "displayId": { "type": "literal", "value": "a" },
                },
            ]),
        );
        let rows = results_to_array(&doc);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["subject"], json!("http://ex/a"));
        assert_eq!(rows[0]["displayId"], json!("a"));
        // An unbound var is present and null, matching sparqlResultsToArray.
        assert_eq!(rows[0]["version"], Value::Null);
    }

    #[test]
    fn integer_literals_are_coerced() {
        let doc = sparql(
            &["count"],
            json!([
                {
                    "count": {
                        "type": "literal",
                        "value": "7",
                        "datatype": "http://www.w3.org/2001/XMLSchema#integer",
                    },
                },
            ]),
        );
        let rows = results_to_array(&doc);
        assert_eq!(rows[0]["count"], json!(7));
        assert_eq!(count_value(&doc), 7);
    }

    #[test]
    fn count_value_defaults_to_zero() {
        let doc = sparql(&["count"], json!([]));
        assert_eq!(count_value(&doc), 0);
    }
}
