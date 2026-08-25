//! WASM bindings for Ferrite DB's threadless, in-memory vector engine.
//!
//! This crate wraps the `wasm` feature of `ferrite-db`, which keeps only the
//! in-memory core (Table management, the write path, and exhaustive search)
//! and excludes the LanceDB/tokio/Rayon substrate, on-disk Segment storage,
//! and background-thread Lifecycle/Concurrency concerns (FDB-EXP-01).
//!
//! The exposed JS/TS surface is deliberately small and synchronous:
//! `create_table` / `create_table_schema`, `insert_records` /
//! `insert_with_metadata`, `search`, `exact_search`, `list_tables`, and
//! `status`. `search` runs the same exhaustive scan as the native engine;
//! `exact_search` is an independent brute-force oracle used for recall
//! validation. `create_table_schema` + `insert_with_metadata` carry typed
//! Metadata Schemas (FDB-EXP-03).

use std::cell::RefCell;
use std::collections::BTreeMap;

use wasm_bindgen::prelude::*;

use serde_json::Value as Json;

use ferrite_db::errors::{Error, Result as FerriteResult};
use ferrite_db::search::{Predicate, SearchOptions, SearchResult, distance, search};
use ferrite_db::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};
use ferrite_db::write_path::{InsertRecord, MetadataValue, WritePath};

/// One search result handed back to JS.
#[wasm_bindgen]
#[derive(Clone)]
pub struct SearchHit {
    /// The matched vector identifier.
    pub id: u64,
    /// Distance of the match under the Table's Metric (lower is nearer).
    pub distance: f32,
    /// The record's Metadata Schema values serialized as a JSON object.
    metadata_json: String,
}

#[wasm_bindgen]
impl SearchHit {
    /// The record's metadata as a JSON object string.
    #[wasm_bindgen(getter)]
    pub fn metadata_json(&self) -> String {
        self.metadata_json.clone()
    }
}

/// Summary of one Table's in-memory state.
#[wasm_bindgen]
#[derive(Clone)]
pub struct TableStatus {
    name: String,
    dimension: u32,
    metric: String,
    vectors: u32,
}

#[wasm_bindgen]
impl TableStatus {
    /// Table name.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.name.clone()
    }
    /// Fixed vector dimension.
    #[wasm_bindgen(getter)]
    pub fn dimension(&self) -> u32 {
        self.dimension
    }
    /// Fixed distance function (`cosine`, `l2`, or `dot`).
    #[wasm_bindgen(getter)]
    pub fn metric(&self) -> String {
        self.metric.clone()
    }
    /// Number of vectors currently held in memory.
    #[wasm_bindgen(getter)]
    pub fn vectors(&self) -> u32 {
        self.vectors
    }
}

/// Summary of the whole session.
#[wasm_bindgen]
#[derive(Clone)]
pub struct SessionStatus {
    /// Number of Tables created in this session.
    pub table_count: u32,
    /// Total vectors held across every Table.
    pub vector_count: u32,
}

/// Per-phase timing breakdown of one exhaustive query (FDB-EXP-08).
#[wasm_bindgen]
pub struct SearchProfile {
    total_rows: u32,
    scanned_rows: u32,
    matched_rows: u32,
    returned_rows: u32,
    filter_us: u64,
    scan_us: u64,
    rank_us: u64,
}

#[wasm_bindgen]
impl SearchProfile {
    #[wasm_bindgen(getter)]
    pub fn total_rows(&self) -> u32 {
        self.total_rows
    }
    #[wasm_bindgen(getter)]
    pub fn scanned_rows(&self) -> u32 {
        self.scanned_rows
    }
    #[wasm_bindgen(getter)]
    pub fn matched_rows(&self) -> u32 {
        self.matched_rows
    }
    #[wasm_bindgen(getter)]
    pub fn returned_rows(&self) -> u32 {
        self.returned_rows
    }
    /// Predicate-filter + Tombstone-visibility pass, microseconds.
    #[wasm_bindgen(getter)]
    pub fn filter_us(&self) -> u64 {
        self.filter_us
    }
    /// Metric distance pass over surviving rows, microseconds.
    #[wasm_bindgen(getter)]
    pub fn scan_us(&self) -> u64 {
        self.scan_us
    }
    /// Deterministic ranking + truncation pass, microseconds.
    #[wasm_bindgen(getter)]
    pub fn rank_us(&self) -> u64 {
        self.rank_us
    }
}

