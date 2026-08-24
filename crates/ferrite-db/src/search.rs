//! Exhaustive Delta search and typed Predicate Trees.
//!
//! Search is scoped to one [`Delta`], which carries exactly one Table handle.
//! Every candidate is predicate-checked and scored before deterministic
//! distance ordering and top-k assembly.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::{
    errors::{Error, Result},
    table::{ColumnType, MetadataSchema, Metric},
    write_path::{Delta, MetadataValue},
};

/// A typed predicate tree evaluated against one record's metadata.
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    Eq {
        column: String,
        value: MetadataValue,
    },
    NotEq {
        column: String,
        value: MetadataValue,
    },
    Lt {
        column: String,
        value: MetadataValue,
    },
    Lte {
        column: String,
        value: MetadataValue,
    },
    Gt {
        column: String,
        value: MetadataValue,
    },
    Gte {
        column: String,
        value: MetadataValue,
    },
    In {
        column: String,
        values: Vec<MetadataValue>,
    },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
}

impl Predicate {
    /// Creates an equality predicate.
    pub fn eq(column: String, value: MetadataValue) -> Self {
        Self::Eq { column, value }
    }

    /// Creates a not-equal predicate.
    pub fn not_eq(column: String, value: MetadataValue) -> Self {
        Self::NotEq { column, value }
    }

    /// Creates a less-than predicate.
    pub fn lt(column: String, value: MetadataValue) -> Self {
        Self::Lt { column, value }
    }

    /// Creates a less-than-or-equal predicate.
    pub fn lte(column: String, value: MetadataValue) -> Self {
        Self::Lte { column, value }
    }

    /// Creates a greater-than predicate.
    pub fn gt(column: String, value: MetadataValue) -> Self {
        Self::Gt { column, value }
    }

    /// Creates a greater-than-or-equal predicate.
    pub fn gte(column: String, value: MetadataValue) -> Self {
        Self::Gte { column, value }
    }

    /// Creates an `IN` predicate. An empty value list matches no records.
    pub fn in_values(column: String, values: Vec<MetadataValue>) -> Self {
        Self::In { column, values }
    }

    /// Creates a conjunction.
    pub fn and(predicates: Vec<Predicate>) -> Self {
        Self::And(predicates)
    }

    /// Creates a disjunction.
    pub fn or(predicates: Vec<Predicate>) -> Self {
        Self::Or(predicates)
    }

    /// Creates a negation.
    pub fn negate(predicate: Predicate) -> Self {
        Self::Not(Box::new(predicate))
    }

    fn validate(&self, schema: &MetadataSchema) -> Result<()> {
        match self {
            Self::Eq { column, value }
            | Self::NotEq { column, value }
            | Self::Lt { column, value }
            | Self::Lte { column, value }
            | Self::Gt { column, value }
            | Self::Gte { column, value } => {
                let declared = schema.column(column)?.column_type();
                validate_value(column, declared, value)?;
                if matches!(value, MetadataValue::String(_)) {
                    return Err(string_predicate_error(column));
                }
                if matches!(
                    self,
                    Self::Lt { .. } | Self::Lte { .. } | Self::Gt { .. } | Self::Gte { .. }
                ) && matches!(value, MetadataValue::Bool(_))
                {
                    return Err(Error::SchemaViolation {
                        reason: format!(
                            "ordering predicate is not supported for bool column: {column}"
                        ),
                    });
                }
                Ok(())
            }
            Self::In { column, values } => {
                let declared = schema.column(column)?.column_type();
                for value in values {
                    validate_value(column, declared, value)?;
                    if matches!(value, MetadataValue::String(_)) {
                        return Err(string_predicate_error(column));
                    }
                }
                Ok(())
            }
            Self::And(predicates) | Self::Or(predicates) => {
                for predicate in predicates {
                    predicate.validate(schema)?;
                }
                Ok(())
            }
            Self::Not(predicate) => predicate.validate(schema),
        }
    }

    fn matches(&self, metadata: &BTreeMap<String, MetadataValue>) -> bool {
        match self {
            Self::Eq { column, value } => {
                metadata.get(column).is_some_and(|actual| actual == value)
            }
            Self::NotEq { column, value } => {
                metadata.get(column).is_some_and(|actual| actual != value)
            }
            Self::Lt { column, value } => metadata
                .get(column)
                .and_then(|actual| compare_values(actual, value))
                .is_some_and(Ordering::is_lt),
            Self::Lte { column, value } => metadata
                .get(column)
                .and_then(|actual| compare_values(actual, value))
                .is_some_and(|ordering| ordering.is_le()),
            Self::Gt { column, value } => metadata
                .get(column)
                .and_then(|actual| compare_values(actual, value))
                .is_some_and(Ordering::is_gt),
            Self::Gte { column, value } => metadata
                .get(column)
                .and_then(|actual| compare_values(actual, value))
                .is_some_and(|ordering| ordering.is_ge()),
            Self::In { column, values } => metadata
                .get(column)
                .is_some_and(|actual| values.iter().any(|value| actual == value)),
            Self::And(predicates) => predicates
                .iter()
                .all(|predicate| predicate.matches(metadata)),
            Self::Or(predicates) => predicates
                .iter()
                .any(|predicate| predicate.matches(metadata)),
            Self::Not(predicate) => !predicate.matches(metadata),
        }
    }
}

