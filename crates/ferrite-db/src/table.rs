//! Table lifecycle and Metadata Schema declarations.
//!
//! A [`TableManager`] owns named [`Table`] handles. A Table fixes its vector
//! dimension and [`Metric`] at creation; later work can therefore accept a
//! single Table handle without making cross-Table queries representable.

use std::collections::BTreeMap;

use crate::errors::{Error, Result};

/// The distance function fixed for a Table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    /// Cosine distance over normalized vectors.
    Cosine,
    /// Squared Euclidean distance.
    L2,
    /// Dot-product distance.
    Dot,
}

/// The scalar types supported by a Metadata Schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Bool,
    I64,
    F64,
    String,
}

/// One named column in a Metadata Schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataColumn {
    name: String,
    column_type: ColumnType,
}

impl MetadataColumn {
    /// Creates a column declaration. Name validation happens when the
    /// declaration is added to a [`MetadataSchema`].
    pub fn new(name: String, column_type: ColumnType) -> Self {
        Self { name, column_type }
    }

    /// Returns the declared column name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the declared scalar type.
    pub fn column_type(&self) -> ColumnType {
        self.column_type
    }
}

/// The typed scalar columns declared for a Table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataSchema {
    columns: Vec<MetadataColumn>,
}

impl MetadataSchema {
    /// Creates and validates a Metadata Schema.
    pub fn new(columns: Vec<MetadataColumn>) -> Result<Self> {
        let mut names = BTreeMap::new();
        for column in &columns {
            if column.name.trim().is_empty() || column.name.contains('\0') {
                return Err(Error::SchemaViolation {
                    reason: "column names must be non-empty and contain no NUL bytes".to_string(),
                });
            }
            if names.insert(column.name.clone(), ()).is_some() {
                return Err(Error::SchemaViolation {
                    reason: format!("duplicate column: {}", column.name),
                });
            }
        }
        Ok(Self { columns })
    }

    /// Returns columns in declaration order.
    pub fn columns(&self) -> &[MetadataColumn] {
        &self.columns
    }

    /// Looks up a declared column by name.
    pub fn column(&self, name: &str) -> Result<&MetadataColumn> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| Error::SchemaViolation {
                reason: format!("unknown column: {name}"),
            })
    }
}

/// An immutable Table definition and handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    name: String,
    dimension: u32,
    metric: Metric,
    metadata_schema: MetadataSchema,
}

impl Table {
    /// Returns the Table's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the fixed vector dimension.
    pub fn dimension(&self) -> u32 {
        self.dimension
    }

    /// Returns the fixed distance function.
    pub fn metric(&self) -> Metric {
        self.metric
    }

    /// Returns the Table's Metadata Schema.
    pub fn metadata_schema(&self) -> &MetadataSchema {
        &self.metadata_schema
    }
}

/// In-process owner of named Tables.
#[derive(Debug, Default)]
pub struct TableManager {
    tables: BTreeMap<String, Table>,
}

impl TableManager {
    /// Creates an empty Table manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates and registers a Table with immutable dimension and Metric.
    pub fn create(
        &mut self,
        name: String,
        dimension: u32,
        metric: Metric,
        metadata_schema: MetadataSchema,
    ) -> Result<Table> {
        validate_table_name(&name)?;
        if dimension == 0 {
            return Err(Error::SchemaViolation {
                reason: "table dimension must be greater than zero".to_string(),
            });
        }
        if self.tables.contains_key(&name) {
            return Err(Error::SchemaViolation {
                reason: format!("duplicate table: {name}"),
            });
        }

        let table = Table {
            name: name.clone(),
            dimension,
            metric,
            metadata_schema,
        };
        self.tables.insert(name, table.clone());
        Ok(table)
    }

    /// Opens a registered Table by name.
    pub fn open(&self, name: &str) -> Result<Table> {
        self.tables
            .get(name)
            .cloned()
            .ok_or_else(|| Error::TableNotFound {
                name: name.to_string(),
            })
    }

