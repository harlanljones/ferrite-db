//! Ferrite DB benchmark harness CLI (ROADMAP FDB-021).
//!
//! Produces one full machine-readable report per run and, with ceiling
//! enforcement enabled (`--enforce-ceilings`), fails the process (non-zero
//! exit) on any ROADMAP §13 ceiling violation.

use std::path::PathBuf;
use std::process::ExitCode;

use corpus_gen::{load, load_sift};
use harness::{Ceilings, HarnessConfig};

const USAGE: &str = "ferrite-harness — Ferrite DB benchmark harness (FDB-021)

USAGE:
  ferrite-harness --corpus-dir DIR [OPTIONS]

OPTIONS:
  --corpus-dir DIR      Fixture directory (required). synthetic: corpus-gen
                        fixtures; sift: base.fvecs/query.fvecs/groundtruth.ivecs
  --dataset MODE        synthetic | sift            [default: synthetic]
  --top-k N             Neighbours per query          [default: fixture top_k]
  --warmup N            Pre-warm queries before timing [default: 100]
  --queries N           Measured queries               [default: all in fixture]
  --selectivity F       Filter tier 1.0|0.1|0.01|0.001 [default: 1.0]
  --concurrency N       Recorded caller-concurrency    [default: 1]
  --p50-max-ms F        p50 latency ceiling            [default: 2.0]
  --p99-max-ms F        p99 latency ceiling            [default: 8.0]
  --recall-min F        recall@k floor                 [default: 0.94]
  --enforce-ceilings    Exit non-zero on any ceiling violation
  --report PATH         Report path ('-' for stdout only) [default: report.json]
  -h, --help";

struct Args {
    corpus_dir: PathBuf,
    dataset: String,
    top_k: Option<u32>,
    warmup: usize,
    queries: Option<usize>,
    selectivity: f64,
    concurrency: usize,
    ceilings: Ceilings,
    enforce_ceilings: bool,
    report: String,
}

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args, String> {
    let mut corpus_dir = None;
    let mut dataset = "synthetic".to_string();
    let mut top_k = None;
    let mut warmup = 100;
    let mut queries = None;
    let mut selectivity = 1.0;
    let mut concurrency = 1;
    let mut ceilings = Ceilings::default();
    let mut enforce_ceilings = false;
    let mut report = "report.json".to_string();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--corpus-dir" => {
                corpus_dir = Some(PathBuf::from(
                    args.next().ok_or("--corpus-dir needs a value")?,
                ))
            }
            "--dataset" => dataset = args.next().ok_or("--dataset needs a value")?,
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
            "--selectivity" => {
                selectivity = args
                    .next()
                    .ok_or("--selectivity needs a value")?
                    .parse()
                    .map_err(|_| "--selectivity expects a float in (0, 1]".to_string())?;
                if !(selectivity > 0.0 && selectivity <= 1.0) {
                    return Err("--selectivity must be within (0, 1]".to_string());
                }
            }
            "--concurrency" => {
                concurrency = args
                    .next()
                    .ok_or("--concurrency needs a value")?
                    .parse()
                    .map_err(|_| "--concurrency expects a positive integer".to_string())?
            }
            "--p50-max-ms" => {
                ceilings.p50_max_ms = args
                    .next()
                    .ok_or("--p50-max-ms needs a value")?
                    .parse()
                    .map_err(|_| "--p50-max-ms expects a float".to_string())?
            }
            "--p99-max-ms" => {
                ceilings.p99_max_ms = args
                    .next()
                    .ok_or("--p99-max-ms needs a value")?
                    .parse()
                    .map_err(|_| "--p99-max-ms expects a float".to_string())?
            }
            "--recall-min" => {
                ceilings.recall_min = args
                    .next()
                    .ok_or("--recall-min needs a value")?
                    .parse()
                    .map_err(|_| "--recall-min expects a float".to_string())?
            }
            "--enforce-ceilings" => enforce_ceilings = true,
            "--report" => report = args.next().ok_or("--report needs a value")?,
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }

    Ok(Args {
        corpus_dir: corpus_dir.ok_or_else(|| format!("--corpus-dir is required\n\n{USAGE}"))?,
        dataset,
        top_k,
        warmup,
        queries,
        selectivity,
        concurrency,
        ceilings,
        enforce_ceilings,
        report,
    })
}

fn run() -> Result<i32, String> {
    let args = parse_args(std::env::args().skip(1))?;

    let corpus = match args.dataset.as_str() {
        "synthetic" => load(&args.corpus_dir)?,
        "sift" => load_sift(&args.corpus_dir)?,
        other => {
            return Err(format!(
                "unknown dataset mode {other:?} (expected synthetic | sift)\n\n{}",
                USAGE
            ));
        }
    };

    let config = HarnessConfig {
        corpus_dir: args.corpus_dir.clone(),
        top_k: match args.top_k {
            Some(k) => k,
            None if corpus.top_k > 0 => corpus.top_k,
            _ => 10,
        },
        warmup_queries: args.warmup,
        measured_queries: args.queries.unwrap_or(corpus.num_queries()),
        selectivity: args.selectivity,
        caller_concurrency: args.concurrency,
        ceilings: args.ceilings,
        enforce_ceilings: args.enforce_ceilings,
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
        "vectors={} dim={} metric={} top_k={} selectivity={:.4} \
         p50={:.3}ms p99={:.3}ms recall@{:.0}={:.4} ingest={:.0} vec/s rss={}",
        report.dataset_vectors,
        report.dimension,
        report.metric,
        report.top_k,
        report.selectivity,
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
    for violation in &report.violations {
        eprintln!("CEILING VIOLATION: {violation}");
    }
    Ok(report.exit_code(args.enforce_ceilings))
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