/// Search options for exhaustive scan search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    top_k: u32,
    probes: Option<u32>,
    ef_search: Option<u32>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            top_k: 10,
            probes: None,
            ef_search: None,
        }
    }
}

impl SearchOptions {
    /// Creates options with the default top-k of 10.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets top-k, which must be between 1 and 1,000 inclusive.
    pub fn with_top_k(mut self, top_k: u32) -> Result<Self> {
        if !(1..=1_000).contains(&top_k) {
            return Err(Error::SchemaViolation {
                reason: "top_k must be between 1 and 1,000".to_string(),
            });
        }
        self.top_k = top_k;
        Ok(self)
    }

    /// Sets the future ANN probe override.
    pub fn with_probes(mut self, probes: Option<u32>) -> Self {
        self.probes = probes;
        self
    }

    /// Sets the future HNSW search override.
    pub fn with_ef_search(mut self, ef_search: Option<u32>) -> Self {
        self.ef_search = ef_search;
        self
    }

    /// Returns top-k.
    pub fn top_k(&self) -> u32 {
        self.top_k
    }

    /// Returns the optional Probe override.
    pub fn probes(&self) -> Option<u32> {
        self.probes
    }

    /// Returns the optional ef_search override.
    pub fn ef_search(&self) -> Option<u32> {
        self.ef_search
    }
}

/// One ranked search result, including retrievable metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    id: u64,
    distance: f32,
    vector: Vec<f32>,
    metadata: BTreeMap<String, MetadataValue>,
}

impl SearchResult {
    /// Returns the vector identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns the metric distance; lower values rank first.
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// Returns the matched vector.
    pub fn vector(&self) -> &[f32] {
        &self.vector
    }

    /// Returns retrievable metadata, including string columns.
    pub fn metadata(&self) -> &BTreeMap<String, MetadataValue> {
        &self.metadata
    }
}

/// Exhaustively searches every recent record in one Delta.
pub fn search(
    delta: &Delta,
    query: &[f32],
    predicate: Option<&Predicate>,
    options: SearchOptions,
) -> Result<Vec<SearchResult>> {
    let actual = u32::try_from(query.len()).unwrap_or(u32::MAX);
    if actual != delta.table().dimension() {
        return Err(Error::DimensionMismatch {
            expected: delta.table().dimension(),
            actual,
        });
    }
    if let Some(predicate) = predicate {
        predicate.validate(delta.table().metadata_schema())?;
    }

    let mut results = delta
        .records()
        .filter(|record| {
            !delta.is_record_tombstoned(record)
                && predicate.is_none_or(|predicate| predicate.matches(record.metadata()))
        })
        .map(|record| SearchResult {
            id: record.id(),
            distance: distance(delta.table().metric(), query, record.vector()),
            vector: record.vector().to_vec(),
            metadata: record.metadata().clone(),
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        left.distance
            .total_cmp(&right.distance)
            .then_with(|| left.id.cmp(&right.id))
    });
    results.truncate(options.top_k as usize);
    Ok(results)
}

fn validate_value(column: &str, declared: ColumnType, value: &MetadataValue) -> Result<()> {
    if value.column_type() != declared {
        return Err(Error::SchemaViolation {
            reason: format!("predicate type mismatch for column: {column}"),
        });
    }
    Ok(())
}

fn string_predicate_error(column: &str) -> Error {
    Error::SchemaViolation {
        reason: format!("string column is retrievable but not filterable: {column}"),
    }
}

fn compare_values(left: &MetadataValue, right: &MetadataValue) -> Option<Ordering> {
    match (left, right) {
        (MetadataValue::Bool(left), MetadataValue::Bool(right)) => Some(left.cmp(right)),
        (MetadataValue::I64(left), MetadataValue::I64(right)) => Some(left.cmp(right)),
        (MetadataValue::F64(left), MetadataValue::F64(right)) => left.partial_cmp(right),
        (MetadataValue::String(_), MetadataValue::String(_)) => None,
        _ => None,
    }
}