/// Snapshot of one Table's Delta layout for the lifecycle inspector.
#[wasm_bindgen]
pub struct LifecycleExport {
    sealed_counts: Vec<u32>,
    sealed_dead: Vec<u32>,
    active_total: u32,
    active_dead: u32,
    tombstoned_ids: u32,
    total_rows: u32,
}

#[wasm_bindgen]
impl LifecycleExport {
    /// Row count of each sealed Segment, in seal order.
    #[wasm_bindgen(getter)]
    pub fn sealed_counts(&self) -> Vec<u32> {
        self.sealed_counts.clone()
    }
    /// Tombstoned row count within each sealed Segment.
    #[wasm_bindgen(getter)]
    pub fn sealed_dead(&self) -> Vec<u32> {
        self.sealed_dead.clone()
    }
    /// Rows in the active Delta buffer.
    #[wasm_bindgen(getter)]
    pub fn active_total(&self) -> u32 {
        self.active_total
    }
    /// Tombstoned rows in the active Delta buffer.
    #[wasm_bindgen(getter)]
    pub fn active_dead(&self) -> u32 {
        self.active_dead
    }
    /// Number of distinct Tombstoned ids.
    #[wasm_bindgen(getter)]
    pub fn tombstoned_ids(&self) -> u32 {
        self.tombstoned_ids
    }
    /// Total recent vectors (sealed + active).
    #[wasm_bindgen(getter)]
    pub fn total_rows(&self) -> u32 {
        self.total_rows
    }
}

/// Snapshot of one Table's vectors for the projection visualizer.
#[wasm_bindgen]
pub struct VectorExport {
    ids: js_sys::BigUint64Array,
    vectors: js_sys::Float32Array,
    metadata_json: Vec<String>,
}

#[wasm_bindgen]
impl VectorExport {
    /// Record ids, aligned with `vectors` rows and `metadata_json`.
    #[wasm_bindgen(getter)]
    pub fn ids(&self) -> js_sys::BigUint64Array {
        self.ids.clone()
    }
    /// Flat row-major f32 vector data (`ids.len() * dimension` values).
    #[wasm_bindgen(getter)]
    pub fn vectors(&self) -> js_sys::Float32Array {
        self.vectors.clone()
    }
    /// Per-record metadata serialized as JSON object strings.
    #[wasm_bindgen(getter)]
    pub fn metadata_json(&self) -> Vec<String> {
        self.metadata_json.clone()
    }
}

struct Inner {
    manager: TableManager,
    tables: BTreeMap<String, WritePath>,
    /// Names of every Table created through this session, in creation order.
    /// Used by `list_tables` for UI table switching (FDB-EXP-03).
    known_tables: Vec<String>,
}

fn to_js<E: std::fmt::Display>(error: E) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn parse_metric(value: &str) -> FerriteResult<Metric> {
    match value.to_ascii_lowercase().as_str() {
        "cosine" => Ok(Metric::Cosine),
        "l2" => Ok(Metric::L2),
        "dot" => Ok(Metric::Dot),
        other => Err(Error::SchemaViolation {
            reason: format!("unknown metric '{other}'; expected cosine, l2, or dot"),
        }),
    }
}

fn metric_name(metric: Metric) -> &'static str {
    match metric {
        Metric::Cosine => "cosine",
        Metric::L2 => "l2",
        Metric::Dot => "dot",
    }
}

fn parse_column_type(value: &str) -> FerriteResult<ColumnType> {
    match value.to_ascii_lowercase().as_str() {
        "bool" => Ok(ColumnType::Bool),
        "i64" => Ok(ColumnType::I64),
        "f64" => Ok(ColumnType::F64),
        "string" => Ok(ColumnType::String),
        other => Err(Error::SchemaViolation {
            reason: format!("unknown column type '{other}'; expected bool, i64, f64, or string"),
        }),
    }
}

