use qdrant_client::qdrant::{r#match::MatchValue, Condition, Filter, Range};
use sbol_db_search_sdk::{VectorError, VectorFilter};
use serde_json::Value;

pub(crate) fn translate(filter: &VectorFilter) -> Result<Filter, VectorError> {
    Ok(match filter {
        VectorFilter::Match { field, value } => Filter::must([condition_for_value(field, value)?]),
        VectorFilter::Any { field, values } => {
            if values.is_empty() {
                return Err(VectorError::InvalidRequest(format!(
                    "any filter for {field:?} has no values"
                )));
            }
            Filter::should(
                values
                    .iter()
                    .map(|value| condition_for_value(field, value))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        VectorFilter::Range { field, gte, lte } => {
            if gte.is_none() && lte.is_none() {
                return Err(VectorError::InvalidRequest(format!(
                    "range filter for {field:?} has no bounds"
                )));
            }
            Filter::must([Condition::range(
                field,
                Range {
                    gte: *gte,
                    lte: *lte,
                    ..Default::default()
                },
            )])
        }
        VectorFilter::And { clauses } => {
            if clauses.is_empty() {
                return Err(VectorError::InvalidRequest(
                    "and filter has no clauses".to_owned(),
                ));
            }
            Filter::must(
                clauses
                    .iter()
                    .map(translate)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(Condition::from),
            )
        }
        VectorFilter::Or { clauses } => {
            if clauses.is_empty() {
                return Err(VectorError::InvalidRequest(
                    "or filter has no clauses".to_owned(),
                ));
            }
            Filter::should(
                clauses
                    .iter()
                    .map(translate)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(Condition::from),
            )
        }
        VectorFilter::Not { clause } => Filter::must_not([Condition::from(translate(clause)?)]),
    })
}

fn condition_for_value(field: &str, value: &Value) -> Result<Condition, VectorError> {
    let condition = match value {
        Value::Null => Condition::is_null(field),
        Value::Bool(value) => Condition::matches(field, MatchValue::Boolean(*value)),
        Value::String(value) => Condition::matches(field, MatchValue::Keyword(value.clone())),
        Value::Number(value) => {
            let Some(value) = value.as_i64() else {
                return Err(VectorError::Unsupported(format!(
                    "Qdrant exact-match filters require integral JSON numbers for field {field:?}"
                )));
            };
            Condition::matches(field, MatchValue::Integer(value))
        }
        Value::Array(_) | Value::Object(_) => {
            return Err(VectorError::Unsupported(format!(
                "Qdrant exact-match filters do not accept compound JSON values for field {field:?}"
            )));
        }
    };
    Ok(condition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::condition::ConditionOneOf;
    use serde_json::json;

    #[test]
    fn preserves_nested_boolean_structure() {
        let translated = translate(&VectorFilter::And {
            clauses: vec![
                VectorFilter::Match {
                    field: "graph".to_owned(),
                    value: json!("public graph"),
                },
                VectorFilter::Not {
                    clause: Box::new(VectorFilter::Range {
                        field: "year".to_owned(),
                        gte: Some(2027.0),
                        lte: None,
                    }),
                },
            ],
        })
        .unwrap();

        assert_eq!(translated.must.len(), 2);
        assert!(translated.must.iter().all(|condition| matches!(
            condition.condition_one_of,
            Some(ConditionOneOf::Filter(_))
        )));
    }

    #[test]
    fn rejects_values_qdrant_cannot_match_exactly() {
        let result = translate(&VectorFilter::Match {
            field: "weight".to_owned(),
            value: json!(1.5),
        });
        assert!(matches!(result, Err(VectorError::Unsupported(_))));
    }
}
