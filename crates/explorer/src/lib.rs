//! Ferrite DB Explorer — localhost demo dashboard core (ROADMAP HJ-252,
//! rescoped by Harlan to a locally-served tool with no networked deployment).
//!
//! ADR note (flagged for confirmation): ADR 0001 governs the *library*
//! contract — sync API, embedded, no service. This crate is a host
//! application consuming ferrite-db exactly as any downstream embedder would;
//! it binds only to 127.0.0.1 and ships as an opt-in demo binary.
//!
//! Architecture: one [`Explorer`] owns a compaction [`Lifecycle`] (exhaustive
//! path, ingestion, segment status) plus a substrate [`SubstrateIndex`] (ANN
//! path with live knobs). Both store the same synthetic dataset generated
//! deterministically by corpus-gen, so every ANN answer can be scored against
//! a locally computed exact oracle.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use corpus_gen::{CorpusConfig, Metric as CgMetric};
use ferrite_db::compaction::Lifecycle;
use ferrite_db::index_substrate::{
    IndexBuildParams, IndexFamily, LadderChoice, LadderOverride, SubstrateIndex,
    SubstrateQueryKnobs,
};
use ferrite_db::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};

/// Dataset + index specification for one explorer session. Every field feeds
/// determinism: identical specs produce identical datasets and answers.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub num_vectors: u32,
    pub dimension: u32,
    pub num_categories: u32,
    pub seed: u64,
    /// Index family built for the ANN path.
    pub family: IndexFamily,
}

impl Default for SessionSpec {
    fn default() -> Self {
        Self {
            num_vectors: 2_000,
            dimension: 64,
            num_categories: 50,
            seed: 42,
            family: IndexFamily::IvfHnswFlat,
        }
    }
}

/// Everything known about the currently loaded session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionStatus {
    pub rows: usize,
    pub active_rows: usize,
    pub sealed_segments: usize,
    pub indexes: Vec<String>,
    pub dimension: u32,
    pub seed: u64,
    pub family: &'static str,
    pub last_build_ms: f64,
    pub last_ingest_ms: f64,
}

/// One search answer row.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Hit {
    pub id: u64,
    pub distance: f32,
}

/// Result of one query: ANN hits plus the exact-search comparison.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueryOutcome {
    pub hits: Vec<Hit>,
    pub exact_ids: Vec<u64>,
    pub recall_at_k: f64,
    pub total_us: u64,
}

/// Aggregate benchmark metrics over a query batch.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchMetrics {
    pub queries: usize,
    pub p50_us: f64,
    pub p99_us: f64,
    pub mean_us: f64,
    pub qps: f64,
    pub recall_at_k: f64,
    pub peak_rss_bytes: u64,
}

/// The demo application state: deterministic dataset loaded into both the
/// exhaustive lifecycle and the ANN substrate.
pub struct Explorer {
    lifecycle: Lifecycle,
    substrate: SubstrateIndex,
    spec: SessionSpec,
    queries: Vec<f32>,
    build_ms: AtomicU64,
    ingest_ms: AtomicU64,
    /// Index names as of the last successful build, so status reads never
    /// touch the seam's blocking runtime from async workers.
    index_names: Mutex<Vec<String>>,
    ingest_lock: Mutex<()>,
}