/// Parses a single metadata value literal against its declared column type.
/// Booleans accept `true`/`false` (case-insensitive); numbers use Rust's
/// native parsers; strings are taken verbatim.
fn parse_metadata_value(column_type: &ColumnType, raw: &str) -> FerriteResult<MetadataValue> {
    let trimmed = raw.trim();
    match column_type {
        ColumnType::Bool => match trimmed.to_ascii_lowercase().as_str() {
            "true" => Ok(MetadataValue::Bool(true)),
            "false" => Ok(MetadataValue::Bool(false)),
            other => Err(Error::SchemaViolation {
                reason: format!("metadata bool column expects true/false, got '{other}'"),
            }),
        },
        ColumnType::I64 => {
            trimmed
                .parse::<i64>()
                .map(MetadataValue::I64)
                .map_err(|_| Error::SchemaViolation {
                    reason: format!("metadata i64 column expects an integer, got '{trimmed}'"),
                })
        }
        ColumnType::F64 => {
            trimmed
                .parse::<f64>()
                .map(MetadataValue::F64)
                .map_err(|_| Error::SchemaViolation {
                    reason: format!("metadata f64 column expects a number, got '{trimmed}'"),
                })
        }
        ColumnType::String => Ok(MetadataValue::String(trimmed.to_string())),
    }
}

/// Parses the JSON representation of a Predicate Tree used by the query
/// runner (FDB-EXP-04):
///
/// - leaf: `{"op":"eq|ne|lt|lte|gt|gte","column":s,"value":scalar}`
/// - set:  `{"op":"in","column":s,"values":[scalar,...]}`
/// - combinator: `{"op":"and"|"or","children":[...]}` / `{"op":"not","child":{...}}`
///
/// Leaf scalars are typed against the Table's declared Metadata Schema so the
/// resulting `Predicate` passes the engine's own validation.
fn parse_predicate_json(schema: &MetadataSchema, node: &Json) -> FerriteResult<Predicate> {
    let obj = node.as_object().ok_or_else(|| Error::SchemaViolation {
        reason: "predicate node must be a JSON object".to_string(),
    })?;
    let op = obj
        .get("op")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::SchemaViolation {
            reason: "predicate node missing 'op'".to_string(),
        })?;
    match op {
        "and" | "or" => {
            let children = obj
                .get("children")
                .and_then(Json::as_array)
                .ok_or_else(|| Error::SchemaViolation {
                    reason: format!("'{op}' predicate requires 'children'"),
                })?;
            let parsed: FerriteResult<Vec<Predicate>> = children
                .iter()
                .map(|child| parse_predicate_json(schema, child))
                .collect();
            if op == "and" {
                Ok(Predicate::and(parsed?))
            } else {
                Ok(Predicate::or(parsed?))
            }
        }
        "not" => {
            let child = obj.get("child").ok_or_else(|| Error::SchemaViolation {
                reason: "'not' predicate requires 'child'".to_string(),
            })?;
            Ok(Predicate::negate(parse_predicate_json(schema, child)?))
        }
        "eq" | "ne" | "lt" | "lte" | "gt" | "gte" => {
            let column = string_field(obj, "column")?;
            let value = typed_scalar(schema, &column, obj.get("value").unwrap_or(&Json::Null))?;
            Ok(match op {
                "eq" => Predicate::eq(column, value),
                "ne" => Predicate::not_eq(column, value),
                "lt" => Predicate::lt(column, value),
                "lte" => Predicate::lte(column, value),
                "gt" => Predicate::gt(column, value),
                _ => Predicate::gte(column, value),
            })
        }
        "in" => {
            let column = string_field(obj, "column")?;
            let raw_values = obj.get("values").and_then(Json::as_array).ok_or_else(|| {
                Error::SchemaViolation {
                    reason: "'in' predicate requires 'values'".to_string(),
                }
            })?;
            let values: FerriteResult<Vec<MetadataValue>> = raw_values
                .iter()
                .map(|raw| typed_scalar(schema, &column, raw))
                .collect();
            Ok(Predicate::in_values(column, values?))
        }
        other => Err(Error::SchemaViolation {
            reason: format!("unknown predicate op '{other}'"),
        }),
    }
}

