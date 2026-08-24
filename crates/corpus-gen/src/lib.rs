//! Reproducible synthetic corpus generator for the Ferrite DB benchmark
//! contract (ADR 0006): a 10M × 512-d f32 corpus, exact-search ground truth,
//! and a versioned query-set specification.
//!
//! Determinism is the central property (FDB-020 exit criterion: byte-identical
//! regeneration). Every f32 is derived from a seeded integer PRNG via exact
//! integer→float conversion, so generated fixtures are bit-for-bit identical
//! across runs and platforms. No floating-point rounding enters the path.
//!
//! Owned exclusively by ROADMAP FDB-020 (corpus tooling directory).

use std::io::Write;

/// The distance function the ground truth is computed against. Mirrors the
/// Table [`Metric`](ferrite_db::table::Metric) vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Cosine,
    L2,
    Dot,
}

impl Metric {
    fn distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Metric::Cosine => {
                let (dot, na, nb) = dot_and_norms(a, b);
                if na == 0.0 || nb == 0.0 {
                    f32::INFINITY
                } else {
                    1.0 - dot / (na * nb)
                }
            }
            Metric::L2 => {
                let (dot, na, nb) = dot_and_norms(a, b);
                na + nb - 2.0 * dot
            }
            Metric::Dot => {
                let (dot, _, _) = dot_and_norms(a, b);
                -dot
            }
        }
    }
}

fn dot_and_norms(a: &[f32], b: &[f32]) -> (f32, f32, f32) {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    (dot, na, nb)
}

/// Configuration for one corpus generation. All fields are part of the
/// versioned fixture identity: changing any of them changes the output bytes.
#[derive(Debug, Clone, Copy)]
pub struct CorpusConfig {
    /// Rows in the corpus (benchmark contract target: 10_000_000).
    pub num_vectors: u64,
    /// Dimensionality (benchmark contract: 512).
    pub dimension: u32,
    /// Held-out query vectors whose ground truth is computed.
    pub num_queries: u64,
    /// Nearest neighbours retained per query.
    pub top_k: u32,
    /// Master seed; all PRNG streams are derived deterministically from it.
    pub seed: u64,
    /// Distinct categorical metadata values per vector, used to construct
    /// predicate selectivities (see `selectivity_tiers`).
    pub num_categories: u32,
    /// Distance function for ground truth.
    pub metric: Metric,
}

impl Default for CorpusConfig {
    fn default() -> Self {
        Self {
            num_vectors: 10_000_000,
            dimension: 512,
            num_queries: 10_000,
            top_k: 10,
            seed: 0x9E37_79B9_7F4A_7C15,
            num_categories: 1000,
            metric: Metric::Cosine,
        }
    }
}

/// The exact nearest neighbours for a single query.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundTruth {
    /// Indices of the `top_k` nearest corpus rows, nearest first.
    pub indices: Vec<u32>,
    /// Corresponding distances, ascending.
    pub distances: Vec<f32>,
}

/// Generated, in-memory corpus artifacts.
#[derive(Debug, Clone)]
pub struct Artifacts {
    pub corpus: Vec<f32>,
    pub categories: Vec<u16>,
    pub queries: Vec<f32>,
    pub ground_truth: Vec<GroundTruth>,
}

/// Serialized fixture bytes plus the machine-readable manifest.
#[derive(Debug, Clone)]
pub struct Serialized {
    pub corpus_bytes: Vec<u8>,
    pub meta_bytes: Vec<u8>,
    pub queries_bytes: Vec<u8>,
    pub ground_truth_bytes: Vec<u8>,
    pub manifest: String,
}

pub type GenResult<T> = Result<T, String>;

/// A deterministic SplitMix64 stream. Pure integer arithmetic → fully
/// reproducible f32 values via exact integer→float conversion.
struct Prng(u64);

