use std::collections::HashMap;

use roaring::RoaringBitmap;
use sbol_db_search_sdk::{VectorError, VectorFilter};
use serde_json::Value;

use crate::model::StoredRecord;

pub(crate) struct PayloadIndex {
    universe: RoaringBitmap,
    exact: HashMap<(String, String), RoaringBitmap>,
    numeric: HashMap<String, Vec<(f64, u32)>>,
}

impl PayloadIndex {
    pub(crate) fn build(records: &[StoredRecord]) -> Result<Self, VectorError> {
        let mut index = Self {
            universe: RoaringBitmap::new(),
            exact: HashMap::new(),
            numeric: HashMap::new(),
        };
        for (id, record) in records.iter().enumerate() {
            let id = u32::try_from(id).map_err(|_| {
                VectorError::Unsupported(
                    "FAISS embedded backend supports at most u32::MAX documents per generation"
                        .to_owned(),
                )
            })?;
            index.universe.insert(id);
            for (field, value) in &record.payload {
                index.insert_value(field, value, id)?;
            }
        }
        for values in index.numeric.values_mut() {
            values.sort_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)));
        }
        Ok(index)
    }

    pub(crate) fn allowed(&self, filter: &VectorFilter) -> Result<Vec<i64>, VectorError> {
        Ok(self.evaluate(filter)?.iter().map(i64::from).collect())
    }

    fn insert_value(&mut self, field: &str, value: &Value, id: u32) -> Result<(), VectorError> {
        let encoded = serde_json::to_string(value).map_err(|error| {
            VectorError::Backend(format!("cannot index payload field {field:?}: {error}"))
        })?;
        self.exact
            .entry((field.to_owned(), encoded))
            .or_default()
            .insert(id);
        if let Some(number) = value.as_f64().filter(|number| number.is_finite()) {
            self.numeric
                .entry(field.to_owned())
                .or_default()
                .push((number, id));
        }
        if let Value::Object(object) = value {
            for (name, nested) in object {
                self.insert_value(&format!("{field}.{name}"), nested, id)?;
            }
        }
        Ok(())
    }

    fn evaluate(&self, filter: &VectorFilter) -> Result<RoaringBitmap, VectorError> {
        match filter {
            VectorFilter::Match { field, value } => self.exact(field, value),
            VectorFilter::Any { field, values } => {
                let mut result = RoaringBitmap::new();
                for value in values {
                    result |= self.exact(field, value)?;
                }
                Ok(result)
            }
            VectorFilter::Range { field, gte, lte } => {
                if gte.is_none() && lte.is_none() {
                    return Err(VectorError::InvalidRequest(format!(
                        "range filter for {field:?} has no bounds"
                    )));
                }
                if gte.is_some_and(|value| !value.is_finite())
                    || lte.is_some_and(|value| !value.is_finite())
                {
                    return Err(VectorError::InvalidRequest(format!(
                        "range filter for {field:?} has a non-finite bound"
                    )));
                }
                let mut result = RoaringBitmap::new();
                for &(value, id) in self.numeric.get(field).into_iter().flatten() {
                    if gte.is_none_or(|bound| value >= bound)
                        && lte.is_none_or(|bound| value <= bound)
                    {
                        result.insert(id);
                    }
                }
                Ok(result)
            }
            VectorFilter::And { clauses } => {
                let mut result = self.universe.clone();
                for clause in clauses {
                    result &= self.evaluate(clause)?;
                }
                Ok(result)
            }
            VectorFilter::Or { clauses } => {
                let mut result = RoaringBitmap::new();
                for clause in clauses {
                    result |= self.evaluate(clause)?;
                }
                Ok(result)
            }
            VectorFilter::Not { clause } => {
                let mut result = self.universe.clone();
                result -= self.evaluate(clause)?;
                Ok(result)
            }
        }
    }

    fn exact(&self, field: &str, value: &Value) -> Result<RoaringBitmap, VectorError> {
        let encoded = serde_json::to_string(value).map_err(|error| {
            VectorError::InvalidRequest(format!(
                "cannot encode filter value for {field:?}: {error}"
            ))
        })?;
        Ok(self
            .exact
            .get(&(field.to_owned(), encoded))
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use sbol_db_search_sdk::DocumentId;
    use serde_json::json;

    fn record(id: &str, graph: &str, year: u64) -> StoredRecord {
        StoredRecord {
            document_id: DocumentId(id.to_owned()),
            vector: vec![1.0, 0.0],
            payload: BTreeMap::from([
                ("graph".to_owned(), json!(graph)),
                ("metadata".to_owned(), json!({"year": year})),
            ]),
        }
    }

    #[test]
    fn compiles_nested_boolean_filters_to_ids() {
        let index = PayloadIndex::build(&[
            record("a", "public", 2024),
            record("b", "public", 2026),
            record("c", "private", 2026),
        ])
        .unwrap();
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
        assert_eq!(index.allowed(&filter).unwrap(), vec![1]);
    }
}
