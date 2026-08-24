//! FDB-032 evidence runner: calibrated vs naive fixed default knobs.
//!
//! Builds one HNSW-backed substrate over corpus-gen fixtures, then measures
//! p50/p99 latency and recall@10 for both knob regimes on the identical
//! fixture and identical query set. Emits one machine-readable verdict JSON;
//! Pareto domination means calibrated recall@10 ≥ naive AND p50 ≤ naive AND
//! p99 ≤ naive.

use std::path::PathBuf;
use std::time::Instant;

use corpus_gen::load;
use ferrite_db::index_substrate::{
    IndexBuildParams, IndexFamily, LadderChoice, LadderOverride, SubstrateIndex,
    SubstrateQueryKnobs, naive_fixed_knobs,
};
use ferrite_db::table::Metric;

struct Args {
    corpus_dir: PathBuf,
    rows: usize,
    queries: usize,
    top_k: u32,
    out: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut corpus_dir = None;
    let mut rows = 100_000usize;
    let mut queries = 100usize;
    let mut top_k = 10u32;
    let mut out = PathBuf::from("docs/baselines/artifacts/fdb032-ann-compare.json");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus-dir" => {
                corpus_dir = Some(PathBuf::from(
                    args.next().ok_or("--corpus-dir needs a value")?,
                ))
            }
            "--rows" => {
                rows = args
                    .next()
                    .ok_or("--rows needs a value")?
                    .parse()
                    .map_err(|_| "--rows expects an integer")?
            }
            "--queries" => {
                queries = args
                    .next()
                    .ok_or("--queries needs a value")?
                    .parse()
                    .map_err(|_| "--queries expects an integer")?
            }
            "--top-k" => {
                top_k = args
                    .next()
                    .ok_or("--top-k needs a value")?
                    .parse()
                    .map_err(|_| "--top-k expects an integer")?
            }
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        corpus_dir: corpus_dir.ok_or("--corpus-dir is required")?,
        rows,
        queries,
        top_k,
        out,
    })
}

fn run_arm(
    index: &SubstrateIndex,
    corpus: &corpus_gen::LoadedCorpus,
    count: usize,
    knobs: SubstrateQueryKnobs,
    recalls_out: &mut Vec<f64>,
) -> Result<Vec<f64>, String> {
    let mut latencies = Vec::with_capacity(count);
    for q in 0..count {
        let query = corpus.query(q);
        let expected = &corpus.ground_truth[q].indices;

        let start = Instant::now();
        let hits = index.query(query, knobs).map_err(|e| e.to_string())?;
        latencies.push(start.elapsed().as_secs_f64() * 1000.0);

        let overlap = hits
            .iter()
            .take(expected.len())
            .filter(|hit| expected.contains(&(hit.id as u32)))
            .count();
        recalls_out.push(overlap as f64 / expected.len() as f64);
    }
    Ok(latencies)
}

fn summarize(latencies: &[f64], recalls: &[f64]) -> (f64, f64, f64, f64) {
    let mut sorted = latencies.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: usize| sorted[(sorted.len() - 1) * p / 100];
    (
        pct(50),
        pct(99),
        sorted.iter().sum::<f64>() / sorted.len() as f64,
        recalls.iter().sum::<f64>() / recalls.len() as f64,
    )
}