/// Parses an optional Predicate Tree JSON payload; `None` passes through.
fn parse_optional_predicate(
    schema: &MetadataSchema,
    predicate_json: Option<String>,
) -> FerriteResult<Option<Predicate>> {
    match predicate_json {
        None => Ok(None),
        Some(json) => {
            let node: Json =
                serde_json::from_str(&json).map_err(|error| Error::SchemaViolation {
                    reason: format!("invalid predicate JSON: {error}"),
                })?;
            parse_predicate_json(schema, &node).map(Some)
        }
    }
}

fn string_field(obj: &serde_json::Map<String, Json>, key: &str) -> FerriteResult<String> {
    obj.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::SchemaViolation {
            reason: format!("predicate missing '{key}'"),
        })
}

/// Types a JSON scalar against the declared column type. String columns take
/// any scalar verbatim; numeric/bool columns must carry a matching JSON type.
fn typed_scalar(schema: &MetadataSchema, column: &str, raw: &Json) -> FerriteResult<MetadataValue> {
    let column_type = schema.column(column)?.column_type();
    match (column_type, raw) {
        (ColumnType::Bool, Json::Bool(value)) => Ok(MetadataValue::Bool(*value)),
        (ColumnType::I64, Json::Number(number)) => number
            .as_i64()
            .map(MetadataValue::I64)
            .ok_or_else(|| Error::SchemaViolation {
                reason: format!("column '{column}' expects an i64 literal"),
            }),
        (ColumnType::F64, Json::Number(number)) => number
            .as_f64()
            .map(MetadataValue::F64)
            .ok_or_else(|| Error::SchemaViolation {
                reason: format!("column '{column}' expects an f64 literal"),
            }),
        (ColumnType::String, Json::String(value)) => Ok(MetadataValue::String(value.clone())),
        (declared, other) => Err(Error::SchemaViolation {
            reason: format!(
                "value {other} does not match declared type {declared:?} of column '{column}'"
            ),
        }),
    }
}

/// Serializes retrievable metadata as a compact JSON object for JS display.
fn metadata_to_json(metadata: &BTreeMap<String, MetadataValue>) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in metadata.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{}:",
            serde_json::to_string(key).unwrap_or_default()
        ));
        match value {
            MetadataValue::Bool(inner) => out.push_str(if *inner { "true" } else { "false" }),
            MetadataValue::I64(inner) => out.push_str(&inner.to_string()),
            MetadataValue::F64(inner) => out.push_str(&inner.to_string()),
            MetadataValue::String(inner) => {
                out.push_str(&serde_json::to_string(inner).unwrap_or_default())
            }
        }
    }
    out.push('}');
    out
}

fn hit_from_result(result: &SearchResult) -> SearchHit {
    SearchHit {
        id: result.id(),
        distance: result.distance(),
        metadata_json: metadata_to_json(result.metadata()),
    }
}

/// The WASM-facing Ferrite DB session. Holds every Table created in this
/// instance; all operations are synchronous and run on the calling thread.
#[wasm_bindgen]
pub struct FerriteDb {
    inner: RefCell<Inner>,
}

#[wasm_bindgen]
impl FerriteDb {
    /// Creates an empty session.
    #[wasm_bindgen(constructor)]
    pub fn new() -> FerriteDb {
        FerriteDb::default()
    }

    /// Creates a Table with the given name, vector `dimension`, and `metric`
    /// (`"cosine"`, `"l2"`, or `"dot"`). Tables carry no metadata columns in
    /// this reduced core, so inserted records need no metadata.
    pub fn create_table(
        &self,
        name: &str,
        dimension: u32,
        metric: &str,
    ) -> std::result::Result<(), JsValue> {
        let mut inner = self.inner.borrow_mut();
        let parsed = parse_metric(metric).map_err(to_js)?;
        let schema = MetadataSchema::new(Vec::<MetadataColumn>::new()).map_err(to_js)?;
        let table = inner
            .manager
            .create(name.to_string(), dimension, parsed, schema)
            .map_err(to_js)?;
        inner.tables.insert(name.to_string(), WritePath::new(table));
        inner.known_tables.push(name.to_string());
        Ok(())
    }

