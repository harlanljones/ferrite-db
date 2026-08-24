//! Write path — append-only inserts, Delta buffering, and auto-chunking.
//!
//! A [`WritePath`] validates an entire insert batch before publishing any of
//! it to the active Delta. Complete Delta chunks are sealed at the target
//! size; the active chunk remains immediately visible to exhaustive search.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    errors::{Error, Result},
    table::{ColumnType, Table},
};

/// Lower bound for a sealed Segment chunk.
pub const MIN_SEGMENT_ROWS: usize = 64 * 1024;
/// Upper bound for a sealed Segment chunk.
pub const MAX_SEGMENT_ROWS: usize = 128 * 1024;
/// Target size used when splitting an unbounded insert batch.
pub const TARGET_SEGMENT_ROWS: usize = 96 * 1024;

/// A scalar value supplied with one inserted vector.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

impl MetadataValue {
    pub(crate) fn column_type(&self) -> ColumnType {
        match self {
            Self::Bool(_) => ColumnType::Bool,
            Self::I64(_) => ColumnType::I64,
            Self::F64(_) => ColumnType::F64,
            Self::String(_) => ColumnType::String,
        }
    }
}

/// One append-only vector and its typed Metadata Schema values.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertRecord {
    id: u64,
    vector: Vec<f32>,
    metadata: BTreeMap<String, MetadataValue>,
    generation: u64,
}

impl InsertRecord {
    /// Creates an insert record.
    pub fn new(id: u64, vector: Vec<f32>, metadata: BTreeMap<String, MetadataValue>) -> Self {
        Self {
            id,
            vector,
            metadata,
            generation: 0,
        }
    }

    /// Returns the caller-supplied vector identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Returns the vector values.
    pub fn vector(&self) -> &[f32] {
        &self.vector
    }

