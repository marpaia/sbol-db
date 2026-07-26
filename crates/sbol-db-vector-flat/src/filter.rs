use std::collections::BTreeMap;

use sbol_db_search_sdk::{VectorError, VectorFilter};
use serde_json::Value;

pub(crate) fn matches(
    payload: &BTreeMap<String, Value>,
    filter: &VectorFilter,
) -> Result<bool, VectorError> {
    match filter {
        VectorFilter::Match { field, value } => Ok(lookup(payload, field) == Some(value)),
        VectorFilter::Any { field, values } => {
            let Some(actual) = lookup(payload, field) else {
                return Ok(false);
            };
            Ok(values.contains(actual))
        }
        VectorFilter::Range { field, gte, lte } => {
            if gte.is_none() && lte.is_none() {
                return Err(VectorError::InvalidRequest(format!(
                    "range filter for {field:?} has no bounds"
                )));
            }
            let Some(actual) = lookup(payload, field).and_then(Value::as_f64) else {
                return Ok(false);
            };
            Ok(gte.is_none_or(|bound| actual >= bound) && lte.is_none_or(|bound| actual <= bound))
        }
        VectorFilter::And { clauses } => {
            for clause in clauses {
                if !matches(payload, clause)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        VectorFilter::Or { clauses } => {
            for clause in clauses {
                if matches(payload, clause)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        VectorFilter::Not { clause } => Ok(!matches(payload, clause)?),
    }
}

fn lookup<'a>(payload: &'a BTreeMap<String, Value>, field: &str) -> Option<&'a Value> {
    let mut segments = field.split('.');
    let first = segments.next()?;
    if first.is_empty() {
        return None;
    }

    let mut value = payload.get(first)?;
    for segment in segments {
        value = value.as_object()?.get(segment)?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates_nested_boolean_filters() {
        let payload = BTreeMap::from([
            ("graph".to_owned(), json!("public")),
            ("metadata".to_owned(), json!({"year": 2026})),
        ]);
        let filter = VectorFilter::And {
            clauses: vec![
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("public"),
                },
                VectorFilter::Range {
                    field: "metadata.year".to_owned(),
                    gte: Some(2025.0),
                    lte: None,
                },
            ],
        };

        assert!(matches(&payload, &filter).unwrap());
    }
}
