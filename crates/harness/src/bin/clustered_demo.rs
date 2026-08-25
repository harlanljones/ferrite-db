//! FDB-024 evidence runner: clustered corpus re-measurement (G3 recall
//! re-evaluation per ADR 0009).
//!
//! Reads a clustered-corpus fixture (produced by `corpus-gen --mode clustered`),
//! loads it through the public Table / insert / search path via the FDB-021
//! harness infrastructure, and emits a machine-readable report containing
//! recall@10 vs the exact-search ground truth, plus p50/p99 latency for an
//! unfiltered top-k query set. This is the smallest credible end-to-end
//! measurement of the G3 recall question on the clustered fixture (per the
//! FDB-024 dispatch's "smallest credible demo" option).
//!
//! Why a dedicated binary instead of `--dataset clustered` on the main
//! harness CLI:
//! - FDB-021's CLI auto-detects via the `FRC1` uniform magic; for FDB-024 we
//!   keep that surface unchanged and add a focused binary that consumes the
//!   `FRC2` clustered magic through `corpus_gen::clustered::load`. Both paths
//!   funnel into the same `harness::run_loaded` measurement loop.
//! - The FDB-024 deliverable is the *evidence line* in ROADMAP §5 + a
//!   reproducible artifact; a one-shot binary captures both without
//!   complicating FDB-021's CLI.

use std::path::PathBuf;
use std::process::ExitCode;

use corpus_gen::clustered;
use harness::{Ceilings, HarnessConfig};

const USAGE: &str = "harness-clustered-demo — FDB-024 clustered-corpus evidence runner

USAGE:
  harness-clustered-demo --corpus-dir DIR [OPTIONS]

OPTIONS:
  --corpus-dir DIR      Clustered fixture directory (required)
  --top-k N             Neighbours per query          [default: fixture top_k]
  --warmup N            Pre-warm queries before timing [default: 50]
  --queries N           Measured queries               [default: all in fixture]
  --report PATH         Report path ('-' for stdout)  [default: fdb024-clustered-report.json]
  -h, --help";

struct Args {
    corpus_dir: PathBuf,
    top_k: Option<u32>,
    warmup: usize,
    queries: Option<usize>,
    report: String,
}

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args, String> {
    let mut corpus_dir = None;
    let mut top_k = None;
    let mut warmup = 50;
    let mut queries = None;
    let mut report = "fdb024-clustered-report.json".to_string();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--corpus-dir" => {
                corpus_dir = Some(PathBuf::from(
                    args.next().ok_or("--corpus-dir needs a value")?,
                ))
            }
            "--top-k" => {
                top_k = Some(
                    args.next()
                        .ok_or("--top-k needs a value")?
                        .parse()
                        .map_err(|_| "--top-k expects a positive integer".to_string())?,
                )
            }
            "--warmup" => {
                warmup = args
                    .next()
                    .ok_or("--warmup needs a value")?
                    .parse()
                    .map_err(|_| "--warmup expects a non-negative integer".to_string())?
            }
            "--queries" => {
                queries = Some(
                    args.next()
                        .ok_or("--queries needs a value")?
                        .parse()
                        .map_err(|_| "--queries expects a positive integer".to_string())?,
                )
            }
            "--report" => report = args.next().ok_or("--report needs a value")?,
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }

    Ok(Args {
        corpus_dir: corpus_dir.ok_or_else(|| format!("--corpus-dir is required\n\n{USAGE}"))?,
        top_k,
        warmup,
        queries,
        report,
    })
}

fn run() -> Result<i32, String> {
    let args = parse_args(std::env::args().skip(1))?;

    // FRC2 magic dispatch: the FDB-021 `load` reads the uniform magic; the
    // clustered loader reads the same byte layout but with the FR?2 family.
    let corpus = clustered::load(&args.corpus_dir)?;

    let top_k = match args.top_k {
        Some(k) => k,
        None if corpus.top_k > 0 => corpus.top_k,
        _ => 10,
    };

    let config = HarnessConfig {
        corpus_dir: args.corpus_dir.clone(),
        top_k,
        warmup_queries: args.warmup,
        measured_queries: args.queries.unwrap_or(corpus.num_queries()),
        selectivity: 1.0,
        caller_concurrency: 1,
        // FDB-024 records evidence only; the clustered corpus is not yet
        // tied to a contract scale, so the ROADMAP §13 hard ceilings
        // (p50 ≤ 2 ms, p99 ≤ 8 ms, recall ≥ 94%) are reported but not
        // enforced at exit. Enforcement comes with the G3 decision.
        ceilings: Ceilings {
            p50_max_ms: f64::MAX,
            p99_max_ms: f64::MAX,
            recall_min: 0.0,
        },
        enforce_ceilings: false,
    };

    let report = harness::run_loaded(corpus, &config)?;
    let json = report.to_json();
    if args.report == "-" {
        println!("{json}");
    } else {
        std::fs::write(&args.report, json.as_bytes())
            .map_err(|e| format!("writing {}: {e}", args.report))?;
        println!("wrote {}", args.report);
    }

    eprintln!(
        "fdb024 clustered: vectors={} dim={} metric={} top_k={} \
         p50={:.3}ms p99={:.3}ms recall@{}={:.4} ingest={:.0} vec/s rss={}",
        report.dataset_vectors,
        report.dimension,
        report.metric,
        report.top_k,
        report.p50_ms,
        report.p99_ms,
        report.top_k,
        report.recall_at_k,
        report.ingest_throughput_vps,
        report
            .peak_rss_bytes
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unknown".to_string()),
    );
    Ok(0)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
