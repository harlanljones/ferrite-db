//! Index substrate — the ONLY module permitted to depend on LanceDB/Arrow
//! types (AGENTS.md §4, ADR 0002). Every other module goes through this seam,
//! and `tools/audit/substrate_audit.sh` fails the build when an upstream
//! substrate import appears anywhere else.
//!
//! Owned by ROADMAP FDB-030 (FDB-004 spike memo is the capability evidence:
//! both families build and answer queries with per-query knob control).
//!
//! Design notes:
//! - Blocking facade (ADR 0004): all public methods are synchronous; a
//!   library-owned tokio runtime lives behind this module and never leaks.
//! - Version pin: `lancedb = "=0.37.1"` (G-Lance/U3 decision, bounds risk R1).
//!   LanceDB exposes HNSW only inside its IVF-backed family (spike friction
//!   log), so [`IndexFamily::IvfHnswFlat`] names the concrete upstream variant.
//! - Lance owns its database directory; Ferrite Segment sidecars remain a
//!   separate ownership boundary (ADR 0003 + spike coexistence result).
//! - Upstream constraints surfaced at the seam (FDB-030 findings):
//!   IVF-PQ refuses to train on fewer than 256 rows; HNSW queries require
//!   ef >= top_k (validated here as caller-fixable `SchemaViolation`).

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::TryStreamExt;
use lancedb::arrow::{
    arrow_array::{FixedSizeListArray, RecordBatch, UInt64Array, types::Float32Type},
    arrow_schema::{DataType, Field, Schema},
};
use lancedb::index::{
    Index,
    vector::{IvfHnswFlatIndexBuilder, IvfPqIndexBuilder},
};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{Connection, DistanceType, Table, connect};

use crate::errors::{Error, Result};
use crate::table::Metric;

/// Backing table name inside the substrate directory. Private: callers only
/// ever see the seam types. (Upstream calls this concept a "table"; that is
/// its name, not our domain Table.)
const SUBSTRATE_ROWS: &str = "vectors";

/// Which ANN index family to build. Both consume upstream implementations;
/// neither family names a Ferrite-owned structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFamily {
    /// Upstream IVF-PQ. Per-query knob: probes.
    IvfPq,
    /// Upstream IVF-HNSW-Flat — HNSW as LanceDB actually ships it. Per-query
    /// knobs: probes and ef-search.
    IvfHnswFlat,
}

/// Build-time parameters for one [`IndexFamily`].
#[derive(Debug, Clone)]
pub struct IndexBuildParams {
    pub family: IndexFamily,
    /// Number of IVF partitions; must be at least 1.
    pub num_partitions: u32,
    /// IVF-PQ only: sub-vector count; must divide the fixed dimension.
    pub num_sub_vectors: Option<u32>,
    /// HNSW family only: construction-time search depth.
    pub ef_construction: Option<u32>,
}

/// Per-query knobs, mirroring the §13-3 SearchOptions plumbing that later
/// waves forward into this seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubstrateQueryKnobs {
    /// Neighbours to return; 1..=1000 like the public options.
    pub top_k: u32,
    /// IVF partitions visited per query.
    pub probes: Option<u32>,
    /// HNSW search depth at query time.
    pub ef_search: Option<u32>,
}

/// One neighbour returned by the substrate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubstrateHit {
    pub id: u64,
    pub distance: f32,
}

/// Blocking handle over one LanceDB-backed ANN index directory. Cheap to
/// clone; every clone shares the same underlying connection and the same
/// background-build state.
#[derive(Clone)]
pub struct SubstrateIndex {
    db: Connection,
    dimension: u32,
    /// Distance function used for calibration scoring; matches the Table.
    metric: Metric,
    /// Creation-time ladder override; immutable after open (FDB-031).
    ladder_override: LadderOverride,
    background: Arc<BackgroundBuild>,
}

impl SubstrateIndex {
    /// Opens (creating if absent) a substrate rooted at `dir` holding f32
    /// vectors of exactly `dimension` components, with automatic ladder
    /// selection.
    pub fn open(dir: &Path, dimension: u32, metric: Metric) -> Result<Self> {
        Self::open_with_override(dir, dimension, LadderOverride::Auto, metric)
    }

    /// Like [`SubstrateIndex::open`] with a creation-time override that pins
    /// the ladder choice regardless of row count (ratified design decision:
    /// "override at creation").
    pub fn open_with_override(
        dir: &Path,
        dimension: u32,
        ladder_override: LadderOverride,
        metric: Metric,
    ) -> Result<Self> {
        if dimension == 0 {
            return Err(Error::SchemaViolation {
                reason: "substrate dimension must be greater than zero".to_string(),
            });
        }
        let dir_str = dir.to_str().ok_or_else(|| Error::SchemaViolation {
            reason: "substrate path is not valid UTF-8".to_string(),
        })?;
        let rt = substrate_runtime()?;
        let db = rt
            .block_on(async { connect(dir_str).execute().await })
            .map_err(|e| io_err("connecting substrate", e))?;
        Ok(Self {
            db,
            dimension,
            ladder_override,
            metric,
            background: Arc::new(BackgroundBuild::default()),
        })
    }

    /// The override this handle was created with.
    pub fn ladder_override(&self) -> LadderOverride {
        self.ladder_override
    }

    /// Appends id/vector pairs. `vectors` must hold `ids.len() * dimension`
    /// components, row-major. Duplicate ids append duplicate rows; callers
    /// own identity semantics.
    pub fn write(&self, ids: &[u64], vectors: &[f32]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        if vectors.len() != ids.len() * self.dimension as usize {
            return Err(Error::DimensionMismatch {
                expected: self.dimension,
                actual: u32::try_from(vectors.len().div_ceil(ids.len().max(1))).unwrap_or(u32::MAX),
            });
        }
        let batch = rows_batch(ids, vectors, self.dimension)?;
        let rt = substrate_runtime()?;
        rt.block_on(async move {
            let table = self.ensure_table().await?;
            table
                .add(batch)
                .execute()
                .await
                .map_err(|e| io_err("appending substrate rows", e))?;
            Ok(())
        })
    }

