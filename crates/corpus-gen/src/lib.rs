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
                    // na/nb are squared norms; cosine needs their roots.
                    1.0 - dot / (na.sqrt() * nb.sqrt())
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
const FORMAT_VERSION: u32 = 1;

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
    put_u32(&mut corpus_bytes, FORMAT_VERSION);
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
    put_u32(&mut meta_bytes, FORMAT_VERSION);
    put_u64(&mut meta_bytes, artifacts.categories.len() as u64);
    for &c in &artifacts.categories {
        meta_bytes.extend_from_slice(&c.to_le_bytes());
    }

    let mut queries_bytes = Vec::new();
    queries_bytes.extend_from_slice(QUERY_MAGIC);
    put_u32(&mut queries_bytes, FORMAT_VERSION);
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
    put_u32(&mut ground_truth_bytes, FORMAT_VERSION);
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

fn read_file(path: &std::path::Path) -> GenResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// A corpus loaded back from fixture files written by [`write_to_dir`].
/// Contents are identical to what was generated.
#[derive(Debug, Clone)]
pub struct LoadedCorpus {
    pub dimension: u32,
    pub metric: Metric,
    pub num_categories: u32,
    pub corpus: Vec<f32>,
    pub categories: Vec<u16>,
    pub queries: Vec<f32>,
    pub ground_truth: Vec<GroundTruth>,
    pub top_k: u32,
}

impl LoadedCorpus {
    /// Number of queries held out in the fixture set.
    pub fn num_queries(&self) -> usize {
        self.ground_truth.len()
    }

    /// Query row `i` as a slice.
    pub fn query(&self, i: usize) -> &[f32] {
        let d = self.dimension as usize;
        &self.queries[i * d..i * d + d]
    }
}

/// Loads fixtures from a directory produced by [`write_to_dir`].
///
/// Every field is validated against the recorded header so a truncated or
/// foreign file fails loudly instead of silently mis-measuring.
pub fn load(dir: &std::path::Path) -> GenResult<LoadedCorpus> {
    let corpus_raw = read_file(&dir.join("corpus.bin"))?;
    let meta_raw = read_file(&dir.join("meta.bin"))?;
    let queries_raw = read_file(&dir.join("queries.bin"))?;
    let truth_raw = read_file(&dir.join("ground_truth.bin"))?;
    let manifest_text = String::from_utf8(read_file(&dir.join("manifest.json"))?)
        .map_err(|_| "manifest.json is not valid UTF-8".to_string())?;

    let mut r = Reader::new(&corpus_raw);
    r.magic(CORPUS_MAGIC)?;
    expect_version(r.u32()?)?;
    let dimension = r.u32()?;
    let num_vectors = r.u64()?;
    let corpus = r.f32s(num_vectors as usize * dimension as usize)?;

    let mut r = Reader::new(&meta_raw);
    r.magic(META_MAGIC)?;
    expect_version(r.u32()?)?;
    let meta_count = r.u64()?;
    let categories = r.u16s(meta_count as usize)?;

    let mut r = Reader::new(&queries_raw);
    r.magic(QUERY_MAGIC)?;
    expect_version(r.u32()?)?;
    let query_dim = r.u32()?;
    let num_stored_queries = r.u64()?;
    let queries = r.f32s(num_stored_queries as usize * query_dim as usize)?;

    let mut r = Reader::new(&truth_raw);
    r.magic(TRUTH_MAGIC)?;
    expect_version(r.u32()?)?;
    let truth_count = r.u64()?;
    let top_k = r.u32()?;
    let mut ground_truth = Vec::with_capacity(truth_count as usize);
    for _ in 0..truth_count {
        let mut indices = Vec::with_capacity(top_k as usize);
        for _ in 0..top_k {
            indices.push(r.u32()?);
        }
        let distances = r.f32s(top_k as usize)?;
        ground_truth.push(GroundTruth { indices, distances });
    }

    if query_dim != dimension {
        return Err(format!(
            "query dimension {query_dim} != corpus dimension {dimension}"
        ));
    }
    if meta_count != num_vectors {
        return Err(format!(
            "meta rows {meta_count} != corpus rows {num_vectors}"
        ));
    }
    if truth_count != num_stored_queries {
        return Err(format!(
            "ground-truth queries {truth_count} != stored queries {num_stored_queries}"
        ));
    }

    let metric = json_metric_field(&manifest_text)
        .ok_or_else(|| "manifest.json missing usable \"metric\"".to_string())?;
    let num_categories = json_u32_field(&manifest_text, "num_categories")
        .filter(|&c| c > 0)
        .ok_or_else(|| "manifest.json missing usable \"num_categories\"".to_string())?;

    Ok(LoadedCorpus {
        dimension,
        metric,
        num_categories,
        corpus,
        categories,
        queries,
        ground_truth,
        top_k,
    })
}

