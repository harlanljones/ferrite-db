//! Ferrite DB benchmark harness (ROADMAP FDB-021).
//!
//! Measures p50/p99 per-call latency and recall@k of the public
//! [`search`](ferrite_db::search::search) entry over corpus fixtures produced
//! by `corpus-gen`, under a recorded environment contract (hardware,
//! caller concurrency, top-k, warmup, cache-state control) and emits one
//! machine-readable JSON report including ingest throughput and peak RSS.
//!
//! Hard SLO ceilings (ROADMAP §13: p50 ≤ 2 ms, p99 ≤ 8 ms, recall@10 ≥ 94%)
//! are wired to process failure: with ceiling enforcement enabled any
//! violation makes the CLI exit non-zero.
//!
//! Latency timing wraps ONLY the library call; the filtered-search oracle
//! (`corpus_gen::filtered_exact_top_k`) is recomputed outside the timed
//! region so oracle cost never contaminates measurements.

use std::path::PathBuf;
use std::time::Instant;

use corpus_gen::{LoadedCorpus, load};
use ferrite_db::search::{Predicate, SearchOptions, search};
use ferrite_db::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};
use ferrite_db::write_path::{InsertRecord, MetadataValue, WritePath};

/// Hard performance ceilings (ROADMAP §13). A run that violates any of them
/// fails the process when enforcement is enabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ceilings {
    pub p50_max_ms: f64,
    pub p99_max_ms: f64,
    pub recall_min: f64,
}

