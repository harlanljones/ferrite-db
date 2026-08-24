//! `corpus-gen` command line: generates the versioned benchmark fixtures
//! into a directory. Defaults follow the ADR 0006 contract (10M × 512-d).
//!
//! Example:
//! ```text
//! corpus-gen --out ./corpus --num-vectors 10000 --num-queries 1000
//! ```

use std::path::PathBuf;

use corpus_gen::{CorpusConfig, GenResult, Metric, serialize, write_to_dir};

fn main() {
    if let Err(e) = run() {
        eprintln!("corpus-gen: {e}");
        std::process::exit(1);
    }
}

fn run() -> GenResult<()> {
    let mut config = CorpusConfig::default();
    let mut out = PathBuf::from("corpus");

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
            "--num-vectors" => config.num_vectors = parse::<u64>(&value()?, "--num-vectors")?,
            "--dimension" => config.dimension = parse::<u32>(&value()?, "--dimension")?,
            "--num-queries" => config.num_queries = parse::<u64>(&value()?, "--num-queries")?,
            "--top-k" => config.top_k = parse::<u32>(&value()?, "--top-k")?,
            "--seed" => config.seed = parse::<u64>(&value()?, "--seed")?,
            "--num-categories" => {
                config.num_categories = parse::<u32>(&value()?, "--num-categories")?
            }
            "--metric" => config.metric = parse_metric(&value()?)?,
            "--out" => out = PathBuf::from(value()?),
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 2;
    }

    let artifacts = corpus_gen::generate(&config)?;
    let serialized = serialize(&config, &artifacts);

    std::fs::create_dir_all(&out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    write_to_dir(&out, &serialized)?;

    eprintln!(
        "corpus-gen: wrote fixtures to {} ({} vectors x {} dim, {} queries, top-{})",
        out.display(),
        config.num_vectors,
        config.dimension,
        config.num_queries,
        config.top_k,
    );
    Ok(())
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
