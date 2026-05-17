//! PgRow → JSON conversion for lab SQL results.
//!
//! sqlx's untyped row API hands back `PgValueRef`s tagged with a
//! Postgres OID / type name. We dispatch on the type name to produce a
//! reasonable JSON value: numbers stay numbers, booleans stay booleans,
//! arrays become JSON arrays, and anything we don't recognize falls
//! through to a text rendering. The lookup table is intentionally
//! limited to types that show up in the project schema; widening it is
//! a matter of adding more arms.
//!
//! Unknown types degrade to `null` with a sentinel string rather than
//! erroring — the lab UI surfaces the column type so a user can spot
//! "oh, we haven't taught the converter about `int4range` yet" and
//! cast in the query if they care.
//!
//! For values that can't be decoded at all, we return a sentinel
//! `"<undecodable: <type>>"` string. This keeps the result shape stable
//! row-to-row even when a single cell is misbehaved.

use serde_json::Value;
use sqlx::postgres::{PgRow, PgValueRef};
use sqlx::{Column, Row, TypeInfo, ValueRef};

use super::sql::Column as RespColumn;

/// Reshape a vector of `PgRow`s into the wire-shape used by
/// `/lab/api/sql/execute`. `limit` caps `rows` server-side; `truncated`
/// reports whether any rows were dropped. `total` is the count we
/// would have returned without truncation.
pub fn rows_to_json(rows: &[PgRow], limit: u32) -> (Vec<RespColumn>, Vec<Vec<Value>>, bool, u64) {
    let total = rows.len() as u64;
    let truncated = total > limit as u64;
    let take = rows.iter().take(limit as usize);

    let columns: Vec<RespColumn> = rows
        .first()
        .map(|row| {
            row.columns()
                .iter()
                .map(|c| RespColumn {
                    name: c.name().to_string(),
                    pg_type: c.type_info().name().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();

    let json_rows = take
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(i, col)| value_to_json(row, i, col.type_info().name()))
                .collect()
        })
        .collect();

    (columns, json_rows, truncated, total)
}

fn value_to_json(row: &PgRow, idx: usize, type_name: &str) -> Value {
    let raw: PgValueRef<'_> = match row.try_get_raw(idx) {
        Ok(v) => v,
        Err(_) => return Value::Null,
    };
    if raw.is_null() {
        return Value::Null;
    }

    // Dispatch on the Postgres type name. We branch on type_name (not
    // OID) because it's stable across protocol revisions and matches
    // what we ship back in the column metadata.
    match type_name {
        "BOOL" => decode::<bool>(row, idx)
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        "INT2" => decode::<i16>(row, idx)
            .map(|n| Value::from(n as i64))
            .unwrap_or(Value::Null),
        "INT4" => decode::<i32>(row, idx)
            .map(|n| Value::from(n as i64))
            .unwrap_or(Value::Null),
        "INT8" => decode::<i64>(row, idx)
            .map(Value::from)
            .unwrap_or(Value::Null),
        "FLOAT4" => decode::<f32>(row, idx)
            .and_then(|f| serde_json::Number::from_f64(f as f64).map(Value::Number))
            .unwrap_or(Value::Null),
        "FLOAT8" => decode::<f64>(row, idx)
            .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
            .unwrap_or(Value::Null),
        "NUMERIC" => {
            // sqlx feature-gates `bigdecimal`/`rust_decimal`; we don't
            // depend on either, so fall through to a text rendering.
            decode_via_text(row, idx)
        }
        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" | "CITEXT" => decode::<String>(row, idx)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "UUID" => decode::<uuid::Uuid>(row, idx)
            .map(|u| Value::String(u.to_string()))
            .unwrap_or(Value::Null),
        "TIMESTAMPTZ" => decode::<chrono::DateTime<chrono::Utc>>(row, idx)
            .map(|t| Value::String(t.to_rfc3339()))
            .unwrap_or(Value::Null),
        "TIMESTAMP" => decode::<chrono::NaiveDateTime>(row, idx)
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        "DATE" => decode::<chrono::NaiveDate>(row, idx)
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        "JSON" | "JSONB" => decode::<Value>(row, idx).unwrap_or(Value::Null),
        "BYTEA" => decode::<Vec<u8>>(row, idx)
            .map(|b| Value::String(hex::encode(b)))
            .unwrap_or(Value::Null),
        "TEXT[]" | "VARCHAR[]" | "_TEXT" | "_VARCHAR" | "_BPCHAR" | "_NAME" => {
            decode::<Vec<String>>(row, idx)
                .map(|v| Value::Array(v.into_iter().map(Value::String).collect()))
                .unwrap_or(Value::Null)
        }
        "_INT4" => decode::<Vec<i32>>(row, idx)
            .map(|v| Value::Array(v.into_iter().map(|i| Value::from(i as i64)).collect()))
            .unwrap_or(Value::Null),
        "_INT8" => decode::<Vec<i64>>(row, idx)
            .map(|v| Value::Array(v.into_iter().map(Value::from).collect()))
            .unwrap_or(Value::Null),
        _ => {
            // Custom Postgres domain (sbol_iri, sbol_ontology_term, …)
            // arrives here. Most of these are text under the covers.
            if let Some(s) = decode::<String>(row, idx) {
                return Value::String(s);
            }
            decode_via_text(row, idx)
        }
    }
}

fn decode<'r, T>(row: &'r PgRow, idx: usize) -> Option<T>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(idx).ok()
}

fn decode_via_text(row: &PgRow, idx: usize) -> Value {
    // PgValueRef carries the raw bytes; the simplest stable fallback is
    // to attempt a String decode (works for any type that has a text
    // encoder, which is most of them). If that fails too, surface a
    // descriptive sentinel rather than dropping the row.
    if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
        return Value::String(s);
    }
    let type_name = row
        .columns()
        .get(idx)
        .map(|c| c.type_info().name().to_string())
        .unwrap_or_else(|| "unknown".into());
    Value::String(format!("<undecodable: {type_name}>"))
}
