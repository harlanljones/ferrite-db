//! FDB-070 SLO tuning sweep runner.
//!
//! Iterates the ladder/calibration knob space (index family × probes × ef)
//! over corpus-gen fixtures, measuring pooled-interleaved p50/p99 latency and
//! recall@k per configuration, and emits one machine-readable campaign JSON
//! per run. The binary is built once per allocator feature (system | mimalloc
//! | jemalloc); label the run accordingly via --allocator.

use std::path::PathBuf;
use std::time::Instant;

use corpus_gen::load;
use ferrite_db::index_substrate::{
    IndexBuildParams, IndexFamily, LadderChoice, LadderOverride, SubstrateIndex,
    SubstrateQueryKnobs,
};
use ferrite_db::table::Metric;

fn main() {
    if let Err(message) = run() {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

struct Args {
    corpus_dir: PathBuf,
    rows: usize,
    queries: usize,
    top_k: u32,
    allocator: String,
    out: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        corpus_dir: PathBuf::new(),
        rows: 100_000,
        queries: 200,
        top_k: 10,
        allocator: "system".to_string(),
        out: PathBuf::from("docs/baselines/artifacts/fdb070-sweep-system.json"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--corpus-dir" => {
                args.corpus_dir = PathBuf::from(it.next().ok_or("--corpus-dir needs a value")?)
            }
            "--rows" => {
                args.rows = it
                    .next()
                    .ok_or("--rows needs a value")?
                    .parse()
                    .map_err(|_| "--rows expects an integer")?
            }
            "--queries" => {
                args.queries = it
                    .next()
                    .ok_or("--queries needs a value")?
                    .parse()
                    .map_err(|_| "--queries expects an integer")?
            }
            "--top-k" => {
                args.top_k = it
                    .next()
                    .ok_or("--top-k needs a value")?
                    .parse()
                    .map_err(|_| "--top-k expects an integer")?
            }
            "--allocator" => args.allocator = it.next().ok_or("--allocator needs a value")?,
            "--out" => args.out = PathBuf::from(it.next().ok_or("--out needs a value")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let corpus = load(&args.corpus_dir).map_err(|e| e.to_string())?;
    let dim = corpus.dimension as usize;
    let rows = args.rows.min(corpus.corpus.len() / dim);
    let count = args.queries.min(corpus.num_queries());

    let work = tempfile_dir()?;
    let index = SubstrateIndex::open_with_override(
        &work,
        corpus.dimension,
        LadderOverride::Force(LadderChoice::Hnsw),
        Metric::Cosine,
    )
    .map_err(|e| e.to_string())?;
    let ids: Vec<u64> = (0..rows as u64).collect();
    index
        .write(&ids, &corpus.corpus[..rows * dim])
        .map_err(|e| e.to_string())?;

    // Build both families over identical data; sweep knobs under whichever is
    // currently registered, switching between them mid-campaign.
    let hnsw_params = IndexBuildParams {
        family: IndexFamily::IvfHnswFlat,
        num_partitions: 4,
        num_sub_vectors: None,
        ef_construction: Some(64),
    };
    let pq_params = IndexBuildParams {
        family: IndexFamily::IvfPq,
        num_partitions: 4,
        num_sub_vectors: Some(32),
        ef_construction: None,
    };

    let mut results: Vec<String> = Vec::new();
    for (family_label, params, grid) in [
        (
            "IvfHnswFlat(partitions=4,ef_c=64)",
            hnsw_params,
            vec![
                (1u32, Some(16u32)),
                (1, Some(64)),
                (1, Some(128)),
                (1, Some(256)),
                (2, Some(16)),
                (2, Some(64)),
                (2, Some(128)),
                (4, Some(16)),
                (4, Some(64)),
                (4, Some(256)),
            ],
        ),
        (
            "IvfPq(partitions=4,sub=32)",
            pq_params,
            vec![(1u32, None), (2, None), (4, None)],
        ),
    ] {
        index.build(&params).map_err(|e| e.to_string())?;
        for (probes, ef) in grid {
            let knobs = SubstrateQueryKnobs {
                top_k: args.top_k,
                probes: Some(probes),
                ef_search: ef,
            };
            // Warmup one pass, then two measured passes pooled.
            for q in 0..count {
                index
                    .query(corpus.query(q), knobs)
                    .map_err(|e| e.to_string())?;
            }
            let mut latencies = Vec::with_capacity(count * 2);
            let mut recalls = Vec::with_capacity(count * 2);
            for _pass in 0..2 {
                for q in 0..count {
                    let expected = &corpus.ground_truth[q].indices;
                    let start = Instant::now();
                    let hits = index
                        .query(corpus.query(q), knobs)
                        .map_err(|e| e.to_string())?;
                    latencies.push(start.elapsed().as_secs_f64() * 1000.0);
                    let overlap = hits
                        .iter()
                        .take(expected.len())
                        .filter(|hit| expected.contains(&(hit.id as u32)))
                        .count();
                    recalls.push(overlap as f64 / expected.len() as f64);
                }
            }
            latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let pct = |p: usize| latencies[(latencies.len() - 1) * p / 100];
            let recall = recalls.iter().sum::<f64>() / recalls.len() as f64;
            println!(
                "{family_label} probes={probes} ef={} p50={:.3} p99={:.3} recall={:.4}",
                ef.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                pct(50),
                pct(99),
                recall
            );
            results.push(format!(
                "{{\"family\": \"{family_label}\", \"knobs\": {}, \"p50_ms\": {:.3}, \
                 \"p99_ms\": {:.3}, \"recall_at_k\": {:.4}}}",
                knobs_json(args.top_k, Some(probes), ef),
                pct(50),
                pct(99),
                recall
            ));
        }
    }

    let json = format!(
        "{{\n\
         \"format_version\": 1,\n\
         \"tool\": \"fdb070-sweep\",\n\
         \"allocator\": {:?},\n\
         \"fixture\": {{\"corpus_dir\": {:?}, \"rows\": {}, \"dimension\": {}, \"queries_per_pass\": {}, \"passes\": 2, \"metric\": \"Cosine\"}},\n\
         \"configs\": [\n{}\n],\n\
         \"peak_rss_bytes\": {}\n\
         }}",
        args.allocator,
        args.corpus_dir.display(),
        rows,
        corpus.dimension,
        count,
        results.join(",\n"),
        peak_rss_bytes().unwrap_or(0),
    );
    std::fs::create_dir_all(args.out.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&args.out, json.as_bytes()).map_err(|e| e.to_string())?;
    println!("wrote {}", args.out.display());
    Ok(())
}

fn knobs_json(top_k: u32, probes: Option<u32>, ef: Option<u32>) -> String {
    format!(
        "{{\"top_k\": {}, \"probes\": {}, \"ef_search\": {}}}",
        top_k,
        probes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "null".into()),
        ef.map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
    )
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

fn tempfile_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("fdb070-sweep-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