    /// Returns the typed metadata values.
    pub fn metadata(&self) -> &BTreeMap<String, MetadataValue> {
        &self.metadata
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

/// One sealed, exhaustively searchable Delta Segment.
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaSegment {
    records: Vec<InsertRecord>,
}

impl DeltaSegment {
    fn new(records: Vec<InsertRecord>) -> Self {
        Self { records }
    }

    /// Number of vectors in this Delta Segment.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether this Delta Segment contains no vectors.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns records in insertion order for exhaustive search.
    pub fn records(&self) -> &[InsertRecord] {
        &self.records
    }
}

/// Recent inserts, including sealed chunks and the active Delta.
#[derive(Debug, Clone)]
pub struct Delta {
    table: Table,
    sealed: Vec<DeltaSegment>,
    active: Vec<InsertRecord>,
    tombstones: BTreeSet<(u64, u64)>,
    tombstoned_ids: BTreeSet<u64>,
    next_generation: u64,
}

impl Delta {
    /// Creates an empty Delta for one Table.
    pub fn new(table: Table) -> Self {
        Self {
            table,
            sealed: Vec::new(),
            active: Vec::new(),
            tombstones: BTreeSet::new(),
            tombstoned_ids: BTreeSet::new(),
            next_generation: 0,
        }
    }

    /// Inserts a batch after validating every record against the Table.
    ///
    /// The batch has no upper bound. Large batches are split into target-sized
    /// sealed Delta Segments, while a final partial chunk stays active and is
    /// immediately visible to exhaustive search.
    pub fn insert(&mut self, mut records: Vec<InsertRecord>) -> Result<()> {
        for record in &records {
            validate_record(&self.table, record)?;
        }
        for record in &mut records {
            record.generation = self.next_generation;
            self.next_generation = self.next_generation.wrapping_add(1);
        }
        self.active.extend(records);
        while self.active.len() >= TARGET_SEGMENT_ROWS {
            let remainder = self.active.split_off(TARGET_SEGMENT_ROWS);
            let sealed = std::mem::replace(&mut self.active, remainder);
            self.sealed.push(DeltaSegment::new(sealed));
        }
        Ok(())
    }

    /// Records Tombstones for known IDs. Unknown IDs succeed and are ignored.
    pub fn delete(&mut self, ids: &[u64]) -> Result<()> {
        for id in ids {
            let matches = self
                .records()
                .filter(|record| record.id() == *id)
                .map(|record| (record.id(), record.generation()))
                .collect::<Vec<_>>();
            if !matches.is_empty() {
                self.tombstoned_ids.insert(*id);
                self.tombstones.extend(matches);
            }
        }
        Ok(())
    }

    /// Replaces each ID using delete-plus-insert semantics.
    pub fn update(&mut self, records: Vec<InsertRecord>) -> Result<()> {
        for record in &records {
            validate_record(&self.table, record)?;
        }
        let ids = records.iter().map(InsertRecord::id).collect::<Vec<_>>();
        self.delete(&ids)?;
        self.insert(records)
    }

    /// Returns the Table fixed to this Delta.
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// Returns sealed Delta Segments.
    pub fn sealed_segments(&self) -> &[DeltaSegment] {
        &self.sealed
    }

    /// Returns the active, immediately searchable Delta records.
    pub fn active_records(&self) -> &[InsertRecord] {
        &self.active
    }

    /// Returns whether the ID is hidden by a Tombstone.
    pub fn is_tombstoned(&self, id: u64) -> bool {
        self.tombstoned_ids.contains(&id)
    }

    pub(crate) fn is_record_tombstoned(&self, record: &InsertRecord) -> bool {
        self.tombstones
            .contains(&(record.id(), record.generation()))
    }

    /// Returns the known Tombstoned IDs in ascending order.
    pub fn tombstoned_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.tombstoned_ids.iter().copied()
    }

    /// Returns all recent records in insertion order for exhaustive search.
    pub fn records(&self) -> impl Iterator<Item = &InsertRecord> {
        self.sealed
            .iter()
            .flat_map(|segment| segment.records())
            .chain(self.active.iter())
    }

    /// Number of recent vectors, including the active Delta.
    pub fn len(&self) -> usize {
        self.sealed.iter().map(DeltaSegment::len).sum::<usize>() + self.active.len()
    }

    /// Whether this Delta contains no recent vectors.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The write-path owner for one Table.
#[derive(Debug, Clone)]
pub struct WritePath {
    delta: Delta,
}

impl WritePath {
    /// Opens an empty write path bound to exactly one Table.
    pub fn new(table: Table) -> Self {
        Self {
            delta: Delta::new(table),
        }
    }

    /// Appends records to the Table's Delta.
    pub fn insert(&mut self, records: Vec<InsertRecord>) -> Result<()> {
        self.delta.insert(records)
    }

    /// Returns the Delta used by immediate exhaustive search.
    pub fn delta(&self) -> &Delta {
        &self.delta
    }
}

fn validate_record(table: &Table, record: &InsertRecord) -> Result<()> {
    let actual = u32::try_from(record.vector.len()).unwrap_or(u32::MAX);
    if actual != table.dimension() {
        return Err(Error::DimensionMismatch {
            expected: table.dimension(),
            actual,
        });
    }

    let schema = table.metadata_schema();
    if record.metadata.len() != schema.columns().len() {
        return Err(Error::SchemaViolation {
            reason: "insert metadata must contain each declared column exactly once".to_string(),
        });
    }
    for column in schema.columns() {
        let value = record
            .metadata
            .get(column.name())
            .ok_or_else(|| Error::SchemaViolation {
                reason: format!("missing metadata column: {}", column.name()),
            })?;
        if value.column_type() != column.column_type() {
            return Err(Error::SchemaViolation {
                reason: format!("metadata type mismatch for column: {}", column.name()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{MetadataColumn, MetadataSchema, Metric, TableManager};

    fn table() -> Table {
        let schema = MetadataSchema::new(vec![MetadataColumn::new(
            "active".to_string(),
            ColumnType::Bool,
        )])
        .unwrap();
        TableManager::new()
            .create("vectors".to_string(), 3, Metric::Cosine, schema)
            .unwrap()
    }

    fn record(id: u64) -> InsertRecord {
        InsertRecord::new(
            id,
            vec![id as f32, 1.0, 2.0],
            BTreeMap::from([(String::from("active"), MetadataValue::Bool(true))]),
        )
    }

    #[test]
    fn validates_dimension_and_schema_before_mutation() {
        let mut path = WritePath::new(table());
        let bad_dimension = InsertRecord::new(
            1,
            vec![1.0, 2.0],
            BTreeMap::from([(String::from("active"), MetadataValue::Bool(true))]),
        );
        assert!(matches!(
            path.insert(vec![bad_dimension]),
            Err(Error::DimensionMismatch { .. })
        ));
        assert!(path.delta().is_empty());

        let bad_schema = InsertRecord::new(2, vec![1.0, 2.0, 3.0], BTreeMap::new());
        assert!(matches!(
            path.insert(vec![bad_schema]),
            Err(Error::SchemaViolation { .. })
        ));
        assert!(path.delta().is_empty());
    }

    #[test]
    fn failed_record_keeps_the_whole_batch_unpublished() {
        let mut path = WritePath::new(table());
        let wrong_type = InsertRecord::new(
            2,
            vec![1.0, 2.0, 3.0],
            BTreeMap::from([(String::from("active"), MetadataValue::I64(1))]),
        );
        assert!(matches!(
            path.insert(vec![record(1), wrong_type]),
            Err(Error::SchemaViolation { .. })
        ));
        assert!(path.delta().is_empty());
    }

    #[test]
    fn inserts_are_immediately_visible_through_delta_records() {
        let mut path = WritePath::new(table());
        path.insert(vec![record(7), record(8)]).unwrap();
        let ids = path
            .delta()
            .records()
            .map(InsertRecord::id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![7, 8]);
    }

    #[test]
    fn delete_hides_known_records_and_ignores_unknown_ids() {
        let mut path = WritePath::new(table());
        path.insert(vec![record(7), record(8)]).unwrap();
        path.delta.delete(&[8, 99]).unwrap();
        assert!(!path.delta.is_tombstoned(7));
        assert!(path.delta.is_tombstoned(8));
        assert_eq!(path.delta.tombstoned_ids().collect::<Vec<_>>(), vec![8]);
    }

    #[test]
    fn update_replaces_old_record_and_keeps_new_record_visible() {
        let mut path = WritePath::new(table());
        path.insert(vec![record(7)]).unwrap();
        let replacement = InsertRecord::new(
            7,
            vec![70.0, 71.0, 72.0],
            BTreeMap::from([(String::from("active"), MetadataValue::Bool(false))]),
        );
        path.delta.update(vec![replacement]).unwrap();
        let old = path.delta.records().next().unwrap();
        let visible = path
            .delta
            .records()
            .filter(|entry| !path.delta.is_record_tombstoned(entry))
            .collect::<Vec<_>>();
        assert!(path.delta.is_record_tombstoned(old));
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].vector(), &[70.0, 71.0, 72.0]);
    }

    #[test]
    fn large_batches_have_bounded_sealed_segments_without_an_upper_batch_limit() {
        let mut path = WritePath::new(table());
        let records = (0..(TARGET_SEGMENT_ROWS * 2 + 17))
            .map(|id| record(id as u64))
            .collect();
        path.insert(records).unwrap();
        assert_eq!(path.delta().sealed_segments().len(), 2);
        assert!(
            path.delta()
                .sealed_segments()
                .iter()
                .all(|segment| (MIN_SEGMENT_ROWS..=MAX_SEGMENT_ROWS).contains(&segment.len()))
        );
        assert_eq!(path.delta().active_records().len(), 17);
        assert_eq!(path.delta().len(), TARGET_SEGMENT_ROWS * 2 + 17);
    }

    #[test]
    fn boundary_batch_sizes_are_split_deterministically() {
        let mut path = WritePath::new(table());
        path.insert((0..MIN_SEGMENT_ROWS).map(|id| record(id as u64)).collect())
            .unwrap();
        assert!(path.delta().sealed_segments().is_empty());
        assert_eq!(path.delta().active_records().len(), MIN_SEGMENT_ROWS);

        path.insert(
            (MIN_SEGMENT_ROWS..TARGET_SEGMENT_ROWS)
                .map(|id| record(id as u64))
                .collect(),
        )
        .unwrap();
        assert_eq!(path.delta().sealed_segments()[0].len(), TARGET_SEGMENT_ROWS);
    }
}