fn distance(metric: Metric, query: &[f32], vector: &[f32]) -> f32 {
    match metric {
        Metric::Cosine => {
            let query_norm = query.iter().map(|value| value * value).sum::<f32>().sqrt();
            let vector_norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            if query_norm == 0.0 || vector_norm == 0.0 {
                1.0
            } else {
                let dot = query
                    .iter()
                    .zip(vector)
                    .map(|(left, right)| left * right)
                    .sum::<f32>();
                1.0 - dot / (query_norm * vector_norm)
            }
        }
        Metric::L2 => query
            .iter()
            .zip(vector)
            .map(|(left, right)| (left - right) * (left - right))
            .sum(),
        Metric::Dot => -query
            .iter()
            .zip(vector)
            .map(|(left, right)| left * right)
            .sum::<f32>(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        table::{MetadataColumn, MetadataSchema, TableManager},
        write_path::{InsertRecord, MetadataValue, WritePath},
    };

    fn path(metric: Metric) -> WritePath {
        let schema = MetadataSchema::new(vec![
            MetadataColumn::new("active".to_string(), ColumnType::Bool),
            MetadataColumn::new("rank".to_string(), ColumnType::I64),
            MetadataColumn::new("label".to_string(), ColumnType::String),
        ])
        .unwrap();
        let table = TableManager::new()
            .create("vectors".to_string(), 3, metric, schema)
            .unwrap();
        let mut path = WritePath::new(table);
        for id in 0..6 {
            path.insert(vec![InsertRecord::new(
                id,
                vec![id as f32, 0.0, 0.0],
                BTreeMap::from([
                    (String::from("active"), MetadataValue::Bool(id % 2 == 0)),
                    (String::from("rank"), MetadataValue::I64(id as i64)),
                    (
                        String::from("label"),
                        MetadataValue::String(format!("v{id}")),
                    ),
                ]),
            )])
            .unwrap();
        }
        path
    }

    #[test]
    fn searches_ranked_results_for_all_metrics() {
        for metric in [Metric::Cosine, Metric::L2, Metric::Dot] {
            let path = path(metric);
            let results = search(
                path.delta(),
                &[5.0, 0.0, 0.0],
                None,
                SearchOptions::new().with_top_k(3).unwrap(),
            )
            .unwrap();
            assert_eq!(results.len(), 3);
            assert!(
                results
                    .windows(2)
                    .all(|pair| pair[0].distance() <= pair[1].distance())
            );
        }
    }

    #[test]
    fn predicate_tree_supports_comparison_membership_and_logic() {
        let path = path(Metric::L2);
        let predicate = Predicate::and(vec![
            Predicate::gte("rank".to_string(), MetadataValue::I64(2)),
            Predicate::or(vec![
                Predicate::eq("active".to_string(), MetadataValue::Bool(true)),
                Predicate::in_values(
                    "rank".to_string(),
                    vec![MetadataValue::I64(3), MetadataValue::I64(5)],
                ),
            ]),
            Predicate::negate(Predicate::eq("rank".to_string(), MetadataValue::I64(4))),
        ]);
        let results = search(
            path.delta(),
            &[0.0, 0.0, 0.0],
            Some(&predicate),
            SearchOptions::default(),
        )
        .unwrap();
        assert_eq!(
            results.iter().map(SearchResult::id).collect::<Vec<_>>(),
            vec![2, 3, 5]
        );
    }

    #[test]
    fn string_columns_are_returned_but_not_filterable() {
        let path = path(Metric::L2);
        let results = search(
            path.delta(),
            &[5.0, 0.0, 0.0],
            None,
            SearchOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            results[0].metadata().get("label"),
            Some(MetadataValue::String(_))
        ));

        let predicate = Predicate::eq("label".to_string(), MetadataValue::String("v1".to_string()));
        assert!(matches!(
            search(
                path.delta(),
                &[5.0, 0.0, 0.0],
                Some(&predicate),
                SearchOptions::default(),
            ),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn rejects_unknown_and_mismatched_predicates() {
        let path = path(Metric::L2);
        let unknown = Predicate::eq("missing".to_string(), MetadataValue::I64(1));
        assert!(matches!(
            search(
                path.delta(),
                &[5.0, 0.0, 0.0],
                Some(&unknown),
                SearchOptions::default(),
            ),
            Err(Error::SchemaViolation { .. })
        ));

        let mismatch = Predicate::eq("rank".to_string(), MetadataValue::Bool(true));
        assert!(matches!(
            search(
                path.delta(),
                &[5.0, 0.0, 0.0],
                Some(&mismatch),
                SearchOptions::default(),
            ),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn top_k_is_bounded_and_deterministic_for_ties() {
        let path = path(Metric::L2);
        let options = SearchOptions::default().with_top_k(2).unwrap();
        let results = search(path.delta(), &[0.0, 0.0, 0.0], None, options).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results.iter().map(SearchResult::id).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(SearchOptions::default().with_top_k(0).is_err());
        assert!(SearchOptions::default().with_top_k(1_001).is_err());
    }
}