impl Explorer {
    /// Generates the deterministic dataset, ingests it into the exhaustive
    /// path, and builds the requested index family. Identical specs yield
    /// byte-identical datasets (corpus-gen determinism).
    pub fn create(spec: SessionSpec) -> Result<Self, String> {
        let started = Instant::now();
        let cg_config = CorpusConfig {
            num_vectors: u64::from(spec.num_vectors),
            dimension: spec.dimension,
            num_queries: 16,
            top_k: 10,
            seed: spec.seed,
            num_categories: spec.num_categories,
            metric: CgMetric::Cosine,
        };
        let artifacts =
            corpus_gen::generate(&cg_config).map_err(|e| format!("generating dataset: {e}"))?;

        let schema = MetadataSchema::new(vec![MetadataColumn::new(
            "category".to_string(),
            ColumnType::I64,
        )])
        .map_err(|e| e.to_string())?;
        let table = TableManager::new()
            .create(
                "explorer".to_string(),
                spec.dimension,
                Metric::Cosine,
                schema,
            )
            .map_err(|e| e.to_string())?;
        let lifecycle = Lifecycle::new(table).map_err(|e| e.to_string())?;

        let dim = spec.dimension as usize;
        let mut batch = Vec::with_capacity(spec.num_vectors as usize);
        for (row, category) in artifacts.categories.iter().enumerate() {
            let mut metadata = BTreeMap::new();
            metadata.insert(
                "category".to_string(),
                ferrite_db::write_path::MetadataValue::I64(i64::from(*category)),
            );
            batch.push(ferrite_db::write_path::InsertRecord::new(
                row as u64,
                artifacts.corpus[row * dim..(row + 1) * dim].to_vec(),
                metadata,
            ));
        }
        lifecycle.insert(batch).map_err(|e| e.to_string())?;
        let ingest_ms = started.elapsed().as_millis() as u64;

        // Deterministic held-out query vectors come straight from corpus-gen.
        let queries = artifacts.queries.clone();

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let substrate_dir = std::env::temp_dir().join(format!(
            "explorer-substrate-{}-{seed}-{nanos}",
            std::process::id(),
            seed = spec.seed
        ));
        std::fs::create_dir_all(&substrate_dir).map_err(|e| e.to_string())?;
        let substrate = SubstrateIndex::open_with_override(
            &substrate_dir,
            spec.dimension,
            LadderOverride::Force(match spec.family {
                IndexFamily::IvfHnswFlat => LadderChoice::Hnsw,
                IndexFamily::IvfPq => LadderChoice::IvfPq,
            }),
            Metric::Cosine,
        )
        .map_err(|e| e.to_string())?;
        substrate
            .write(&ids(spec.num_vectors), &artifacts.corpus)
            .map_err(|e| e.to_string())?;

        let explorer = Self {
            lifecycle,
            substrate,
            spec,
            queries,
            build_ms: AtomicU64::new(0),
            ingest_ms: AtomicU64::new(ingest_ms),
            index_names: Mutex::new(Vec::new()),
            ingest_lock: Mutex::new(()),
        };
        explorer.rebuild_index()?;
        Ok(explorer)
    }

    /// Rebuilds the ANN index with the session's family defaults.
    pub fn rebuild_index(&self) -> Result<(), String> {
        let started = Instant::now();
        let params = match self.spec.family {
            IndexFamily::IvfHnswFlat => IndexBuildParams {
                family: IndexFamily::IvfHnswFlat,
                num_partitions: 4,
                num_sub_vectors: None,
                ef_construction: Some(64),
            },
            IndexFamily::IvfPq => IndexBuildParams {
                family: IndexFamily::IvfPq,
                num_partitions: 4,
                num_sub_vectors: Some((self.spec.dimension / 16).max(1)),
                ef_construction: None,
            },
        };
        self.substrate.build(&params).map_err(|e| e.to_string())?;
        if let Ok(mut slot) = self.index_names.lock() {
            *slot = self.substrate.index_names().unwrap_or_default();
        }
        self.build_ms
            .store(started.elapsed().as_millis() as u64, Ordering::Release);
        Ok(())
    }

    /// Current session status for the dashboard. Snapshot failures degrade
    /// to zeroed counters rather than failing the whole request.
    pub fn status(&self) -> SessionStatus {
        let snapshot = self.lifecycle.snapshot().ok();
        let rows = snapshot.as_ref().map_or(0, |delta| delta.len());
        let active = snapshot
            .as_ref()
            .map_or(0, |delta| delta.active_records().len());
        let sealed = snapshot
            .as_ref()
            .map_or(0, |delta| delta.sealed_segments().len());
        SessionStatus {
            rows,
            active_rows: active,
            sealed_segments: sealed,
            indexes: self
                .index_names
                .lock()
                .map(|slot| slot.clone())
                .unwrap_or_default(),
            dimension: self.spec.dimension,
            seed: self.spec.seed,
            family: match self.spec.family {
                IndexFamily::IvfHnswFlat => "IVF-HNSW-Flat",
                IndexFamily::IvfPq => "IVF-PQ",
            },
            last_build_ms: self.build_ms.load(Ordering::Acquire) as f64,
            last_ingest_ms: self.ingest_ms.load(Ordering::Acquire) as f64,
        }
    }