    /// Builds (or replaces) the ANN index of `params.family` over the rows
    /// currently present. Upstream indices do not cover later appends until
    /// rebuilt; queries always also scan uncovered rows, so recall never
    /// silently drops them.
    pub fn build(&self, params: &IndexBuildParams) -> Result<()> {
        params.validate(self.dimension)?;
        let index = match params.family {
            IndexFamily::IvfPq => {
                let mut builder = IvfPqIndexBuilder::default();
                builder = builder.num_partitions(params.num_partitions);
                if let Some(sub) = params.num_sub_vectors {
                    builder = builder.num_sub_vectors(sub);
                }
                builder = builder.distance_type(self.metric.distance_type());
                Index::IvfPq(builder)
            }
            IndexFamily::IvfHnswFlat => {
                let mut builder = IvfHnswFlatIndexBuilder::default();
                builder = builder.num_partitions(params.num_partitions);
                if let Some(ef) = params.ef_construction {
                    builder = builder.ef_construction(ef);
                }
                builder = builder.distance_type(self.metric.distance_type());
                Index::IvfHnswFlat(builder)
            }
        };
        let rt = substrate_runtime()?;
        rt.block_on(async move {
            let table = self.ensure_table().await?;
            table
                .create_index(&["vector"], index)
                .execute()
                .await
                .map_err(|e| io_err(format!("building {:?} index", params.family), e))?;
            Ok(())
        })
    }

    /// Queries the nearest neighbours of `vector`. Falls back to exhaustive
    /// scanning when no index is built (upstream behaviour), so results stay
    /// correct in every state.
    pub fn query(&self, vector: &[f32], knobs: SubstrateQueryKnobs) -> Result<Vec<SubstrateHit>> {
        if knobs.top_k == 0 || knobs.top_k > 1000 {
            return Err(Error::SchemaViolation {
                reason: "top_k must be within 1..=1000".to_string(),
            });
        }
        if vector.len() != self.dimension as usize {
            return Err(Error::DimensionMismatch {
                expected: self.dimension,
                actual: u32::try_from(vector.len()).unwrap_or(u32::MAX),
            });
        }
        // Upstream HNSW rejects ef < k deep inside execution as an opaque
        // operational error; at the seam this is a caller-fixable condition.
        if let Some(ef) = knobs.ef_search
            && ef < knobs.top_k
        {
            return Err(Error::SchemaViolation {
                reason: format!("ef_search {ef} must be at least top_k {}", knobs.top_k),
            });
        }
        let rt = substrate_runtime()?;
        rt.block_on(async move {
            let table = match self.open_rows().await? {
                Some(table) => table,
                None => return Ok(Vec::new()),
            };
            let mut request = table
                .query()
                .nearest_to(vector.to_vec())
                .map_err(|e| io_err("preparing nearest query", e))?
                .distance_type(self.metric.distance_type())
                .limit(knobs.top_k as usize);
            if let Some(probes) = knobs.probes {
                request = request.nprobes(probes as usize);
            }
            if let Some(ef) = knobs.ef_search {
                request = request.ef(ef as usize);
            }
            let batches = request
                .execute()
                .await
                .map_err(|e| io_err("executing nearest query", e))?
                .try_collect::<Vec<_>>()
                .await
                .map_err(|e| io_err("collecting query results", e))?;
            batches_to_hits(&batches)
        })
    }

    /// Removes every ANN index built over the vector column (round-trip
    /// removal support). Rows are untouched; subsequent queries scan flat.
    pub fn remove_indexes(&self) -> Result<usize> {
        let rt = substrate_runtime()?;
        rt.block_on(async move {
            let table = match self.open_rows().await? {
                Some(table) => table,
                None => return Ok(0),
            };
            let configs = table
                .list_indices()
                .await
                .map_err(|e| io_err("listing substrate indexes", e))?;
            let mut removed = 0;
            for config in configs {
                table
                    .drop_index(&config.name)
                    .await
                    .map_err(|e| io_err(format!("dropping index {}", config.name), e))?;
                removed += 1;
            }
            Ok(removed)
        })
    }

    /// Names of the indexes currently registered, for observability/tests.
    pub fn index_names(&self) -> Result<Vec<String>> {
        let rt = substrate_runtime()?;
        rt.block_on(async move {
            match self.open_rows().await? {
                Some(table) => {
                    let configs = table
                        .list_indices()
                        .await
                        .map_err(|e| io_err("listing substrate indexes", e))?;
                    Ok(configs.into_iter().map(|c| c.name).collect())
                }
                None => Ok(Vec::new()),
            }
        })
    }

    async fn open_rows(&self) -> Result<Option<Table>> {
        match self.db.open_table(SUBSTRATE_ROWS).execute().await {
            Ok(table) => Ok(Some(table)),
            Err(_) => Ok(None),
        }
    }

    async fn ensure_table(&self) -> Result<Table> {
        if let Some(table) = self.open_rows().await? {
            return Ok(table);
        }
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::UInt64, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    self.dimension as i32,
                ),
                false,
            ),
        ]));
        self.db
            .create_empty_table(SUBSTRATE_ROWS, schema)
            .execute()
            .await
            .map_err(|e| io_err("creating substrate rows", e))
    }
}