    /// Creates a Table with a typed Metadata Schema, enabling custom dataset
    /// ingestion (FDB-EXP-03). `col_names` and `col_types` must be equal
    /// length; `col_types` are `"bool"`, `"i64"`, `"f64"`, or `"string"`.
    pub fn create_table_schema(
        &self,
        name: &str,
        dimension: u32,
        metric: &str,
        col_names: Vec<String>,
        col_types: Vec<String>,
    ) -> std::result::Result<(), JsValue> {
        let mut inner = self.inner.borrow_mut();
        let parsed = parse_metric(metric).map_err(to_js)?;
        let mut columns = Vec::with_capacity(col_names.len());
        for (col_name, col_type) in col_names.iter().zip(col_types.iter()) {
            let column_type = parse_column_type(col_type).map_err(to_js)?;
            columns.push(MetadataColumn::new(col_name.clone(), column_type));
        }
        let schema = MetadataSchema::new(columns).map_err(to_js)?;
        let table = inner
            .manager
            .create(name.to_string(), dimension, parsed, schema)
            .map_err(to_js)?;
        inner.tables.insert(name.to_string(), WritePath::new(table));
        inner.known_tables.push(name.to_string());
        Ok(())
    }

    /// Names of every Table created in this session, in creation order.
    pub fn list_tables(&self) -> Vec<String> {
        self.inner.borrow().known_tables.clone()
    }

    /// Records Tombstones for the given ids in `table_name` (FDB-016
    /// delete-as-Tombstone semantics for the reduced core). Returns the new
    /// in-memory vector count.
    pub fn delete_records(
        &self,
        table_name: &str,
        ids: Vec<u64>,
    ) -> std::result::Result<u32, JsValue> {
        let mut inner = self.inner.borrow_mut();
        let path = inner.tables.get_mut(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        path.delete(&ids).map_err(to_js)?;
        Ok(path.delta().len() as u32)
    }

    /// Snapshot of the Table's Delta layout for the storage lifecycle
    /// inspector (FDB-EXP-07): sealed Segment row/dead counts, active buffer
    /// counts, and Tombstone totals.
    pub fn export_lifecycle(
        &self,
        table_name: &str,
    ) -> std::result::Result<LifecycleExport, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let delta = path.delta();
        let mut sealed_counts = Vec::new();
        let mut sealed_dead = Vec::new();
        for segment in delta.sealed_segments() {
            sealed_counts.push(segment.len() as u32);
            sealed_dead.push(
                segment
                    .records()
                    .iter()
                    .filter(|record| delta.is_tombstoned(record.id()))
                    .count() as u32,
            );
        }
        let active_total = delta.active_records().len() as u32;
        let active_dead = delta
            .active_records()
            .iter()
            .filter(|record| delta.is_tombstoned(record.id()))
            .count() as u32;
        Ok(LifecycleExport {
            sealed_counts,
            sealed_dead,
            active_total,
            active_dead,
            tombstoned_ids: delta.tombstoned_ids().count() as u32,
            total_rows: delta.len() as u32,
        })
    }