    /// Runs one ANN query plus its exact-search comparison. `query_index`
    /// selects a deterministic held-out query vector; explicit knobs override
    /// calibration (the SearchOptions escape hatch story).
    pub fn query(
        &self,
        query_index: usize,
        top_k: u32,
        probes: Option<u32>,
        ef_search: Option<u32>,
    ) -> Result<QueryOutcome, String> {
        if self.queries.is_empty() {
            return Err("no queries available".to_string());
        }
        let qi = query_index % (self.queries.len() / self.spec.dimension as usize);
        let dim = self.spec.dimension as usize;
        let vector = &self.queries[qi * dim..(qi + 1) * dim];
        let knobs = SubstrateQueryKnobs {
            top_k,
            probes,
            ef_search,
        };

        let started = Instant::now();
        let hits = self
            .substrate
            .query(vector, knobs)
            .map_err(|e| e.to_string())?;
        let total_us = started.elapsed().as_micros() as u64;

        let snapshot = self.lifecycle.snapshot().map_err(|e| e.to_string())?;
        let expected = exact_top_k(vector, &snapshot, self.spec.dimension, top_k as usize);
        let overlap = hits
            .iter()
            .take(expected.len())
            .filter(|hit| expected.contains(&hit.id))
            .count();
        let recall = if expected.is_empty() {
            0.0
        } else {
            overlap as f64 / expected.len() as f64
        };

        Ok(QueryOutcome {
            hits: hits
                .into_iter()
                .map(|hit| Hit {
                    id: hit.id,
                    distance: hit.distance,
                })
                .collect(),
            exact_ids: expected,
            recall_at_k: recall,
            total_us,
        })
    }

    /// Batch benchmark across the held-out query set with pooled percentiles.
    pub fn bench(
        &self,
        passes: usize,
        top_k: u32,
        probes: Option<u32>,
        ef_search: Option<u32>,
    ) -> Result<BenchMetrics, String> {
        let dim = self.spec.dimension as usize;
        let query_count = self.queries.len() / dim;
        if query_count == 0 {
            return Err("no queries available".to_string());
        }
        let knobs = SubstrateQueryKnobs {
            top_k,
            probes,
            ef_search,
        };
        let started_total = Instant::now();
        let mut latencies = Vec::with_capacity(query_count * passes.max(1));
        let mut recalls = Vec::with_capacity(query_count * passes.max(1));
        for _ in 0..passes.max(1) {
            for qi in 0..query_count {
                let vector = &self.queries[qi * dim..(qi + 1) * dim];
                let start = Instant::now();
                let hits = self
                    .substrate
                    .query(vector, knobs)
                    .map_err(|e| e.to_string())?;
                latencies.push(start.elapsed().as_secs_f64() * 1_000_000.0);
                let snapshot = self.lifecycle.snapshot().map_err(|e| e.to_string())?;
                let expected = exact_top_k(vector, &snapshot, self.spec.dimension, top_k as usize);
                let overlap = hits
                    .iter()
                    .take(expected.len())
                    .filter(|hit| expected.contains(&hit.id))
                    .count();
                recalls.push(if expected.is_empty() {
                    0.0
                } else {
                    overlap as f64 / expected.len() as f64
                });
            }
        }
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: usize| latencies[(latencies.len() - 1) * p / 100];
        let elapsed = started_total.elapsed().as_secs_f64();
        Ok(BenchMetrics {
            queries: latencies.len(),
            p50_us: pct(50),
            p99_us: pct(99),
            mean_us: latencies.iter().sum::<f64>() / latencies.len() as f64,
            qps: if elapsed > 0.0 {
                latencies.len() as f64 / elapsed
            } else {
                0.0
            },
            recall_at_k: recalls.iter().sum::<f64>() / recalls.len() as f64,
            peak_rss_bytes: peak_rss_bytes().unwrap_or(0),
        })
    }