fn main() {
    let args = parse_args();
    let args = match args {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(2);
        }
    };
    if let Err(message) = run(args) {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), String> {
    let corpus = load(&args.corpus_dir).map_err(|e| e.to_string())?;
    assert_eq!(
        corpus.metric,
        corpus_gen::Metric::Cosine,
        "runner assumes Cosine fixtures"
    );
    let dim = corpus.dimension as usize;
    let rows = args.rows.min(corpus.corpus.len() / dim);
    let count = args.queries.min(corpus.num_queries());

    let work_dir = tempfile_dir()?;
    let index = SubstrateIndex::open_with_override(
        &work_dir,
        corpus.dimension,
        LadderOverride::Force(LadderChoice::Hnsw),
        Metric::Cosine,
    )
    .map_err(|e| e.to_string())?;

    let ids: Vec<u64> = (0..rows as u64).collect();
    let vectors: Vec<f32> = corpus.corpus[..rows * dim].to_vec();
    let ingest_start = Instant::now();
    index.write(&ids, &vectors).map_err(|e| e.to_string())?;
    println!(
        "ingested {} rows in {:.1}s",
        rows,
        ingest_start.elapsed().as_secs_f64()
    );

    index
        .build(&IndexBuildParams {
            family: IndexFamily::IvfHnswFlat,
            num_partitions: 4,
            num_sub_vectors: None,
            ef_construction: Some(64),
        })
        .map_err(|e| e.to_string())?;

    // Warmup with mid-grid knobs so both regimes time steady-state.
    let warm = SubstrateQueryKnobs {
        top_k: args.top_k,
        probes: Some(2),
        ef_search: Some(32),
    };
    for q in 0..count.min(10) {
        index
            .query(corpus.query(q), warm)
            .map_err(|e| e.to_string())?;
    }

    // Interleaved A/B passes over the full query set: drift (frequency
    // scaling, background load) hits both arms equally instead of whichever
    // ran second.
    const PASSES: usize = 5;
    let naive_knobs = naive_fixed_knobs(args.top_k);
    let calibrated_knobs = index.calibrate(args.top_k).map_err(|e| e.to_string())?;
    println!(
        "knob regimes: naive={:?} calibrated={:?}",
        naive_knobs, calibrated_knobs
    );

    let mut naive_recalls = Vec::new();
    let mut calib_recalls = Vec::new();
    let mut naive_latencies = Vec::new();
    let mut calib_latencies = Vec::new();
    for pass in 0..PASSES {
        let naive_lat = run_arm(&index, &corpus, count, naive_knobs, &mut naive_recalls)
            .map_err(|e| format!("pass {pass} naive: {e}"))?;
        naive_latencies.extend(naive_lat);
        let calib_lat = run_arm(&index, &corpus, count, calibrated_knobs, &mut calib_recalls)
            .map_err(|e| format!("pass {pass} calibrated: {e}"))?;
        calib_latencies.extend(calib_lat);
    }

    let (naive_p50, naive_p99, naive_mean, naive_recall) =
        summarize(&naive_latencies, &naive_recalls);
    let (calib_p50, calib_p99, calib_mean, calib_recall) =
        summarize(&calib_latencies, &calib_recalls);

    let pareto = calib_recall >= naive_recall && calib_p50 <= naive_p50 && calib_p99 <= naive_p99;

    let json = format!(
        "{{\n \
         \"format_version\": 1,\n \
         \"tool\": \"ann-compare (FDB-032)\",\n \
         \"method\": {{\"passes\": {}, \"queries_per_pass\": {}, \"interleaved\": true}},\n \
         \"fixture\": {{\"corpus_dir\": {:?}, \"rows\": {rows}, \"dimension\": {}, \"metric\": \"Cosine\", \"family\": \"IvfHnswFlat(partitions=4, ef_construction=64)\"}},\n \
         \"naive_fixed\": {{\"knobs\": {}, \"p50_ms\": {:.3}, \"p99_ms\": {:.3}, \"mean_ms\": {:.3}, \"recall_at_k\": {:.4}}},\n \
         \"calibrated\": {{\"knobs\": {}, \"p50_ms\": {:.3}, \"p99_ms\": {:.3}, \"mean_ms\": {:.3}, \"recall_at_k\": {:.4}}},\n \
         \"pareto_dominates\": {}\n\
         }}",
        PASSES,
        count,
        args.corpus_dir.display(),
        corpus.dimension,
        knobs_json(&naive_knobs),
        naive_p50,
        naive_p99,
        naive_mean,
        naive_recall,
        knobs_json(&calibrated_knobs),
        calib_p50,
        calib_p99,
        calib_mean,
        calib_recall,
        pareto,
    );
    std::fs::create_dir_all(args.out.parent().unwrap_or(&PathBuf::from(".")))
        .map_err(|e| e.to_string())?;
    std::fs::write(&args.out, json.as_bytes()).map_err(|e| e.to_string())?;

    println!(
        "naive      knobs={:?} p50={:.3}ms p99={:.3}ms recall@{}={:.4}",
        naive_knobs, naive_p50, naive_p99, args.top_k, naive_recall
    );
    println!(
        "calibrated knobs={:?} p50={:.3}ms p99={:.3}ms recall@{}={:.4}",
        calibrated_knobs, calib_p50, calib_p99, args.top_k, calib_recall
    );
    println!("wrote {}", args.out.display());
    println!("PARETO_DOMINATES={pareto}");
    if !pareto {
        std::process::exit(1);
    }
    Ok(())
}

fn knobs_json(knobs: &SubstrateQueryKnobs) -> String {
    format!(
        "{{\"top_k\": {}, \"probes\": {}, \"ef_search\": {}}}",
        knobs.top_k,
        knobs
            .probes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        knobs
            .ef_search
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
    )
}

fn tempfile_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("fdb032-ann-compare-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