impl Default for Ceilings {
    fn default() -> Self {
        Self {
            p50_max_ms: 2.0,
            p99_max_ms: 8.0,
            recall_min: 0.94,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Directory containing corpus-gen fixtures (or SIFT1M files in sift mode).
    pub corpus_dir: PathBuf,
    pub top_k: u32,
    /// Queries run before timing to pre-warm caches; cold-start effects must
    /// not masquerade as steady-state p99 (cache-state control).
    pub warmup_queries: usize,
    pub measured_queries: usize,
    /// Target predicate selectivity tier: 1.0 (unfiltered), 0.1, 0.01, 0.001.
    pub selectivity: f64,
    /// Recorded caller-concurrency level of the environment spec.
    pub caller_concurrency: usize,
    pub ceilings: Ceilings,
    pub enforce_ceilings: bool,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub dataset_vectors: u64,
    pub dimension: u32,
    pub metric: String,
    pub top_k: u32,
    pub selectivity: f64,
    /// Number of category values in the filter predicate; 0 = unfiltered.
    pub predicate_categories: usize,
    pub warmup_queries: usize,
    pub measured_queries: usize,
    pub hardware: String,
    pub caller_concurrency: usize,
    pub available_parallelism: usize,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub mean_ms: f64,
    pub recall_at_k: f64,
    pub ingest_vectors: u64,
    pub ingest_seconds: f64,
    pub ingest_throughput_vps: f64,
    /// Peak resident set size in bytes; `None` when the platform does not
    /// expose it (recorded as JSON null).
    pub peak_rss_bytes: Option<u64>,
    pub ceilings: Ceilings,
    pub violations: Vec<String>,
}

impl Report {
    pub fn exit_code(&self, enforce: bool) -> i32 {
        if enforce && !self.violations.is_empty() {
            1
        } else {
            0
        }
    }

    /// Machine-readable JSON report (hand-serialized, dependency-free).
    pub fn to_json(&self) -> String {
        let rss = match self.peak_rss_bytes {
            Some(b) => b.to_string(),
            None => "null".to_string(),
        };
        let violations = self
            .violations
            .iter()
            .map(|v| format!("    {v:?}"))
            .collect::<Vec<_>>()
            .join(",\n");
        format!(
            "{{\n\
             \"format_version\": 1,\n\
             \"tool\": \"ferrite-harness\",\n\
             \"dataset\": {{\"num_vectors\": {}, \"dimension\": {}, \"metric\": \"{}\", \"top_k\": {}}},\n\
             \"environment\": {{\"hardware\": {:?}, \"caller_concurrency\": {}, \"available_parallelism\": {}}},\n\
             \"run\": {{\"warmup_queries\": {}, \"measured_queries\": {}, \"selectivity\": {:.6}, \"predicate_categories\": {}}},\n\
             \"latency_ms\": {{\"p50\": {:.6}, \"p99\": {:.6}, \"mean\": {:.6}}},\n\
             \"recall_at_k\": {:.6},\n\
             \"ingest\": {{\"vectors\": {}, \"seconds\": {:.6}, \"throughput_vectors_per_sec\": {:.3}}},\n\
             \"memory_bytes\": {{\"peak_rss\": {}}},\n\
             \"ceilings\": {{\"p50_max_ms\": {:.6}, \"p99_max_ms\": {:.6}, \"recall_min\": {:.6}}},\n\
             \"ceiling_violations\": [\n{}\n]\n\
             }}",
            self.dataset_vectors,
            self.dimension,
            self.metric,
            self.top_k,
            self.hardware,
            self.caller_concurrency,
            self.available_parallelism,
            self.warmup_queries,
            self.measured_queries,
            self.selectivity,
            self.predicate_categories,
            self.p50_ms,
            self.p99_ms,
            self.mean_ms,
            self.recall_at_k,
            self.ingest_vectors,
            self.ingest_seconds,
            self.ingest_throughput_vps,
            rss,
            self.ceilings.p50_max_ms,
            self.ceilings.p99_max_ms,
            self.ceilings.recall_min,
            violations,
        )
    }
}

type HResult<T> = Result<T, String>;

/// Runs one full measurement pass and produces the machine-readable report.
pub fn run(config: &HarnessConfig) -> HResult<Report> {
    let corpus = load(&config.corpus_dir)?;
    run_loaded(corpus, config)
}

/// Like [`run`], but against an already-loaded corpus (used by tests).
pub fn run_loaded(corpus: LoadedCorpus, config: &HarnessConfig) -> HResult<Report> {
    if config.measured_queries == 0 {
        return Err("measured_queries must be greater than zero".to_string());
    }
    if config.top_k == 0 || config.top_k > 1000 {
        return Err("top_k must be within 1..=1000".to_string());
    }
    let measured = config.measured_queries.min(corpus.num_queries());
    if measured == 0 {
        return Err("fixture contains no queries".to_string());
    }

    let metric = match corpus.metric {
        corpus_gen::Metric::Cosine => Metric::Cosine,
        corpus_gen::Metric::L2 => Metric::L2,
        corpus_gen::Metric::Dot => Metric::Dot,
    };

    // Filter predicate for the requested selectivity tier.
    let keep = keep_categories(config.selectivity, corpus.num_categories);
    if keep.is_some() && corpus.num_categories == 0 {
        return Err(
            "filtered runs need categorical fixtures; this corpus has no metadata".to_string(),
        );
    }
    let predicate = keep.as_ref().map(|vals| {
        Predicate::in_values(
            "category".to_string(),
            vals.iter().map(|&c| MetadataValue::I64(c)).collect(),
        )
    });

    // Ingest: build the searchable Delta from the fixture vectors. Timed so
    // the report carries ingest throughput alongside latency and recall.
    let schema = MetadataSchema::new(vec![MetadataColumn::new(
        "category".to_string(),
        ColumnType::I64,
    )])
    .map_err(|e| e.to_string())?;
    let table = TableManager::new()
        .create("bench".to_string(), corpus.dimension, metric, schema)
        .map_err(|e| e.to_string())?;
    let dim = corpus.dimension as usize;
    let num_vectors = corpus.corpus.len() / dim;

    let ingest_start = Instant::now();
    let mut path = WritePath::new(table);
    const BATCH: usize = 4096;
    let mut batch: Vec<InsertRecord> = Vec::with_capacity(BATCH);
    for row in 0..num_vectors {
        let category = corpus.categories.get(row).copied().unwrap_or(0);
        let mut metadata = std::collections::BTreeMap::new();
        metadata.insert("category".to_string(), MetadataValue::I64(category as i64));
        batch.push(InsertRecord::new(
            row as u64,
            corpus.corpus[row * dim..row * dim + dim].to_vec(),
            metadata,
        ));
        if batch.len() == BATCH {
            path.insert(std::mem::take(&mut batch))
                .map_err(|e| e.to_string())?;
        }
    }
    if !batch.is_empty() {
        path.insert(batch).map_err(|e| e.to_string())?;
    }
    let ingest_elapsed = ingest_start.elapsed();

    let delta = path.delta();
    let options_for = |top_k: u32| -> HResult<SearchOptions> {
        SearchOptions::new()
            .with_top_k(top_k)
            .map_err(|e| e.to_string())
    };

    // Warmup: identical call shape as the measured loop so instruction/data
    // caches reach steady state before any sample is taken.
    for q in 0..config.warmup_queries.min(measured) {
        let options = options_for(config.top_k)?;
        search(delta, corpus.query(q), predicate.as_ref(), options).map_err(|e| e.to_string())?;
    }

    // Measured loop: time only the library call; recompute the exact answer
    // outside the timed region for recall.
    let mut latencies_ms: Vec<f64> = Vec::with_capacity(measured);
    let mut recalls: Vec<f64> = Vec::with_capacity(measured);
    let k_usize = config.top_k as usize;
    for q in 0..measured {
        let query = corpus.query(q);
        let expected: Vec<u32> = match (&keep, corpus.num_categories) {
            (None, _) => corpus.ground_truth[q].indices.clone(),
            (Some(vals), _) => corpus_gen::filtered_exact_top_k(
                query,
                &corpus.corpus,
                &corpus.categories,
                dim,
                k_usize,
                corpus.metric,
                vals,
            ),
        };

        let options = options_for(config.top_k)?;
        let start = Instant::now();
        let results =
            search(delta, query, predicate.as_ref(), options).map_err(|e| e.to_string())?;
        latencies_ms.push(start.elapsed().as_secs_f64() * 1000.0);

        let hit = results
            .iter()
            .map(|r| r.id() as u32)
            .filter(|id| expected.contains(id))
            .count();
        if !expected.is_empty() {
            recalls.push(hit as f64 / expected.len() as f64);
        }
    }

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: usize| -> f64 {
        let idx = ((latencies_ms.len() - 1) * p / 100).min(latencies_ms.len() - 1);
        latencies_ms[idx]
    };
    let mean = latencies_ms.iter().sum::<f64>() / latencies_ms.len() as f64;
    let recall_at_k = if recalls.is_empty() {
        0.0
    } else {
        recalls.iter().sum::<f64>() / recalls.len() as f64
    };

    let ingest_vectors = num_vectors as u64;
    let ingest_seconds = ingest_elapsed.as_secs_f64();

    let mut report = Report {
        dataset_vectors: ingest_vectors,
        dimension: corpus.dimension,
        metric: format!("{:?}", corpus.metric),
        top_k: config.top_k,
        selectivity: config.selectivity,
        predicate_categories: keep.as_ref().map_or(0, Vec::len),
        warmup_queries: config.warmup_queries.min(measured),
        measured_queries: measured,
        hardware: hardware_description(),
        caller_concurrency: config.caller_concurrency,
        available_parallelism: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        p50_ms: pct(50),
        p99_ms: pct(99),
        mean_ms: mean,
        recall_at_k,
        ingest_vectors,
        ingest_seconds,
        ingest_throughput_vps: if ingest_seconds > 0.0 {
            ingest_vectors as f64 / ingest_seconds
        } else {
            0.0
        },
        peak_rss_bytes: peak_rss_bytes(),
        ceilings: config.ceilings,
        violations: Vec::new(),
    };

    if report.p50_ms > config.ceilings.p50_max_ms {
        report.violations.push(format!(
            "p50 {:.6} ms exceeds ceiling {:.6} ms",
            report.p50_ms, config.ceilings.p50_max_ms
        ));
    }
    if report.p99_ms > config.ceilings.p99_max_ms {
        report.violations.push(format!(
            "p99 {:.6} ms exceeds ceiling {:.6} ms",
            report.p99_ms, config.ceilings.p99_max_ms
        ));
    }
    if report.recall_at_k < config.ceilings.recall_min {
        report.violations.push(format!(
            "recall@{} {:.6} below minimum {:.6}",
            report.top_k, report.recall_at_k, config.ceilings.recall_min
        ));
    }

    Ok(report)
}

/// Category values whose combined share approximates the target selectivity
/// tier. `None` means unfiltered. With uniformly distributed categories,
/// filtering on k of C values yields selectivity ~k/C.
fn keep_categories(selectivity: f64, num_categories: u32) -> Option<Vec<i64>> {
    if selectivity >= 1.0 || num_categories == 0 {
        return None;
    }
    let k = ((selectivity * f64::from(num_categories)).round() as u32).max(1);
    Some((0..i64::from(k)).collect())
}

fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn hardware_description() -> String {
    std::process::Command::new("uname")
        .args(["-s", "-r", "-m"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| format!("{} {}", std::env::consts::OS, std::env::consts::ARCH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use corpus_gen::{CorpusConfig, Metric as CgMetric, generate, serialize, write_to_dir};

    fn fixture_dir(tag: &str) -> PathBuf {
        let cfg = CorpusConfig {
            num_vectors: 300,
            dimension: 16,
            num_queries: 12,
            top_k: 10,
            seed: 7,
            num_categories: 4,
            metric: CgMetric::Cosine,
        };
        let art = generate(&cfg).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "harness-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        write_to_dir(&dir, &serialize(&cfg, &art)).unwrap();
        dir
    }

    fn base_config(dir: &std::path::Path) -> HarnessConfig {
        HarnessConfig {
            corpus_dir: dir.to_path_buf(),
            top_k: 10,
            warmup_queries: 3,
            measured_queries: 12,
            selectivity: 1.0,
            caller_concurrency: 1,
            ceilings: Ceilings {
                p50_max_ms: f64::MAX,
                p99_max_ms: f64::MAX,
                recall_min: 0.0,
            },
            enforce_ceilings: false,
        }
    }

    #[test]
    fn dry_run_produces_machine_readable_report() {
        let dir = fixture_dir("dry");
        let report = run(&base_config(&dir)).expect("harness run");
        // Exhaustive Delta scan must reproduce the exact-search oracle.
        assert!(
            report.recall_at_k >= 0.95,
            "recall@k {} below sanity floor",
            report.recall_at_k
        );
        assert!(report.p50_ms > 0.0);
        assert!(report.p99_ms >= report.p50_ms);
        assert_eq!(report.dataset_vectors, 300);
        assert_eq!(report.measured_queries, 12);
        assert!(report.violations.is_empty());
        let json = report.to_json();
        for key in [
            "\"latency_ms\"",
            "\"p99\"",
            "\"recall_at_k\"",
            "\"ingest\"",
            "\"peak_rss\"",
            "\"ceilings\"",
            "\"hardware\"",
        ] {
            assert!(json.contains(key), "report JSON missing {key}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ceiling_violation_fails_the_process() {
        let dir = fixture_dir("ceilings");
        let mut cfg = base_config(&dir);
        // An impossibly tight latency ceiling must trip the violation path.
        cfg.ceilings.p50_max_ms = 0.0;
        cfg.enforce_ceilings = false;
        let lenient = run(&cfg).expect("run");
        assert!(!lenient.violations.is_empty(), "violation not detected");
        assert_eq!(lenient.exit_code(false), 0, "unenforced run exits zero");

        cfg.enforce_ceilings = true;
        let enforced = run(&cfg).expect("run");
        assert_eq!(
            enforced.exit_code(true),
            1,
            "enforced ceiling violation must fail the process"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filtered_run_matches_filtered_oracle() {
        let dir = fixture_dir("filtered");
        let mut cfg = base_config(&dir);
        cfg.selectivity = 0.5;
        cfg.warmup_queries = 0;
        let report = run(&cfg).expect("filtered run");
        assert_eq!(report.predicate_categories, 2);
        assert!(
            report.recall_at_k >= 0.95,
            "filtered recall@k {} below oracle agreement",
            report.recall_at_k
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