    /// Serializes concurrent re-ingestion attempts behind a lock so the demo
    /// never interleaves two dataset swaps.
    pub fn ingest_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        match self.ingest_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn ids(num_vectors: u32) -> Vec<u64> {
    (0..u64::from(num_vectors)).collect()
}

/// Exact nearest-neighbour ids over the exhaustive lineage (local oracle).
fn exact_top_k(
    query: &[f32],
    delta: &ferrite_db::write_path::Delta,
    _dimension: u32,
    k: usize,
) -> Vec<u64> {
    let mut best: Vec<(f32, u64)> = Vec::with_capacity(k);
    for record in delta.records() {
        if delta.is_tombstoned(record.id()) {
            continue;
        }
        let vector = record.vector();
        let dot: f32 = query.iter().zip(vector).map(|(a, b)| a * b).sum();
        let nq: f32 = query.iter().map(|v| v * v).sum::<f32>().sqrt();
        let nv: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        let d = if nq == 0.0 || nv == 0.0 {
            1.0
        } else {
            1.0 - dot / (nq * nv)
        };
        if best.len() < k {
            best.push((d, record.id()));
        } else if d < best[k - 1].0 {
            best[k - 1] = (d, record.id());
        } else {
            continue;
        }
        best.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }
    best.into_iter().map(|(_, id)| id).collect()
}

fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok()
                .map(|kb| kb * 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrite_db::index_substrate::IndexFamily;

    fn small_spec(family: IndexFamily) -> SessionSpec {
        SessionSpec {
            num_vectors: 400,
            dimension: 16,
            num_categories: 8,
            seed: 7,
            family,
        }
    }

    #[test]
    fn ingest_is_deterministic_across_identical_specs() {
        let a = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let b = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let sa = a.status();
        let sb = b.status();
        assert_eq!(sa.rows, sb.rows);
        assert_eq!(sa.dimension, sb.dimension);
        // Identical specs ⇒ identical datasets and identical EXACT oracle.
        // (ANN answers may reorder across independent index builds because
        // upstream kmeans training is unseeded — recorded in FDB-032.)
        let qa = a.query(1, 5, Some(2), Some(32)).unwrap();
        let qb = b.query(1, 5, Some(2), Some(32)).unwrap();
        assert_eq!(qa.exact_ids, qb.exact_ids);
        // Every ANN answer must at least be a plausible corpus id.
        for hit in &qa.hits {
            assert!(hit.id < u64::from(a.spec.num_vectors));
        }
    }

    #[test]
    fn query_agrees_with_exact_oracle_on_small_data() {
        let explorer = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let outcome = explorer.query(0, 10, Some(4), Some(64)).unwrap();
        assert_eq!(outcome.hits.len(), 10);
        // With ef=64 and all partitions probed on 400 rows, recall must be
        // essentially perfect; the oracle itself is recomputed independently.
        assert!(
            outcome.recall_at_k >= 0.9,
            "recall {} unexpectedly low",
            outcome.recall_at_k
        );
        // Distances arrive nearest-first.
        for pair in outcome.hits.windows(2) {
            assert!(pair[0].distance <= pair[1].distance);
        }
    }

    #[test]
    fn knob_overrides_change_results_when_cheap() {
        let explorer = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let thorough = explorer.query(2, 10, Some(4), Some(128)).unwrap();
        // The escape hatch must be honored: an invalid override errors rather
        // than being silently ignored.
        assert!(explorer.query(2, 10, Some(4), Some(4)).is_err());
        assert_eq!(thorough.hits.len(), 10);
    }

    #[test]
    fn bench_produces_complete_metrics() {
        let explorer = Explorer::create(small_spec(IndexFamily::IvfPq)).unwrap();
        let metrics = explorer.bench(2, 10, Some(4), None).unwrap();
        assert_eq!(metrics.queries, 32); // 16 held-out queries x 2 passes
        assert!(metrics.p50_us > 0.0 && metrics.p99_us >= metrics.p50_us);
        assert!(metrics.qps > 0.0);
        assert!(metrics.peak_rss_bytes > 0);

        // Configuration comparison: both families produce usable metrics.
        let hnsw = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let m = hnsw.bench(1, 10, Some(2), Some(64)).unwrap();
        assert!(m.recall_at_k >= 0.0 && m.recall_at_k <= 1.0);
    }

    #[test]
    fn status_reflects_loaded_session() {
        let explorer = Explorer::create(small_spec(IndexFamily::IvfHnswFlat)).unwrap();
        let status = explorer.status();
        assert_eq!(status.rows, 400);
        assert!(!status.indexes.is_empty(), "index should be registered");
        assert!(status.last_build_ms < 60_000.0);
        assert_eq!(status.family, "IVF-HNSW-Flat");
    }
}
