//! `corpus-gen` command line: generates the versioned benchmark fixtures
//! into a directory. Defaults follow the ADR 0006 contract (10M × 512-d).
//!
//! Example:
//! ```text
//! corpus-gen --out ./corpus --num-vectors 10000 --num-queries 1000
//! ```

use std::path::PathBuf;

use corpus_gen::{CorpusConfig, GenResult, Metric, clustered, serialize, write_to_dir};

fn main() {
    if let Err(e) = run() {
        eprintln!("corpus-gen: {e}");
        std::process::exit(1);
    }
}

fn run() -> GenResult<()> {
    let mut config = CorpusConfig::default();
    let mut clustered_cfg = clustered::ClusteredConfig::default();
    let mut out = PathBuf::from("corpus");
    let mut mode = Mode::Uniform;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let value = || -> GenResult<String> {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("missing value for {arg}"))
        };
        match arg.as_str() {
            "--mode" => {
                mode = match value()?.to_ascii_lowercase().as_str() {
                    "uniform" => Mode::Uniform,
                    "clustered" => Mode::Clustered,
                    other => {
                        return Err(format!(
                            "unknown mode: {other} (expected uniform|clustered)"
                        ));
                    }
                }
            }
            "--num-vectors" => {
                let v = parse::<u64>(&value()?, "--num-vectors")?;
                config.num_vectors = v;
                clustered_cfg.num_vectors = v;
            }
            "--dimension" => {
                let v = parse::<u32>(&value()?, "--dimension")?;
                config.dimension = v;
                clustered_cfg.dimension = v;
            }
            "--num-queries" => {
                let v = parse::<u64>(&value()?, "--num-queries")?;
                config.num_queries = v;
                clustered_cfg.num_queries = v;
            }
            "--top-k" => {
                let v = parse::<u32>(&value()?, "--top-k")?;
                config.top_k = v;
                clustered_cfg.top_k = v;
            }
            "--seed" => {
                let v = parse::<u64>(&value()?, "--seed")?;
                config.seed = v;
                clustered_cfg.seed = v;
            }
            "--num-categories" => {
                let v = parse::<u32>(&value()?, "--num-categories")?;
                config.num_categories = v;
                clustered_cfg.num_categories = v;
            }
            "--num-clusters" => {
                clustered_cfg.num_clusters = parse::<u32>(&value()?, "--num-clusters")?
            }
            "--cluster-stddev" => {
                clustered_cfg.cluster_stddev = parse::<f32>(&value()?, "--cluster-stddev")?
            }
            "--center-mean" => {
                clustered_cfg.center_mean = parse::<f32>(&value()?, "--center-mean")?
            }
            "--center-stddev" => {
                clustered_cfg.center_stddev = parse::<f32>(&value()?, "--center-stddev")?
            }
            "--metric" => {
                let m = parse_metric(&value()?)?;
                config.metric = m;
                clustered_cfg.metric = match m {
                    Metric::Cosine => clustered::Metric::Cosine,
                    Metric::L2 => clustered::Metric::L2,
                    Metric::Dot => clustered::Metric::Dot,
                };
            }
            "--out" => out = PathBuf::from(value()?),
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 2;
    }

    std::fs::create_dir_all(&out).map_err(|e| format!("creating {}: {e}", out.display()))?;

    match mode {
        Mode::Uniform => {
            let artifacts = corpus_gen::generate(&config)?;
            let serialized = serialize(&config, &artifacts);
            write_to_dir(&out, &serialized)?;
            eprintln!(
                "corpus-gen: wrote uniform fixtures to {} ({} vectors x {} dim, {} queries, top-{})",
                out.display(),
                config.num_vectors,
                config.dimension,
                config.num_queries,
                config.top_k,
            );
        }
        Mode::Clustered => {
            let artifacts = clustered::generate(&clustered_cfg)?;
            let serialized = clustered::serialize(&clustered_cfg, &artifacts);
            clustered::write_to_dir(&out, &serialized)?;
            eprintln!(
                "corpus-gen: wrote clustered fixtures to {} ({} vectors x {} dim, {} clusters sigma={}, {} queries, top-{})",
                out.display(),
                clustered_cfg.num_vectors,
                clustered_cfg.dimension,
                clustered_cfg.num_clusters,
                clustered_cfg.cluster_stddev,
                clustered_cfg.num_queries,
                clustered_cfg.top_k,
            );
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Mode {
    Uniform,
    Clustered,
}

fn parse<T: std::str::FromStr>(raw: &str, name: &str) -> GenResult<T> {
    raw.parse::<T>()
        .map_err(|_| format!("invalid value for {name}: {raw}"))
}

fn parse_metric(raw: &str) -> GenResult<Metric> {
    match raw.to_ascii_lowercase().as_str() {
        "cosine" => Ok(Metric::Cosine),
        "l2" => Ok(Metric::L2),
        "dot" => Ok(Metric::Dot),
        other => Err(format!("unknown metric: {other}")),
    }
}