    /// Full snapshot of a Table's in-memory vectors for visualization
    /// (FDB-EXP-06): parallel ids/vectors plus per-record metadata JSON.
    pub fn export_vectors(&self, table_name: &str) -> std::result::Result<VectorExport, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let mut ids = Vec::new();
        let mut vectors = Vec::new();
        let mut metadata_json = Vec::new();
        for record in path.delta().records() {
            ids.push(record.id());
            vectors.extend_from_slice(record.vector());
            metadata_json.push(metadata_to_json(record.metadata()));
        }
        Ok(VectorExport {
            ids: js_sys::BigUint64Array::from(ids.as_slice()),
            vectors: js_sys::Float32Array::from(vectors.as_slice()),
            metadata_json,
        })
    }

    /// Appends `vectors.len() / dimension` records to `table_name`. `vectors`
    /// is a flat, row-major f32 array aligned with `ids` (one vector per id).
    /// Returns the number of vectors now held in the Table.
    pub fn insert_records(
        &self,
        table_name: &str,
        ids: Vec<u64>,
        vectors: Vec<f32>,
    ) -> std::result::Result<u32, JsValue> {
        let mut inner = self.inner.borrow_mut();
        let path = inner.tables.get_mut(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let dimension = path.delta().table().dimension() as usize;
        if dimension == 0 || vectors.len() != ids.len() * dimension {
            return Err(to_js(Error::DimensionMismatch {
                expected: dimension as u32,
                actual: (vectors.len() / ids.len().max(1)) as u32,
            }));
        }
        let records = ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                let vector = vectors[index * dimension..(index + 1) * dimension].to_vec();
                InsertRecord::new(id, vector, BTreeMap::new())
            })
            .collect();
        path.insert(records).map_err(to_js)?;
        Ok(path.delta().len() as u32)
    }

    /// Appends records with typed metadata to `table_name`. `vectors` is a
    /// flat row-major f32 array aligned with `ids`; `values` is a flat array of
    /// `ids.len() * col_names.len()` string literals, one per (record, column),
    /// parsed according to `col_types`. This is the ingestion path used by
    /// custom datasets and the synthetic generator (FDB-EXP-03).
    pub fn insert_with_metadata(
        &self,
        table_name: &str,
        ids: Vec<u64>,
        vectors: Vec<f32>,
        col_names: Vec<String>,
        col_types: Vec<String>,
        values: Vec<String>,
    ) -> std::result::Result<u32, JsValue> {
        let mut inner = self.inner.borrow_mut();
        let path = inner.tables.get_mut(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let dimension = path.delta().table().dimension() as usize;
        if dimension == 0 || vectors.len() != ids.len() * dimension {
            return Err(to_js(Error::DimensionMismatch {
                expected: dimension as u32,
                actual: (vectors.len() / ids.len().max(1)) as u32,
            }));
        }
        let columns = col_names.len();
        if values.len() != ids.len() * columns {
            return Err(to_js(Error::SchemaViolation {
                reason: format!(
                    "metadata value count {} does not match {} records x {} columns",
                    values.len(),
                    ids.len(),
                    columns
                ),
            }));
        }
        let parsed_types: FerriteResult<Vec<ColumnType>> =
            col_types.iter().map(|ty| parse_column_type(ty)).collect();
        let parsed_types = parsed_types.map_err(to_js)?;

        let records = ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                let vector = vectors[index * dimension..(index + 1) * dimension].to_vec();
                let mut metadata = BTreeMap::new();
                for (col_index, (col, col_type)) in
                    col_names.iter().zip(parsed_types.iter()).enumerate()
                {
                    let raw = &values[index * columns + col_index];
                    let value = parse_metadata_value(col_type, raw)?;
                    metadata.insert(col.clone(), value);
                }
                Ok(InsertRecord::new(id, vector, metadata))
            })
            .collect::<FerriteResult<Vec<_>>>()
            .map_err(to_js)?;
        path.insert(records).map_err(to_js)?;
        Ok(path.delta().len() as u32)
    }

    /// Exhaustive nearest-neighbour search over `table_name`'s in-memory
    /// Deltas, returning the `top_k` nearest hits. Mirrors the native engine's
    /// admission-gated exhaustive `search`.
    pub fn search(
        &self,
        table_name: &str,
        query: Vec<f32>,
        top_k: u32,
    ) -> std::result::Result<Vec<SearchHit>, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let options = SearchOptions::new().with_top_k(top_k).map_err(to_js)?;
        let results = search(path.delta(), &query, None, options).map_err(to_js)?;
        Ok(results.iter().map(hit_from_result).collect())
    }

    /// Independent brute-force oracle: scores every vector in `table_name`
    /// against `query` using the Table's Metric and returns the `top_k`
    /// nearest. Used for exact-match / recall validation distinct from the
    /// engine's own search path.
    pub fn exact_search(
        &self,
        table_name: &str,
        query: Vec<f32>,
        top_k: u32,
    ) -> std::result::Result<Vec<SearchHit>, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let table = path.delta().table();
        let dimension = table.dimension();
        let actual = u32::try_from(query.len()).unwrap_or(u32::MAX);
        if actual != dimension {
            return Err(to_js(Error::DimensionMismatch {
                expected: dimension,
                actual,
            }));
        }
        if top_k == 0 {
            return Err(to_js(Error::SchemaViolation {
                reason: "top_k must be at least 1".to_string(),
            }));
        }
        let mut scored: Vec<(f32, u64, &BTreeMap<String, MetadataValue>)> = path
            .delta()
            .records()
            .map(|record| {
                (
                    distance(table.metric(), &query, record.vector()),
                    record.id(),
                    record.metadata(),
                )
            })
            .collect();
        scored.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        let keep = top_k as usize;
        Ok(scored
            .into_iter()
            .take(keep)
            .map(|(distance, id, metadata)| SearchHit {
                id,
                distance,
                metadata_json: metadata_to_json(metadata),
            })
            .collect())
    }

    /// Engine search with full query-runner plumbing (FDB-EXP-04): dynamic
    /// `SearchOptions` overrides (`probes`, `ef_search`) and a Predicate Tree
    /// supplied as JSON (see [`parse_predicate_json`]). On the WASM reduced
    /// core the scan is exhaustive, so probe/ef knobs are carried for parity
    /// with the native substrate but do not change results.
    pub fn search_advanced(
        &self,
        table_name: &str,
        query: Vec<f32>,
        top_k: u32,
        probes: Option<u32>,
        ef_search: Option<u32>,
        predicate_json: Option<String>,
    ) -> std::result::Result<Vec<SearchHit>, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let options = SearchOptions::new()
            .with_top_k(top_k)
            .map_err(to_js)?
            .with_probes(probes)
            .with_ef_search(ef_search);
        let predicate =
            parse_optional_predicate(path.delta().table().metadata_schema(), predicate_json)
                .map_err(to_js)?;
        let results = search(path.delta(), &query, predicate.as_ref(), options).map_err(to_js)?;
        Ok(results.iter().map(hit_from_result).collect())
    }

    /// Exact brute-force oracle under the same predicate and knob plumbing as
    /// [`FerriteDb::search_advanced`], giving recall@k a like-for-like
    /// baseline when filters are active.
    pub fn exact_search_advanced(
        &self,
        table_name: &str,
        query: Vec<f32>,
        top_k: u32,
        predicate_json: Option<String>,
    ) -> std::result::Result<Vec<SearchHit>, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let table = path.delta().table();
        let dimension = table.dimension();
        let actual = u32::try_from(query.len()).unwrap_or(u32::MAX);
        if actual != dimension {
            return Err(to_js(Error::DimensionMismatch {
                expected: dimension,
                actual,
            }));
        }
        if top_k == 0 {
            return Err(to_js(Error::SchemaViolation {
                reason: "top_k must be at least 1".to_string(),
            }));
        }
        let predicate =
            parse_optional_predicate(table.metadata_schema(), predicate_json).map_err(to_js)?;
        if let Some(predicate) = &predicate {
            predicate.validate(table.metadata_schema()).map_err(to_js)?;
        }
        let mut scored: Vec<(f32, u64, &BTreeMap<String, MetadataValue>)> = path
            .delta()
            .records()
            .filter(|record| {
                predicate
                    .as_ref()
                    .is_none_or(|predicate| predicate.matches(record.metadata()))
            })
            .map(|record| {
                (
                    distance(table.metric(), &query, record.vector()),
                    record.id(),
                    record.metadata(),
                )
            })
            .collect();
        scored.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        let keep = top_k as usize;
        Ok(scored
            .into_iter()
            .take(keep)
            .map(|(distance, id, metadata)| SearchHit {
                id,
                distance,
                metadata_json: metadata_to_json(metadata),
            })
            .collect())
    }

    /// Per-phase execution profile of one exhaustive query (FDB-EXP-08):
    /// predicate filtering vs distance scan vs top-k ranking. The phases
    /// mirror exactly what the engine's fused [`search`] performs internally,
    /// timed here as separate passes over the same Delta for telemetry.
    pub fn profile_search(
        &self,
        table_name: &str,
        query: Vec<f32>,
        top_k: u32,
        predicate_json: Option<String>,
    ) -> std::result::Result<SearchProfile, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let delta = path.delta();
        let table = delta.table();
        let actual = u32::try_from(query.len()).unwrap_or(u32::MAX);
        if actual != table.dimension() {
            return Err(to_js(Error::DimensionMismatch {
                expected: table.dimension(),
                actual,
            }));
        }
        if top_k == 0 || top_k > 1000 {
            return Err(to_js(Error::SchemaViolation {
                reason: "top_k must be within 1..=1000".to_string(),
            }));
        }
        let predicate =
            parse_optional_predicate(table.metadata_schema(), predicate_json).map_err(to_js)?;
        if let Some(predicate) = &predicate {
            predicate.validate(table.metadata_schema()).map_err(to_js)?;
        }

        // Phase 1: predicate filter (and Tombstone visibility) over all rows.
        let t0 = std::time::Instant::now();
        let mut matched = Vec::new();
        for record in delta.records() {
            let keep = predicate
                .as_ref()
                .is_none_or(|predicate| predicate.matches(record.metadata()));
            if keep && !delta.is_tombstoned(record.id()) {
                matched.push(record);
            }
        }
        let matched_rows = matched.len() as u32;
        let filter_us = t0.elapsed().as_micros() as u64;

        // Phase 2: metric scan over the surviving rows.
        let t1 = std::time::Instant::now();
        let mut scored: Vec<(f32, u64)> = matched
            .iter()
            .map(|record| {
                (
                    distance(table.metric(), &query, record.vector()),
                    record.id(),
                )
            })
            .collect();
        let scanned_rows = scored.len() as u32;
        let scan_us = t1.elapsed().as_micros() as u64;

        // Phase 3: deterministic ranking + truncation.
        let t2 = std::time::Instant::now();
        scored.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        scored.truncate(top_k as usize);
        let rank_us = t2.elapsed().as_micros() as u64;

        Ok(SearchProfile {
            total_rows: delta.len() as u32,
            scanned_rows,
            matched_rows,
            returned_rows: scored.len() as u32,
            filter_us,
            scan_us,
            rank_us,
        })
    }

    /// Per-Table status: dimension, Metric, and the in-memory vector count.
    pub fn table_status(&self, table_name: &str) -> std::result::Result<TableStatus, JsValue> {
        let inner = self.inner.borrow();
        let path = inner.tables.get(table_name).ok_or_else(|| {
            to_js(Error::TableNotFound {
                name: table_name.to_string(),
            })
        })?;
        let table = path.delta().table();
        Ok(TableStatus {
            name: table.name().to_string(),
            dimension: table.dimension(),
            metric: metric_name(table.metric()).to_string(),
            vectors: path.delta().len() as u32,
        })
    }

    /// Whole-session status: how many Tables exist and how many vectors are
    /// held in total.
    pub fn status(&self) -> SessionStatus {
        let inner = self.inner.borrow();
        let vector_count: usize = inner.tables.values().map(|path| path.delta().len()).sum();
        SessionStatus {
            table_count: inner.tables.len() as u32,
            vector_count: vector_count as u32,
        }
    }
}