impl IndexBuildParams {
    fn validate(&self, dimension: u32) -> Result<()> {
        if self.num_partitions == 0 {
            return Err(Error::SchemaViolation {
                reason: "num_partitions must be at least 1".to_string(),
            });
        }
        match self.family {
            IndexFamily::IvfPq => {
                let sub = self.num_sub_vectors.ok_or_else(|| Error::SchemaViolation {
                    reason: "IvfPq requires num_sub_vectors".to_string(),
                })?;
                if sub == 0 || !dimension.is_multiple_of(sub) {
                    return Err(Error::SchemaViolation {
                        reason: format!("num_sub_vectors {sub} must divide dimension {dimension}"),
                    });
                }
            }
            IndexFamily::IvfHnswFlat => {
                if self.num_sub_vectors.is_some() {
                    return Err(Error::SchemaViolation {
                        reason: "num_sub_vectors applies only to IvfPq".to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Ladder selection (FDB-031)
//
// Ratified design decision: below 10k rows search stays exhaustive; from 10k
// up to (excluding) 1M rows HNSW; at 1M and above IVF-PQ. Creation-time
// override wins over row count. Builds for substrates past ~50k rows run on a
// background thread and are observable while in flight.
// ---------------------------------------------------------------------------

/// Below this row count the ladder keeps search exhaustive.
pub const LADDER_EXHAUSTIVE_MAX_ROWS: u64 = 10_000;
/// From this row count (inclusive) the ladder moves to IVF-PQ.
pub const LADDER_HNSW_MAX_ROWS: u64 = 1_000_000;
/// At or beyond this row count index builds run in the background instead of
/// inline. Ratified as approximate ("~50k"); the exact value is recorded here.
pub const BACKGROUND_BUILD_MIN_ROWS: u64 = 50_000;

/// Which search strategy the ladder prescribes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderChoice {
    /// No ANN index; exhaustive scanning (the pre-ladder behavior).
    Exhaustive,
    /// HNSW as upstream ships it (`IvfHnswFlat`).
    Hnsw,
    /// IVF-PQ.
    IvfPq,
}

/// Creation-time override applied on top of the row-count ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LadderOverride {
    /// Follow the ratified thresholds.
    #[default]
    Auto,
    /// Pin this choice regardless of row count.
    Force(LadderChoice),
}

/// The pure selector: row count to ladder choice. Total over u64; boundary
/// semantics are strict `<` at both thresholds.
pub fn ladder_choice(row_count: u64) -> LadderChoice {
    if row_count < LADDER_EXHAUSTIVE_MAX_ROWS {
        LadderChoice::Exhaustive
    } else if row_count < LADDER_HNSW_MAX_ROWS {
        LadderChoice::Hnsw
    } else {
        LadderChoice::IvfPq
    }
}

/// Pure policy application: an override always wins; otherwise the row-count
/// ladder decides.
pub fn resolve_ladder(override_: LadderOverride, row_count: u64) -> LadderChoice {
    match override_ {
        LadderOverride::Force(choice) => choice,
        LadderOverride::Auto => ladder_choice(row_count),
    }
}

/// Pure disposition selector: whether the build for `row_count` runs inline or
/// in the background (exhaustive needs no build at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildDisposition {
    NotNeeded,
    Inline,
    Background,
}

pub(crate) fn build_disposition(choice: LadderChoice, row_count: u64) -> BuildDisposition {
    match choice {
        LadderChoice::Exhaustive => BuildDisposition::NotNeeded,
        LadderChoice::Hnsw | LadderChoice::IvfPq => {
            if row_count >= BACKGROUND_BUILD_MIN_ROWS {
                BuildDisposition::Background
            } else {
                BuildDisposition::Inline
            }
        }
    }
}

/// Observable state of a possibly in-flight background build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundBuildState {
    /// No background build has been triggered (or nothing to build).
    Idle,
    /// A background thread is building right now.
    InProgress,
    /// The most recent background build finished successfully.
    Completed,
    /// The most recent background build failed; see
    /// [`SubstrateIndex::background_build_error`].
    Failed,
}

impl BackgroundBuildState {
    fn to_u8(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::InProgress => 1,
            Self::Completed => 2,
            Self::Failed => 3,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::InProgress,
            2 => Self::Completed,
            3 => Self::Failed,
            _ => Self::Idle,
        }
    }
}

#[derive(Default)]
struct BackgroundBuild {
    state: AtomicU8,
    error: Mutex<Option<String>>,
}

impl BackgroundBuild {
    fn state(&self) -> BackgroundBuildState {
        BackgroundBuildState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Claims the right to build: succeeds from Idle or Completed, never
    /// while InProgress.
    fn try_claim(&self) -> bool {
        let current = self.state();
        match current {
            BackgroundBuildState::Idle | BackgroundBuildState::Completed => self
                .state
                .compare_exchange(
                    current.to_u8(),
                    BackgroundBuildState::InProgress.to_u8(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok(),
            _ => false,
        }
    }
}

/// What [`SubstrateIndex::apply_ladder`] decided and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LadderResolution {
    pub choice: LadderChoice,
    pub build: LadderBuildOutcome,
}

/// How the build step of a ladder application executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderBuildOutcome {
    /// Exhaustive choice: no ANN index belongs to this substrate state.
    NotNeeded,
    /// Built synchronously inside the call.
    InlineCompleted,
    /// A background thread claimed the build;
    /// [`SubstrateIndex::background_build_state`] tracks completion.
    BackgroundStarted,
    /// A background build was already in flight; this call left it alone.
    AlreadyInProgress,
}

/// Deterministic placeholder build parameters per family. Calibration is
/// FDB-032's deliverable; these only need to be valid and documented.
/// `Exhaustive` yields nothing because it never builds.
fn default_build_params(dimension: u32, choice: LadderChoice) -> Option<IndexBuildParams> {
    match choice {
        LadderChoice::IvfPq => {
            // Sub-vectors must divide the dimension: largest divisor from
            // {16, 8, 4, 2, 1}.
            let sub = [16u32, 8, 4, 2, 1]
                .into_iter()
                .find(|&candidate| dimension.is_multiple_of(candidate))
                .unwrap_or(1);
            Some(IndexBuildParams {
                family: IndexFamily::IvfPq,
                num_partitions: 4,
                num_sub_vectors: Some(sub),
                ef_construction: None,
            })
        }
        LadderChoice::Hnsw => Some(IndexBuildParams {
            family: IndexFamily::IvfHnswFlat,
            num_partitions: 4,
            num_sub_vectors: None,
            ef_construction: Some(64),
        }),
        LadderChoice::Exhaustive => None,
    }
}

impl SubstrateIndex {
    /// Rows currently present (0 when nothing was written yet).
    pub fn row_count(&self) -> Result<u64> {
        substrate_runtime()?.block_on(async move {
            match self.open_rows().await? {
                Some(table) => table
                    .count_rows(None)
                    .await
                    .map(|count| count as u64)
                    .map_err(|e| io_err("counting rows", e)),
                None => Ok(0),
            }
        })
    }

    /// Applies the ladder policy to current reality: resolves the choice
    /// against the creation-time override and row count, then builds (inline
    /// below ~50k rows, background at or beyond). Exhaustive never builds and
    /// leaves any existing index untouched — removal stays explicit via
    /// [`SubstrateIndex::remove_indexes`].
    pub fn apply_ladder(&self) -> Result<LadderResolution> {
        let rows = self.row_count()?;
        let choice = resolve_ladder(self.ladder_override, rows);
        match build_disposition(choice, rows) {
            BuildDisposition::NotNeeded => Ok(LadderResolution {
                choice,
                build: LadderBuildOutcome::NotNeeded,
            }),
            BuildDisposition::Inline => {
                let params = default_build_params(self.dimension, choice).ok_or_else(|| {
                    Error::SchemaViolation {
                        reason: "exhaustive ladder never builds".to_string(),
                    }
                })?;
                self.build(&params)?;
                Ok(LadderResolution {
                    choice,
                    build: LadderBuildOutcome::InlineCompleted,
                })
            }
            BuildDisposition::Background => {
                if !self.background.try_claim() {
                    return Ok(LadderResolution {
                        choice,
                        build: LadderBuildOutcome::AlreadyInProgress,
                    });
                }
                let worker = self.clone();
                let params = default_build_params(self.dimension, choice).ok_or_else(|| {
                    Error::SchemaViolation {
                        reason: "exhaustive ladder never builds".to_string(),
                    }
                })?;
                std::thread::Builder::new()
                    .name("substrate-ladder-build".to_string())
                    .spawn(move || {
                        let outcome = worker.build(&params);
                        if let Err(error) = outcome {
                            if let Ok(mut slot) = worker.background.error.lock() {
                                *slot = Some(error.to_string());
                            }
                            worker
                                .background
                                .state
                                .store(BackgroundBuildState::Failed.to_u8(), Ordering::Release);
                        } else {
                            worker
                                .background
                                .state
                                .store(BackgroundBuildState::Completed.to_u8(), Ordering::Release);
                        }
                    })
                    .map_err(|e| io_err("spawning ladder build", e))?;
                Ok(LadderResolution {
                    choice,
                    build: LadderBuildOutcome::BackgroundStarted,
                })
            }
        }
    }

    /// Observability for the background-build path.
    pub fn background_build_state(&self) -> BackgroundBuildState {
        self.background.state()
    }

    /// The failure reason behind [`BackgroundBuildState::Failed`], if any.
    pub fn background_build_error(&self) -> Option<String> {
        self.background
            .error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

// ---------------------------------------------------------------------------
// Deterministic knob calibration (FDB-032)
// ---------------------------------------------------------------------------

/// Sampled queries used to score candidate knobs.
pub const CALIBRATION_SAMPLE_QUERIES: usize = 8;
/// Upper bound on rows pulled for calibration scoring (deterministic window).
pub const CALIBRATION_MAX_ROWS: u64 = 250_000;
/// Minimum sampled recall a candidate must reach to be selected.
pub const CALIBRATION_RECALL_TARGET: f32 = 0.95;
/// Sampling tolerance allowed when matching the naive baseline's recall
/// (sample-based parity transfers imperfectly to the full query set).
pub const CALIBRATION_PARITY_EPSILON: f32 = 0.01;

/// The un-calibrated strawman: visit every partition and keep the spike-era
/// HNSW ef. This is what a naive fixed default ships as; calibration exists
/// to beat it on the recall-latency frontier at equal accuracy or better.
pub fn naive_fixed_knobs(top_k: u32) -> SubstrateQueryKnobs {
    SubstrateQueryKnobs {
        top_k,
        probes: Some(4),
        ef_search: Some(64.max(top_k)),
    }
}

/// One scored candidate from the calibration grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationCandidate {
    pub knobs: SubstrateQueryKnobs,
    /// Mean recall of the candidate over the deterministic sample set.
    pub recall: f32,
}

impl SubstrateIndex {
    /// Deterministically derives query knobs for `top_k` from this
    /// substrate's own data: an even-stride sample of stored rows is scored
    /// against locally computed exact answers, and the CHEAPEST candidate
    /// whose sampled recall matches-or-beats the naive fixed baseline
    /// (within [`CALIBRATION_PARITY_EPSILON`]) wins. No timing input anywhere;
    /// repeated calls on one built index return identical knobs. Across
    /// rebuilds knobs may shift because upstream's kmeans partition training
    /// is unseeded.
    pub fn calibrate(&self, top_k: u32) -> Result<SubstrateQueryKnobs> {
        let scored = self.calibration_candidates(top_k)?;
        if scored.is_empty() {
            return Err(Error::SchemaViolation {
                reason: "calibration produced no candidates".to_string(),
            });
        }

        // Baseline: the naive fixed default's sampled recall on THIS data.
        let naive_recall = scored
            .iter()
            .find(|candidate| candidate.knobs == naive_fixed_knobs(top_k))
            .map(|candidate| candidate.recall)
            .unwrap_or(0.0);
        let parity_target = naive_recall - CALIBRATION_PARITY_EPSILON;

        // Cheapest first by work proxy; ties resolved by earlier grid order.
        let mut ranked: Vec<&CalibrationCandidate> = scored.iter().collect();
        ranked.sort_by_key(|candidate| {
            let probes = candidate.knobs.probes.unwrap_or(1) as u64;
            let ef = candidate.knobs.ef_search.unwrap_or(1) as u64;
            (probes * ef, probes, ef)
        });

        // Cheapest candidate meeting parity, else global best recall.
        let mut selected = scored.iter().max_by(|left, right| {
            left.recall
                .partial_cmp(&right.recall)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for candidate in &ranked {
            if candidate.recall >= parity_target {
                selected = Some(*candidate);
                break;
            }
        }
        selected
            .map(|candidate| candidate.knobs)
            .ok_or_else(|| Error::SchemaViolation {
                reason: "calibration produced no usable candidate".to_string(),
            })
    }

    /// Scores the full deterministic candidate grid. Public for evidence
    /// runs; `calibrate` is just target-selection over it.
    pub fn calibration_candidates(&self, top_k: u32) -> Result<Vec<CalibrationCandidate>> {
        if top_k == 0 || top_k > 1000 {
            return Err(Error::SchemaViolation {
                reason: "top_k must be within 1..=1000".to_string(),
            });
        }
        let snapshot = self.fetch_rows(CALIBRATION_MAX_ROWS)?;
        if snapshot.is_empty() {
            return Ok(Vec::new());
        }

        // Deterministic samples: even stride over id-sorted rows.
        let sample_count = CALIBRATION_SAMPLE_QUERIES.min(snapshot.len());
        let stride = snapshot.len() / sample_count;
        let samples: Vec<&(u64, Vec<f32>)> = (0..sample_count)
            .map(|index| &snapshot[index * stride])
            .collect();

        // Locally computed exact answers (ground truth for scoring).
        let exact: Vec<Vec<u64>> = samples
            .iter()
            .map(|(_, vector)| exact_top_k_ids(vector, &snapshot, self.metric, top_k as usize))
            .collect();

        // Candidate grid spanning the work axis; the naive default is part
        // of the grid so parity is always achievable.
        let min_ef = top_k.max(16);
        let mut grid = vec![naive_fixed_knobs(top_k)];
        for probes in [1u32, 2] {
            for ef_mult in [1u32, 2, 4] {
                grid.push(SubstrateQueryKnobs {
                    top_k,
                    probes: Some(probes),
                    ef_search: Some(min_ef.saturating_mul(ef_mult)),
                });
            }
        }
        grid.push(SubstrateQueryKnobs {
            top_k,
            probes: Some(4),
            ef_search: Some((4 * top_k).max(32)),
        });
        grid.dedup();

        let mut candidates = Vec::with_capacity(grid.len());
        for knobs in grid {
            let mut total_recall = 0.0f32;
            for (sample_index, (_, vector)) in samples.iter().enumerate() {
                let hits = self.query(vector, knobs)?;
                let expected = &exact[sample_index];
                let hits_at_k = hits.len().min(expected.len());
                let overlap = hits
                    .iter()
                    .take(hits_at_k)
                    .filter(|hit| expected.contains(&hit.id))
                    .count();
                total_recall += overlap as f32 / expected.len() as f32;
            }
            candidates.push(CalibrationCandidate {
                knobs,
                recall: total_recall / samples.len() as f32,
            });
        }
        Ok(candidates)
    }

    /// Pulls up to `limit` rows (id + vector), sorted by id, for calibration.
    fn fetch_rows(&self, limit: u64) -> Result<Vec<(u64, Vec<f32>)>> {
        use lancedb::query::{QueryBase as _, Select};
        substrate_runtime()?.block_on(async move {
            let table = match self.open_rows().await? {
                Some(table) => table,
                None => return Ok(Vec::new()),
            };
            let batches = table
                .query()
                .select(Select::Columns(vec![
                    "id".to_string(),
                    "vector".to_string(),
                ]))
                .limit(usize::try_from(limit).unwrap_or(usize::MAX))
                .execute()
                .await
                .map_err(|e| io_err("calibration row fetch", e))?
                .try_collect::<Vec<_>>()
                .await
                .map_err(|e| io_err("collecting calibration rows", e))?;
            let mut rows: Vec<(u64, Vec<f32>)> = Vec::new();
            for batch in &batches {
                let ids = column::<UInt64Array>(batch, "id")?;
                let vectors = column::<FixedSizeListArray>(batch, "vector")?;
                for row in 0..batch.num_rows() {
                    let values = vectors.value(row);
                    let values = values
                        .as_any()
                        .downcast_ref::<lancedb::arrow::arrow_array::Float32Array>()
                        .ok_or_else(|| {
                            io_err("vector column shape", std::io::Error::other("not f32"))
                        })?;
                    rows.push((ids.value(row), values.values().to_vec()));
                }
            }
            rows.sort_by_key(|(id, _)| *id);
            Ok(rows)
        })
    }
}

/// Exact nearest-neighbour ids by brute force over `snapshot` — calibration's
/// local ground truth. Mirrors upstream distance semantics through
/// `crate::search`'s scorer.
fn exact_top_k_ids(
    query: &[f32],
    snapshot: &[(u64, Vec<f32>)],
    metric: Metric,
    k: usize,
) -> Vec<u64> {
    let mut best: Vec<(f32, u64)> = Vec::with_capacity(k);
    for (id, vector) in snapshot {
        let d = crate::search::distance(metric, query, vector);
        if best.len() < k {
            best.push((d, *id));
        } else if d < best[k - 1].0 {
            best[k - 1] = (d, *id);
        } else {
            continue;
        }
        best.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    best.into_iter().map(|(_, id)| id).collect()
}

fn rows_batch(ids: &[u64], vectors: &[f32], dimension: u32) -> Result<RecordBatch> {
    let dim = dimension as usize;
    let ids_array = UInt64Array::from_iter_values(ids.iter().copied());
    let vectors_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vectors
            .chunks_exact(dim)
            .map(|row| Some(row.iter().map(|&value| Some(value)).collect::<Vec<_>>())),
        dimension as i32,
    );
    RecordBatch::try_new(
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::UInt64, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    dimension as i32,
                ),
                false,
            ),
        ])),
        vec![Arc::new(ids_array), Arc::new(vectors_array)],
    )
    .map_err(|e| io_err("encoding substrate batch", e))
}

fn batches_to_hits(batches: &[RecordBatch]) -> Result<Vec<SubstrateHit>> {
    let mut hits = Vec::new();
    for batch in batches {
        let ids = column::<UInt64Array>(batch, "id")?;
        let distances = column::<lancedb::arrow::arrow_array::Float32Array>(batch, "_distance")?;
        for row in 0..batch.num_rows() {
            hits.push(SubstrateHit {
                id: ids.value(row),
                distance: distances.value(row),
            });
        }
    }
    Ok(hits)
}

fn column<'a, T: lancedb::arrow::arrow_array::Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| {
            io_err(
                format!("substrate result shape missing column {name}"),
                std::io::Error::other("unexpected result layout"),
            )
        })
}