/// Exact top-k restricted to rows whose category value is in `keep`.
/// Returns global row indices, nearest first. This is the filtered-search
/// oracle: the harness compares filtered runs against it at each
/// selectivity tier.
pub fn filtered_exact_top_k(
    query: &[f32],
    corpus: &[f32],
    categories: &[u16],
    dimension: usize,
    k: usize,
    metric: Metric,
    keep: &[i64],
) -> Vec<u32> {
    let keep_set: std::collections::HashSet<i64> = keep.iter().copied().collect();
    let n = corpus.len() / dimension;
    let mut best: Vec<(f32, u32)> = Vec::with_capacity(k);
    for row in 0..n {
        if !keep_set.contains(&(categories[row] as i64)) {
            continue;
        }
        let candidate = &corpus[row * dimension..row * dimension + dimension];
        let d = metric.distance(query, candidate);
        if best.len() < k {
            best.push((d, row as u32));
        } else if d < best[k - 1].0 {
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
    best.into_iter().map(|(_, idx)| idx).collect()
}

fn expect_version(v: u32) -> GenResult<()> {
    if v != FORMAT_VERSION {
        return Err(format!(
            "fixture format version {v} != supported {FORMAT_VERSION}"
        ));
    }
    Ok(())
}

/// Loads SIFT1M-style fixture files (`base.fvecs`, `query.fvecs`,
/// `groundtruth.ivecs`) into the same [`LoadedCorpus`] shape as [`load`],
/// giving the harness its secondary ANN-benchmarks comparability mode.
/// SIFT ground truth is Euclidean (L2). The format carries no categorical
/// metadata, so filtered runs are unavailable (`num_categories == 0`).
pub fn load_sift(dir: &std::path::Path) -> GenResult<LoadedCorpus> {
    let base = read_file(&dir.join("base.fvecs"))?;
    let queries_raw = read_file(&dir.join("query.fvecs"))?;
    let truth = read_file(&dir.join("groundtruth.ivecs"))?;

    let (corpus, dimension) = parse_fvecs(&base)?;
    let (queries, query_dim) = parse_fvecs(&queries_raw)?;
    if query_dim != dimension {
        return Err(format!(
            "SIFT query dimension {query_dim} != base dimension {dimension}"
        ));
    }
    let (truth_flat, top_k) = parse_ivecs(&truth)?;
    let num_queries = truth_flat.len().checked_div(top_k).unwrap_or(0);
    let ground_truth = (0..num_queries)
        .map(|q| GroundTruth {
            indices: truth_flat[q * top_k..(q + 1) * top_k].to_vec(),
            distances: vec![0.0; top_k],
        })
        .collect();
    let num_vectors = corpus.len() / dimension;

    Ok(LoadedCorpus {
        dimension: dimension as u32,
        metric: Metric::L2,
        num_categories: 0,
        categories: vec![0; num_vectors],
        corpus,
        queries,
        ground_truth,
        top_k: top_k as u32,
    })
}

/// Parses the classic fvecs container: repeated records of `[i32 dim][dim ×
/// f32]`. Returns the flat vector payload and per-record width.
fn parse_fvecs(bytes: &[u8]) -> GenResult<(Vec<f32>, usize)> {
    let mut r = Reader::new(bytes);
    let mut out = Vec::new();
    let mut width: Option<usize> = None;
    while r.pos < bytes.len() {
        let dim = r.u32()? as usize;
        if dim == 0 || dim > 1 << 20 {
            return Err(format!("implausible fvecs record width {dim}"));
        }
        match width {
            None => width = Some(dim),
            Some(w) if w != dim => return Err("fvecs records have inconsistent widths".to_string()),
            _ => {}
        }
        out.extend(r.f32s(dim)?);
    }
    let width = width.ok_or_else(|| "empty fvecs file".to_string())?;
    Ok((out, width))
}

/// Parses the ivecs container: repeated records of `[i32 dim][dim × i32]`.
/// Returns flat indices and per-record width.
fn parse_ivecs(bytes: &[u8]) -> GenResult<(Vec<u32>, usize)> {
    let mut r = Reader::new(bytes);
    let mut out = Vec::new();
    let mut width: Option<usize> = None;
    while r.pos < bytes.len() {
        let dim = r.u32()? as usize;
        if dim == 0 || dim > 1 << 20 {
            return Err(format!("implausible ivecs record width {dim}"));
        }
        match width {
            None => width = Some(dim),
            Some(w) if w != dim => return Err("ivecs records have inconsistent widths".to_string()),
            _ => {}
        }
        for _ in 0..dim {
            out.push(r.u32()?);
        }
    }
    let width = width.ok_or_else(|| "empty ivecs file".to_string())?;
    Ok((out, width))
}

fn json_u32_field(manifest: &str, field: &str) -> Option<u32> {
    let needle = format!("\"{field}\":");
    let pos = manifest.find(&needle)?;
    let rest = manifest[pos + needle.len()..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse().ok()
}

fn json_metric_field(manifest: &str) -> Option<Metric> {
    let needle = "\"metric\": \"";
    let pos = manifest.find(needle)?;
    let rest = &manifest[pos + needle.len()..];
    let end = rest.find('"')?;
    match &rest[..end] {
        "Cosine" => Some(Metric::Cosine),
        "L2" => Some(Metric::L2),
        "Dot" => Some(Metric::Dot),
        _ => None,
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn magic(&mut self, expected: &[u8]) -> GenResult<()> {
        let got = self.take(expected.len())?;
        if got != expected {
            return Err("fixture magic mismatch (not a corpus-gen fixture?)".to_string());
        }
        Ok(())
    }

    fn u32(&mut self) -> GenResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> GenResult<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f32s(&mut self, count: usize) -> GenResult<Vec<f32>> {
        let b = self.take(count * 4)?;
        Ok(b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }

    fn u16s(&mut self, count: usize) -> GenResult<Vec<u16>> {
        let b = self.take(count * 2)?;
        Ok(b.chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect())
    }

    fn take(&mut self, n: usize) -> GenResult<&'a [u8]> {
        if self.bytes.len() < self.pos + n {
            return Err(format!(
                "fixture truncated: wanted {} bytes at offset {}, file has {}",
                n,
                self.pos,
                self.bytes.len()
            ));
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }
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

    fn temp_fixture_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "corpus-gen-{}-{}-{tag}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_round_trips_written_fixtures() {
        let cfg = small_config();
        let art = generate(&cfg).unwrap();
        let dir = temp_fixture_dir("roundtrip");
        write_to_dir(&dir, &serialize(&cfg, &art)).unwrap();

        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.dimension, cfg.dimension);
        assert_eq!(loaded.metric, cfg.metric);
        assert_eq!(loaded.num_categories, cfg.num_categories);
        assert_eq!(loaded.top_k, cfg.top_k);
        assert_eq!(loaded.corpus, art.corpus);
        assert_eq!(loaded.categories, art.categories);
        assert_eq!(loaded.queries, art.queries);
        assert_eq!(loaded.ground_truth.len(), cfg.num_queries as usize);
        for (a, b) in loaded.ground_truth.iter().zip(&art.ground_truth) {
            assert_eq!(a, b);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_rejects_truncated_fixtures() {
        let cfg = small_config();
        let dir = temp_fixture_dir("truncated");
        write_to_dir(&dir, &serialize(&cfg, &generate(&cfg).unwrap())).unwrap();
        let corpus_path = dir.join("corpus.bin");
        let bytes = std::fs::read(&corpus_path).unwrap();
        std::fs::write(&corpus_path, &bytes[..bytes.len() - 8]).unwrap();
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_sift_parses_fvecs_ivecs() {
        // Two base vectors, one query (dim 3), ground truth k=2.
        let mut base: Vec<u8> = Vec::new();
        for vec in [&[1.0f32, 0.0, 0.0][..], &[0.0, 1.0, 0.0][..]] {
            base.extend_from_slice(&3i32.to_le_bytes());
            for v in vec {
                base.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut queries: Vec<u8> = Vec::new();
        queries.extend_from_slice(&3i32.to_le_bytes());
        for v in [0.5f32, 0.5, 0.0] {
            queries.extend_from_slice(&v.to_le_bytes());
        }
        let mut truth: Vec<u8> = Vec::new();
        truth.extend_from_slice(&2i32.to_le_bytes());
        truth.extend_from_slice(&0i32.to_le_bytes());
        truth.extend_from_slice(&1i32.to_le_bytes());

        let dir = temp_fixture_dir("sift");
        std::fs::write(dir.join("base.fvecs"), &base).unwrap();
        std::fs::write(dir.join("query.fvecs"), &queries).unwrap();
        std::fs::write(dir.join("groundtruth.ivecs"), &truth).unwrap();

        let loaded = load_sift(&dir).unwrap();
        assert_eq!(loaded.dimension, 3);
        assert_eq!(loaded.metric, Metric::L2);
        assert_eq!(loaded.num_categories, 0);
        assert_eq!(loaded.top_k, 2);
        assert_eq!(loaded.num_queries(), 1);
        assert_eq!(loaded.ground_truth[0].indices, vec![0, 1]);
        assert_eq!(loaded.corpus.len(), 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filtered_exact_top_k_matches_bruteforce_subset() {
        let cfg = small_config();
        let art = generate(&cfg).unwrap();
        let dim = cfg.dimension as usize;
        let keep: Vec<i64> = vec![0, 1];
        // Independent recomputation over the kept rows only.
        let qv = &art.queries[0..dim];
        let mut all: Vec<(f32, u32)> = (0..cfg.num_vectors as usize)
            .filter(|&row| (art.categories[row] as i64) == 0 || (art.categories[row] as i64) == 1)
            .map(|row| {
                let cand = &art.corpus[row * dim..row * dim + dim];
                (Metric::Cosine.distance(qv, cand), row as u32)
            })
            .collect();
        all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let expected: Vec<u32> = all[..cfg.top_k as usize].iter().map(|(_, i)| *i).collect();

        let got = filtered_exact_top_k(
            qv,
            &art.corpus,
            &art.categories,
            dim,
            cfg.top_k as usize,
            Metric::Cosine,
            &keep,
        );
        assert_eq!(got, expected);
    }
}