impl Default for FerriteDb {
    fn default() -> Self {
        FerriteDb {
            inner: RefCell::new(Inner {
                manager: TableManager::new(),
                tables: BTreeMap::new(),
                known_tables: Vec::new(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: error paths return `JsValue`, and `JsValue` conversions panic on
    // non-`wasm32` targets, so these tests exercise only the success surface
    // (the WASM build is the authoritative environment for error handling).
    #[test]
    fn end_to_end_in_memory_table() {
        let db = FerriteDb::new();
        db.create_table("vectors", 2, "l2").unwrap();

        let ids = vec![0u64, 1, 2, 3];
        let vectors = vec![
            0.0f32, 0.0, // id 0 at origin
            1.0, 0.0, // id 1
            0.0, 1.0, // id 2
            10.0, 10.0, // id 3 far away
        ];
        let inserted = db.insert_records("vectors", ids, vectors).unwrap();
        assert_eq!(inserted, 4);

        // Nearest neighbour of the origin should be a small-distance hit set.
        let hits = db.search("vectors", vec![0.1, 0.1], 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, 0);

        // The exact oracle must agree with the engine's exhaustive search.
        let exact = db.exact_search("vectors", vec![0.1, 0.1], 2).unwrap();
        assert_eq!(exact.len(), 2);
        assert_eq!(exact[0].id, hits[0].id);
        assert!((exact[0].distance - hits[0].distance).abs() < 1e-6);

        // Status reflects the single Table and its four vectors.
        let status = db.status();
        assert_eq!(status.table_count, 1);
        assert_eq!(status.vector_count, 4);
        let table = db.table_status("vectors").unwrap();
        assert_eq!(table.name(), "vectors");
        assert_eq!(table.dimension, 2);
        assert_eq!(table.metric(), "l2");
        assert_eq!(table.vectors, 4);
    }
}