impl Prng {
    fn new(seed: u64) -> Self {
        Prng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next f32 uniformly in [0, 1) using the top 24 mantissa bits; the value
    /// is exact and platform-independent.
    fn next_f32(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32; // 24-bit significand
        (bits as f32) / (1u32 << 24) as f32
    }

    fn next_category(&mut self, num_categories: u32) -> u16 {
        (self.next_u64() % num_categories as u64) as u16
    }
}

/// Deterministically derives an independent sub-stream seed from the master
/// seed and a stream identifier.
fn sub_seed(master: u64, stream: u64) -> u64 {
    let mut p = Prng::new(master ^ 0x1234_5678_9ABC_DEF0);
    // advance a few rounds mixing in the stream id
    let _ = p.next_u64();
    p.next_u64() ^ stream.wrapping_mul(0x85EB_CA6B_9487_E9C1)
}

/// Generates the corpus, per-vector categories, query set, and exact
/// ground truth. Pure function of [`CorpusConfig`].
pub fn generate(config: &CorpusConfig) -> GenResult<Artifacts> {
    if config.dimension == 0 {
        return Err("dimension must be greater than zero".to_string());
    }
    if config.top_k == 0 {
        return Err("top_k must be greater than zero".to_string());
    }
    if config.num_vectors == 0 || config.num_queries == 0 {
        return Err("num_vectors and num_queries must be greater than zero".to_string());
    }

    let dim = config.dimension as usize;
    let n = config.num_vectors as usize;
    let q = config.num_queries as usize;

    // Corpus vectors (independent stream).
    let mut corpus_prng = Prng::new(sub_seed(config.seed, 1));
    let mut corpus = Vec::with_capacity(n * dim);
    for _ in 0..n * dim {
        corpus.push(corpus_prng.next_f32());
    }

    // Per-vector categories (independent stream).
    let mut cat_prng = Prng::new(sub_seed(config.seed, 2));
    let categories: Vec<u16> = (0..n)
        .map(|_| cat_prng.next_category(config.num_categories))
        .collect();

    // Query vectors (independent stream, distinct seed space).
    let mut query_prng = Prng::new(sub_seed(config.seed, 3));
    let mut queries = Vec::with_capacity(q * dim);
    for _ in 0..q * dim {
        queries.push(query_prng.next_f32());
    }

    // Exact ground truth per query.
    let mut ground_truth = Vec::with_capacity(q);
    for qi in 0..q {
        let qv = &queries[qi * dim..qi * dim + dim];
        let entry = exact_top_k(qv, &corpus, dim, config.top_k as usize, config.metric);
        ground_truth.push(entry);
    }

    Ok(Artifacts {
        corpus,
        categories,
        queries,
        ground_truth,
    })
}

/// Exact top-k by the chosen metric over every corpus row. Retains the k
/// smallest distances; ties broken by ascending row index (deterministic).
fn exact_top_k(query: &[f32], corpus: &[f32], dim: usize, k: usize, metric: Metric) -> GroundTruth {
    // Keeps the k best (smallest) distances as a min-heap-by-worst via a
    // small sorted vec; O(n*k) per query, exact.
    let mut best: Vec<(f32, u32)> = Vec::with_capacity(k);
    let n = corpus.len() / dim;
    for row in 0..n {
        let candidate = &corpus[row * dim..row * dim + dim];
        let d = metric.distance(query, candidate);
        if best.len() < k {
            best.push((d, row as u32));
        } else if d < best[k - 1].0 {
            // best is kept ascending, so index k-1 is the current worst;
            // replace it only when a strictly nearer candidate arrives.
            best[k - 1] = (d, row as u32);
        } else {
            continue;
        }
        best.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
    }
    best.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    GroundTruth {
        indices: best.iter().map(|(_, idx)| *idx).collect(),
        distances: best.iter().map(|(d, _)| *d).collect(),
    }
}

/// Selectivity tiers the harness can reconstruct from `num_categories`:
/// selecting `category == c` yields approximately `1 / num_categories` rows.
pub fn selectivity_tiers(num_categories: u32) -> Vec<f32> {
    let mut tiers = Vec::new();
    for div in [1u32, 10, 100, 1000] {
        let cats = (num_categories / div).max(1);
        tiers.push(1.0 / cats as f32);
    }
    tiers
}

// ---------------------------------------------------------------------------
// Serialization (manual, dependency-free, little-endian, deterministic)
// ---------------------------------------------------------------------------

const CORPUS_MAGIC: &[u8; 4] = b"FRC1";
const META_MAGIC: &[u8; 4] = b"FRM1";
const QUERY_MAGIC: &[u8; 4] = b"FRQ1";
const TRUTH_MAGIC: &[u8; 4] = b"FRG1";

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Serializes artifacts to versioned fixture byte buffers and a manifest.
pub fn serialize(config: &CorpusConfig, artifacts: &Artifacts) -> Serialized {
    let dim = config.dimension;

    let mut corpus_bytes = Vec::new();
    corpus_bytes.extend_from_slice(CORPUS_MAGIC);
    put_u32(&mut corpus_bytes, 1); // format version
    put_u32(&mut corpus_bytes, dim);
    put_u64(
        &mut corpus_bytes,
        artifacts.corpus.len() as u64 / dim as u64,
    );
    for &v in &artifacts.corpus {
        corpus_bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut meta_bytes = Vec::new();
    meta_bytes.extend_from_slice(META_MAGIC);
    put_u64(&mut meta_bytes, artifacts.categories.len() as u64);
    for &c in &artifacts.categories {
        meta_bytes.extend_from_slice(&c.to_le_bytes());
    }

    let mut queries_bytes = Vec::new();
    queries_bytes.extend_from_slice(QUERY_MAGIC);
    put_u32(&mut queries_bytes, 1);
    put_u32(&mut queries_bytes, dim);
    put_u64(
        &mut queries_bytes,
        artifacts.queries.len() as u64 / dim as u64,
    );
    for &v in &artifacts.queries {
        queries_bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut ground_truth_bytes = Vec::new();
    ground_truth_bytes.extend_from_slice(TRUTH_MAGIC);
    put_u32(&mut ground_truth_bytes, 1);
    put_u64(&mut ground_truth_bytes, artifacts.ground_truth.len() as u64);
    put_u32(&mut ground_truth_bytes, config.top_k);
    for gt in &artifacts.ground_truth {
        for &idx in &gt.indices {
            put_u32(&mut ground_truth_bytes, idx);
        }
        for &d in &gt.distances {
            ground_truth_bytes.extend_from_slice(&d.to_le_bytes());
        }
    }

    let manifest = manifest_json(config);
    Serialized {
        corpus_bytes,
        meta_bytes,
        queries_bytes,
        ground_truth_bytes,
        manifest,
    }
}

fn manifest_json(config: &CorpusConfig) -> String {
    let tiers = selectivity_tiers(config.num_categories);
    let tiers_str = tiers
        .iter()
        .map(|t| format!("{:.6}", t))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{\n  \"format_version\": 1,\n  \"generator\": \"corpus-gen\",\n  \"metric\": \"{:?}\",\n  \"dimension\": {},\n  \"num_vectors\": {},\n  \"num_queries\": {},\n  \"top_k\": {},\n  \"seed\": {},\n  \"distribution\": \"uniform01\",\n  \"num_categories\": {},\n  \"selectivity_tiers\": [{}],\n  \"files\": {{\n    \"corpus\": \"corpus.bin\",\n    \"meta\": \"meta.bin\",\n    \"queries\": \"queries.bin\",\n    \"ground_truth\": \"ground_truth.bin\"\n  }}\n}}",
        config.metric,
        config.dimension,
        config.num_vectors,
        config.num_queries,
        config.top_k,
        config.seed,
        config.num_categories,
        tiers_str,
    )
}

/// Writes all fixtures to `dir`, creating the files. The directory must
/// already exist.
pub fn write_to_dir(dir: &std::path::Path, serialized: &Serialized) -> GenResult<()> {
    write_file(&dir.join("corpus.bin"), &serialized.corpus_bytes)?;
    write_file(&dir.join("meta.bin"), &serialized.meta_bytes)?;
    write_file(&dir.join("queries.bin"), &serialized.queries_bytes)?;
    write_file(
        &dir.join("ground_truth.bin"),
        &serialized.ground_truth_bytes,
    )?;
    write_file(&dir.join("manifest.json"), serialized.manifest.as_bytes())?;
    Ok(())
}

fn write_file(path: &std::path::Path, bytes: &[u8]) -> GenResult<()> {
    let mut file =
        std::fs::File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_config() -> CorpusConfig {
        CorpusConfig {
            num_vectors: 200,
            dimension: 8,
            num_queries: 20,
            top_k: 5,
            seed: 42,
            num_categories: 10,
            metric: Metric::Cosine,
        }
    }

    #[test]
    fn regeneration_is_byte_identical() {
        let cfg = small_config();
        let a = serialize(&cfg, &generate(&cfg).unwrap());
        let b = serialize(&cfg, &generate(&cfg).unwrap());
        assert_eq!(a.corpus_bytes, b.corpus_bytes);
        assert_eq!(a.meta_bytes, b.meta_bytes);
        assert_eq!(a.queries_bytes, b.queries_bytes);
        assert_eq!(a.ground_truth_bytes, b.ground_truth_bytes);
        assert_eq!(a.manifest, b.manifest);
    }

    #[test]
    fn different_seed_changes_output() {
        let mut cfg = small_config();
        let a = serialize(&cfg, &generate(&cfg).unwrap());
        cfg.seed = 99;
        let b = serialize(&cfg, &generate(&cfg).unwrap());
        assert_ne!(a.corpus_bytes, b.corpus_bytes);
    }

    #[test]
    fn ground_truth_is_exact_top_k() {
        let cfg = small_config();
        let art = generate(&cfg).unwrap();
        // Recompute ground truth for query 0 independently and compare.
        let dim = cfg.dimension as usize;
        let qv = &art.queries[0..dim];
        let mut all: Vec<(f32, u32)> = (0..cfg.num_vectors as usize)
            .map(|row| {
                let cand = &art.corpus[row * dim..row * dim + dim];
                (Metric::Cosine.distance(qv, cand), row as u32)
            })
            .collect();
        all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let expected: Vec<u32> = all[..cfg.top_k as usize].iter().map(|(_, i)| *i).collect();
        assert_eq!(art.ground_truth[0].indices, expected);
    }

    #[test]
    fn category_count_matches_num_categories_distribution() {
        let cfg = small_config();
        let art = generate(&cfg).unwrap();
        assert_eq!(art.categories.len(), cfg.num_vectors as usize);
        let max = art.categories.iter().copied().max().unwrap();
        assert!((max as u32) < cfg.num_categories);
    }

    #[test]
    fn manifest_reports_selectivity_tiers() {
        let cfg = small_config();
        let s = serialize(&cfg, &generate(&cfg).unwrap());
        assert!(s.manifest.contains("selectivity_tiers"));
        assert!(s.manifest.contains("1.000000"));
    }
}