    /// Drops a registered Table by name.
    pub fn drop(&mut self, name: &str) -> Result<()> {
        if self.tables.remove(name).is_none() {
            return Err(Error::TableNotFound {
                name: name.to_string(),
            });
        }
        Ok(())
    }
}

fn validate_table_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.contains('\0') {
        return Err(Error::SchemaViolation {
            reason: "table names must be non-empty and contain no NUL bytes".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> MetadataSchema {
        MetadataSchema::new(vec![MetadataColumn::new(
            "active".to_string(),
            ColumnType::Bool,
        )])
        .unwrap()
    }

    #[test]
    fn accepts_all_declared_scalar_types() {
        let schema = MetadataSchema::new(vec![
            MetadataColumn::new("active".to_string(), ColumnType::Bool),
            MetadataColumn::new("count".to_string(), ColumnType::I64),
            MetadataColumn::new("ratio".to_string(), ColumnType::F64),
            MetadataColumn::new("label".to_string(), ColumnType::String),
        ])
        .unwrap();
        assert_eq!(schema.columns().len(), 4);
        assert_eq!(
            schema.column("label").unwrap().column_type(),
            ColumnType::String
        );
    }

    #[test]
    fn rejects_duplicate_and_malformed_declarations() {
        let duplicate = MetadataSchema::new(vec![
            MetadataColumn::new("id".to_string(), ColumnType::I64),
            MetadataColumn::new("id".to_string(), ColumnType::String),
        ])
        .unwrap_err();
        assert!(matches!(duplicate, Error::SchemaViolation { .. }));

        let empty = MetadataSchema::new(vec![MetadataColumn::new(
            "  ".to_string(),
            ColumnType::Bool,
        )])
        .unwrap_err();
        assert!(matches!(empty, Error::SchemaViolation { .. }));
    }

    #[test]
    fn validates_table_name_and_dimension() {
        let mut manager = TableManager::new();
        assert!(matches!(
            manager.create("".to_string(), 3, Metric::Cosine, schema()),
            Err(Error::SchemaViolation { .. })
        ));
        assert!(matches!(
            manager.create("vectors".to_string(), 0, Metric::L2, schema()),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn duplicate_and_unknown_tables_return_taxonomy_errors() {
        let mut manager = TableManager::new();
        let table = manager
            .create("vectors".to_string(), 3, Metric::Dot, schema())
            .unwrap();
        assert_eq!(table.dimension(), 3);
        assert_eq!(table.metric(), Metric::Dot);

        assert!(matches!(
            manager.create("vectors".to_string(), 3, Metric::Dot, schema()),
            Err(Error::SchemaViolation { .. })
        ));
        assert!(matches!(
            manager.open("missing"),
            Err(Error::TableNotFound { .. })
        ));
        assert!(matches!(
            manager.drop("missing"),
            Err(Error::TableNotFound { .. })
        ));
    }

    #[test]
    fn accepts_each_metric_at_creation() {
        let mut manager = TableManager::new();
        for (name, metric) in [
            ("cosine", Metric::Cosine),
            ("l2", Metric::L2),
            ("dot", Metric::Dot),
        ] {
            assert_eq!(
                manager
                    .create(name.to_string(), 3, metric, MetadataSchema::default())
                    .unwrap()
                    .metric(),
                metric
            );
        }
    }

    #[test]
    fn open_and_drop_manage_the_same_table_name() {
        let mut manager = TableManager::new();
        manager
            .create(
                "vectors".to_string(),
                3,
                Metric::Cosine,
                MetadataSchema::default(),
            )
            .unwrap();
        let opened = manager.open("vectors").unwrap();
        assert_eq!(opened.name(), "vectors");
        manager.drop("vectors").unwrap();
        assert!(matches!(
            manager.open("vectors"),
            Err(Error::TableNotFound { .. })
        ));
    }
}