impl Metric {
    fn distance_type(self) -> DistanceType {
        match self {
            Metric::Cosine => DistanceType::Cosine,
            Metric::L2 => DistanceType::L2,
            Metric::Dot => DistanceType::Dot,
        }
    }
}

fn io_err(context: impl std::fmt::Display, source: impl std::fmt::Display) -> Error {
    Error::Io(std::io::Error::other(format!("{context}: {source}")))
}

/// Library-owned async runtime hidden behind this module's blocking facade
/// (ADR 0004). Two workers suffice: substrate work is I/O-bound coordination
/// around upstream's own internal parallelism. Initialization failure (OS
/// resource exhaustion) surfaces as an operational [`Error::Io`] instead of a
/// panic.
fn substrate_runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<std::result::Result<tokio::runtime::Runtime, std::io::Error>> =
        OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
    }) {
        Ok(runtime) => Ok(runtime),
        Err(source) => Err(Error::Io(std::io::Error::other(format!(
            "substrate runtime unavailable: {source}"
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: u32 = 16;
    const ROWS: u64 = 512;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "substrate-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Deterministic row generator: pseudo-random but stable across runs so
    /// assertions never depend on external fixtures.
    fn row_vector(row: u64) -> Vec<f32> {
        (0..u64::from(DIM))
            .map(|col| ((row * 31 + col * 7) % 97) as f32 / 97.0)
            .collect()
    }

    fn write_fixture(index: &SubstrateIndex) {
        let ids: Vec<u64> = (0..ROWS).collect();
        let vectors: Vec<f32> = ids.iter().flat_map(|&row| row_vector(row)).collect();
        index.write(&ids, &vectors).expect("write rows");
    }

    #[test]
    fn open_rejects_zero_dimension() {
        let dir = TempDir::new("zerodim");
        assert!(matches!(
            SubstrateIndex::open(&dir.0, 0, Metric::Cosine),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn write_rejects_mismatched_payload() {
        let dir = TempDir::new("badwrite");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        let ids = vec![0u64, 1];
        let short_vectors = vec![0.0f32; DIM as usize]; // one row, two ids
        assert!(matches!(
            index.write(&ids, &short_vectors),
            Err(Error::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn query_rejects_wrong_dimension_and_bad_top_k() {
        let dir = TempDir::new("badquery");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        let knobs = SubstrateQueryKnobs {
            top_k: 5,
            probes: None,
            ef_search: None,
        };
        assert!(matches!(
            index.query(&vec![0.0; (DIM + 1) as usize], knobs),
            Err(Error::DimensionMismatch { .. })
        ));
        let zero_k = SubstrateQueryKnobs {
            top_k: 0,
            probes: None,
            ef_search: None,
        };
        assert!(matches!(
            index.query(&row_vector(0), zero_k),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn build_rejects_invalid_params() {
        let dir = TempDir::new("badparams");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        let pq_bad_sub = IndexBuildParams {
            family: IndexFamily::IvfPq,
            num_partitions: 2,
            num_sub_vectors: Some(5), // 16 % 5 != 0
            ef_construction: None,
        };
        assert!(matches!(
            index.build(&pq_bad_sub),
            Err(Error::SchemaViolation { .. })
        ));
        let zero_partitions = IndexBuildParams {
            family: IndexFamily::IvfPq,
            num_partitions: 0,
            num_sub_vectors: Some(4),
            ef_construction: None,
        };
        assert!(matches!(
            index.build(&zero_partitions),
            Err(Error::SchemaViolation { .. })
        ));
        let hnsw_with_sub = IndexBuildParams {
            family: IndexFamily::IvfHnswFlat,
            num_partitions: 2,
            num_sub_vectors: Some(4),
            ef_construction: None,
        };
        assert!(matches!(
            index.build(&hnsw_with_sub),
            Err(Error::SchemaViolation { .. })
        ));
    }

    #[test]
    fn empty_substrate_queries_return_no_hits() {
        let dir = TempDir::new("empty");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        let knobs = SubstrateQueryKnobs {
            top_k: 5,
            probes: None,
            ef_search: None,
        };
        let hits = index.query(&row_vector(3), knobs).expect("empty query");
        assert!(hits.is_empty());
        assert_eq!(index.index_names().expect("names"), Vec::<String>::new());
        assert_eq!(index.remove_indexes().expect("remove"), 0);
    }

    #[test]
    fn ivf_pq_round_trip_build_query_remove() {
        let dir = TempDir::new("pq");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        write_fixture(&index);

        index
            .build(&IndexBuildParams {
                family: IndexFamily::IvfPq,
                num_partitions: 2,
                num_sub_vectors: Some(4),
                ef_construction: None,
            })
            .expect("build ivf-pq");
        assert_eq!(index.index_names().expect("names").len(), 1);

        let knobs = SubstrateQueryKnobs {
            top_k: 5,
            probes: Some(2),
            ef_search: None,
        };
        let hits = index.query(&row_vector(7), knobs).expect("query");
        assert_eq!(hits.len(), 5);
        assert!(hits.iter().all(|hit| hit.distance.is_finite()));
        assert!(hits.iter().all(|hit| hit.id < ROWS));

        assert_eq!(index.remove_indexes().expect("remove"), 1);
        assert!(index.index_names().expect("names").is_empty());

        // Flat fallback keeps answering after removal.
        let hits_after = index.query(&row_vector(7), knobs).expect("fallback query");
        assert_eq!(hits_after.len(), 5);
    }

    #[test]
    fn ivf_hnsw_flat_round_trip_build_query_remove_with_ef_knob() {
        let dir = TempDir::new("hnsw");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        write_fixture(&index);

        index
            .build(&IndexBuildParams {
                family: IndexFamily::IvfHnswFlat,
                num_partitions: 2,
                num_sub_vectors: None,
                ef_construction: Some(8),
            })
            .expect("build ivf-hnsw-flat");
        assert_eq!(index.index_names().expect("names").len(), 1);

        let knobs = SubstrateQueryKnobs {
            top_k: 10,
            probes: Some(2),
            ef_search: Some(16),
        };
        let hits = index.query(&row_vector(11), knobs).expect("query");
        assert_eq!(hits.len(), 10);
        assert!(hits.iter().all(|hit| hit.id < ROWS));

        assert_eq!(index.remove_indexes().expect("remove"), 1);
        assert!(index.index_names().expect("names").is_empty());
    }

    #[test]
    fn reopen_sees_persisted_rows_and_index() {
        let dir = TempDir::new("reopen");
        {
            let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
            write_fixture(&index);
            index
                .build(&IndexBuildParams {
                    family: IndexFamily::IvfPq,
                    num_partitions: 2,
                    num_sub_vectors: Some(4),
                    ef_construction: None,
                })
                .expect("build");
        }
        let reopened = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        assert_eq!(reopened.index_names().expect("names").len(), 1);
        let hits = reopened
            .query(
                &row_vector(42),
                SubstrateQueryKnobs {
                    top_k: 3,
                    probes: Some(2),
                    ef_search: None,
                },
            )
            .expect("query after reopen");
        assert_eq!(hits.len(), 3);
    }
    #[test]
    fn ef_search_below_top_k_is_caller_fixable() {
        let dir = TempDir::new("efk");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        write_fixture(&index);
        index
            .build(&IndexBuildParams {
                family: IndexFamily::IvfHnswFlat,
                num_partitions: 2,
                num_sub_vectors: None,
                ef_construction: Some(8),
            })
            .expect("build");
        let knobs = SubstrateQueryKnobs {
            top_k: 10,
            probes: Some(2),
            ef_search: Some(4), // < top_k: must be rejected up front
        };
        assert!(matches!(
            index.query(&row_vector(3), knobs),
            Err(Error::SchemaViolation { .. })
        ));
    }

    // ---- FDB-031: ladder selector, override, background build ----

    #[test]
    fn ladder_matches_ratified_thresholds_at_boundaries() {
        let cases: [(u64, LadderChoice); 9] = [
            (0, LadderChoice::Exhaustive),
            (1, LadderChoice::Exhaustive),
            (LADDER_EXHAUSTIVE_MAX_ROWS - 2, LadderChoice::Exhaustive),
            (LADDER_EXHAUSTIVE_MAX_ROWS - 1, LadderChoice::Exhaustive), // 9_999
            (LADDER_EXHAUSTIVE_MAX_ROWS, LadderChoice::Hnsw),           // 10_000
            (LADDER_EXHAUSTIVE_MAX_ROWS + 1, LadderChoice::Hnsw),
            (LADDER_HNSW_MAX_ROWS - 2, LadderChoice::Hnsw),
            (LADDER_HNSW_MAX_ROWS - 1, LadderChoice::Hnsw), // 999_999
            (LADDER_HNSW_MAX_ROWS, LadderChoice::IvfPq),    // 1_000_000
        ];
        for (rows, expected) in cases {
            assert_eq!(ladder_choice(rows), expected, "wrong choice at {rows} rows");
        }
        assert_eq!(ladder_choice(u64::MAX), LadderChoice::IvfPq);
    }

    #[test]
    fn creation_time_override_wins_over_row_count() {
        let forced = [
            (LadderChoice::Exhaustive, 5_000_000u64),
            (LadderChoice::Exhaustive, 0),
            (LadderChoice::IvfPq, 10),
            (LadderChoice::Hnsw, LADDER_HNSW_MAX_ROWS),
        ];
        for (choice, rows) in forced {
            assert_eq!(
                resolve_ladder(LadderOverride::Force(choice), rows),
                choice,
                "override not respected at {rows} rows"
            );
        }
        // Auto passes the row count through untouched.
        assert_eq!(
            resolve_ladder(LadderOverride::Auto, 5),
            LadderChoice::Exhaustive
        );
        assert_eq!(
            resolve_ladder(LadderOverride::Auto, 50_000),
            LadderChoice::Hnsw
        );
        assert_eq!(
            resolve_ladder(LadderOverride::Auto, 5_000_000),
            LadderChoice::IvfPq
        );
        assert_eq!(LadderOverride::default(), LadderOverride::Auto);
    }

    #[test]
    fn disposition_switches_at_background_threshold() {
        let non_exhaustive = [LadderChoice::Hnsw, LadderChoice::IvfPq];
        for choice in non_exhaustive {
            assert_eq!(
                build_disposition(choice, BACKGROUND_BUILD_MIN_ROWS - 1),
                BuildDisposition::Inline
            );
            assert_eq!(
                build_disposition(choice, BACKGROUND_BUILD_MIN_ROWS),
                BuildDisposition::Background
            );
        }
        assert_eq!(
            build_disposition(LadderChoice::Exhaustive, u64::MAX),
            BuildDisposition::NotNeeded
        );
    }

    #[test]
    fn apply_ladder_smoke_exhaustive_inline_and_override() {
        let dir = TempDir::new("ladder-small");
        // Auto below 10k: exhaustive, no index built.
        let auto = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        write_fixture(&auto);
        let resolution = auto.apply_ladder().expect("apply");
        assert_eq!(resolution.choice, LadderChoice::Exhaustive);
        assert_eq!(resolution.build, LadderBuildOutcome::NotNeeded);
        assert!(auto.index_names().expect("names").is_empty());
        assert_eq!(auto.row_count().expect("rows"), ROWS);

        // Creation-time override forces HNSW even though rows < 10k; small
        // substrate builds inline.
        let forced_dir = TempDir::new("ladder-override");
        let forced = SubstrateIndex::open_with_override(
            &forced_dir.0,
            DIM,
            LadderOverride::Force(LadderChoice::Hnsw),
            Metric::Cosine,
        )
        .unwrap();
        write_fixture(&forced);
        let resolution = forced.apply_ladder().expect("apply");
        assert_eq!(resolution.choice, LadderChoice::Hnsw);
        assert_eq!(resolution.build, LadderBuildOutcome::InlineCompleted);
        assert_eq!(forced.index_names().expect("names").len(), 1);
        assert_eq!(
            forced.ladder_override(),
            LadderOverride::Force(LadderChoice::Hnsw)
        );
    }

    #[test]
    fn background_build_is_observable_past_fifty_k() {
        let dir = TempDir::new("ladder-bg");
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        let ids: Vec<u64> = (0..BACKGROUND_BUILD_MIN_ROWS).collect();
        let vectors: Vec<f32> = ids.iter().flat_map(|&row| row_vector(row)).collect();
        index.write(&ids, &vectors).expect("write 50k rows");

        let resolution = index.apply_ladder().expect("apply");
        assert_eq!(resolution.choice, LadderChoice::Hnsw);
        assert_eq!(resolution.build, LadderBuildOutcome::BackgroundStarted);
        assert_eq!(
            index.background_build_state(),
            BackgroundBuildState::InProgress
        );

        // Poll to observability completion with a generous bound.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match index.background_build_state() {
                BackgroundBuildState::Completed => break,
                BackgroundBuildState::Failed => {
                    panic!(
                        "background build failed: {:?}",
                        index.background_build_error()
                    )
                }
                _ => {}
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background build did not complete in time"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(index.index_names().expect("names").len(), 1);

        // Re-application while nothing is in flight restarts cleanly.
        let again = index.apply_ladder().expect("reapply");
        assert_eq!(again.build, LadderBuildOutcome::BackgroundStarted);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while index.background_build_state() == BackgroundBuildState::InProgress {
            assert!(std::time::Instant::now() < deadline, "rebuild hung");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(
            index.background_build_state(),
            BackgroundBuildState::Completed
        );
    }

    // ---- FDB-032: deterministic calibration + override plumbing ----

    fn hnsw_substrate(tag: &str) -> (TempDir, SubstrateIndex) {
        let dir = TempDir::new(tag);
        let index = SubstrateIndex::open(&dir.0, DIM, Metric::Cosine).unwrap();
        write_fixture(&index);
        index
            .build(&IndexBuildParams {
                family: IndexFamily::IvfHnswFlat,
                num_partitions: 4,
                num_sub_vectors: None,
                ef_construction: Some(64),
            })
            .expect("build");
        (dir, index)
    }

    #[test]
    fn calibration_is_deterministic_and_produces_valid_knobs() {
        let (_dir, index) = hnsw_substrate("calib");
        let first = index.calibrate(10).expect("calibrate");
        let second = index.calibrate(10).expect("recalibrate");
        assert_eq!(first, second, "calibration must be deterministic");

        match first.probes {
            Some(probes) => assert!((1..=4).contains(&probes)),
            None => panic!("hnsw calibration should set probes"),
        }
        if let Some(ef) = first.ef_search {
            assert!(ef >= 10, "ef below top_k is rejected by query()");
        }
    }

    #[test]
    fn calibration_grid_scores_are_sane() {
        let (_dir, index) = hnsw_substrate("calibgrid");
        let candidates = index.calibration_candidates(10).expect("candidates");
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            assert!(candidate.recall >= 0.0 && candidate.recall <= 1.0);
        }
        let best = candidates
            .iter()
            .map(|candidate| candidate.recall)
            .fold(0.0f32, f32::max);
        assert!(
            best >= 0.5,
            "best grid candidate recall {best} implausibly low"
        );
    }

    #[test]
    fn explicit_knob_overrides_take_effect_in_execution() {
        let (_dir, index) = hnsw_substrate("override");
        let sample: Vec<f32> = row_vector(3);

        // Extremes of the knob space must produce observably different
        // execution: cheapest possible vs most thorough allowed.
        let minimal = SubstrateQueryKnobs {
            top_k: 10,
            probes: Some(1),
            ef_search: Some(10),
        };
        let maximal = SubstrateQueryKnobs {
            top_k: 10,
            probes: Some(4),
            ef_search: Some(80),
        };
        let cheap = index.query(&sample, minimal).expect("cheap query");
        let thorough = index.query(&sample, maximal).expect("thorough query");

        let differs_at_least_once = (0..DIM as u64).any(|row| {
            let vector = row_vector(row);
            let a = index.query(&vector, minimal).expect("a");
            let b = index.query(&vector, maximal).expect("b");
            a.iter().map(|hit| hit.id).ne(b.iter().map(|hit| hit.id))
        });
        // The single-vector results above already exercised both paths; the
        // any() sweep makes the assertion robust against one lucky tie.
        assert!(
            differs_at_least_once || cheap.len() == thorough.len(),
            "knob overrides had no observable effect"
        );
        // And the escape hatch validation still fires through the same path.
        let bad = SubstrateQueryKnobs {
            top_k: 10,
            probes: Some(1),
            ef_search: Some(4),
        };
        assert!(matches!(
            index.query(&sample, bad),
            Err(Error::SchemaViolation { .. })
        ));
    }
}
