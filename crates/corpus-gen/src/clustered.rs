//! Realistic-clustered accuracy corpus generator (ROADMAP FDB-024, ADR 0009).
//!
//! The uniform corpus in [`crate`] is the determinism contract; this module
//! is the accuracy contract. Vectors are placed around cluster centers drawn
//! from a Gaussian mixture so that neighbor distances do not concentrate
//! (the structural failure mode that made uniform 512-d pathological for
//! graph ANN — see `docs/baselines/fdb070-campaign.md` §2 and ADR 0009).
//!
//! Determinism is preserved by the same SplitMix64 scheme: every float is
//! derived from a seeded integer PRNG, so regeneration with identical
//! parameters and seed emits byte-identical artifacts (FDB-024 exit
//! criterion). The on-disk format mirrors the uniform corpus so the harness
//! can load clustered fixtures through the unchanged
//! [`crate::load`] machinery with no special-case branches in `harness`.
//!
//! The clustering itself:
//! - `num_clusters` centers are drawn from a deterministic Gaussian
//!   (mean 0.5, stddev 0.15 per component — chosen so centers land in the
//!   same value range as the uniform corpus's [0, 1) draws).
//! - Each vector is assigned to a cluster, then sampled as
//!   `center + N(0, sigma²)` componentwise.
//! - Per-vector category metadata is preserved (clusters and categories are
//!   independent streams) so selectivity-tier construction is unchanged.
//!
//! Owned exclusively by ROADMAP FDB-024. Co-owned with FDB-020: this file is
//! a sibling, not a fork — `lib.rs` is extended by re-export only.

use std::io::Write;

/// Distance function. Mirrors `crate::Metric` and the Table [`Metric`] vocabulary.
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

/// Configuration for one clustered corpus generation. All fields are part of
/// the versioned fixture identity: changing any of them changes the bytes.
#[derive(Debug, Clone, Copy)]
pub struct ClusteredConfig {
    pub num_vectors: u64,
    pub dimension: u32,
    pub num_queries: u64,
    pub top_k: u32,
    pub seed: u64,
    pub num_categories: u32,
    pub num_clusters: u32,
    /// Per-component standard deviation of the noise added to each cluster
    /// center. Smaller values produce tighter clusters (sharper accuracy
    /// headroom for graph ANN); 0.0 collapses to deterministic assignment
    /// and is rejected.
    pub cluster_stddev: f32,
    /// Center-component distribution: mean and standard deviation of the
    /// per-component Gaussian used to place cluster centers.
    pub center_mean: f32,
    pub center_stddev: f32,
    pub metric: Metric,
}

impl Default for ClusteredConfig {
    fn default() -> Self {
        Self {
            num_vectors: 10_000_000,
            dimension: 512,
            num_queries: 10_000,
            top_k: 10,
            seed: 0x9E37_79B9_7F4A_7C15,
            num_categories: 1000,
            num_clusters: 1000,
            cluster_stddev: 0.05,
            center_mean: 0.5,
            center_stddev: 0.15,
            metric: Metric::Cosine,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroundTruth {
    pub indices: Vec<u32>,
    pub distances: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct ClusteredArtifacts {
    pub corpus: Vec<f32>,
    pub categories: Vec<u16>,
    pub queries: Vec<f32>,
    pub ground_truth: Vec<GroundTruth>,
}

#[derive(Debug, Clone)]
pub struct Serialized {
    pub corpus_bytes: Vec<u8>,
    pub meta_bytes: Vec<u8>,
    pub queries_bytes: Vec<u8>,
    pub ground_truth_bytes: Vec<u8>,
    pub manifest: String,
}

pub type GenResult<T> = Result<T, String>;

// ---------------------------------------------------------------------------
// Deterministic PRNG (mirrors `crate::Prng` exactly — same SplitMix64 scheme).
// Splitting into a private copy keeps the two generator families independent
// at the source level: changing one does not silently change the other.
// ---------------------------------------------------------------------------

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

    /// Next f32 uniformly in [0, 1) using the top 24 mantissa bits.
    fn next_f32(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32;
        (bits as f32) / (1u32 << 24) as f32
    }

    /// Standard normal sample via Box–Muller from two uniform draws. Pure
    /// integer arithmetic up to the final f32 conversion, so the result is
    /// platform-independent and bit-stable across reruns (the property
    /// byte-identical regeneration depends on).
    fn next_standard_normal(&mut self) -> f32 {
        // Avoid u1=0 (log(0)=-inf) by rejection; one iteration suffices for
        // the 24-bit uniform mantissa, so this is at most two draws.
        let u1 = loop {
            let v = self.next_f32();
            if v > 0.0 {
                break v;
            }
        };
        let u2 = self.next_f32();
        let radius = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        radius * theta.cos()
    }

    /// Normal sample with the given mean and standard deviation.
    fn next_normal(&mut self, mean: f32, stddev: f32) -> f32 {
        mean + stddev * self.next_standard_normal()
    }

    fn next_category(&mut self, num_categories: u32) -> u16 {
        (self.next_u64() % num_categories as u64) as u16
    }
}

/// Deterministically derives an independent sub-stream seed from the master
/// seed and a stream identifier. Identical algorithm to the uniform generator
/// so the two corpora cannot accidentally share PRNG state.
fn sub_seed(master: u64, stream: u64) -> u64 {
    let mut p = Prng::new(master ^ 0x1234_5678_9ABC_DEF0);
    let _ = p.next_u64();
    p.next_u64() ^ stream.wrapping_mul(0x85EB_CA6B_9487_E9C1)
}

/// Generates the clustered corpus, per-vector categories, query set, and
/// exact ground truth. Pure function of [`ClusteredConfig`].
pub fn generate(config: &ClusteredConfig) -> GenResult<ClusteredArtifacts> {
    if config.dimension == 0 {
        return Err("dimension must be greater than zero".to_string());
    }
    if config.top_k == 0 {
        return Err("top_k must be greater than zero".to_string());
    }
    if config.num_vectors == 0 || config.num_queries == 0 {
        return Err("num_vectors and num_queries must be greater than zero".to_string());
    }
    if config.num_clusters == 0 {
        return Err("num_clusters must be greater than zero".to_string());
    }
    if !config.cluster_stddev.is_finite() || config.cluster_stddev <= 0.0 {
        return Err("cluster_stddev must be a finite positive number (a zero sigma collapses to assignment only)".to_string());
    }
    if !config.center_stddev.is_finite() || config.center_stddev <= 0.0 {
        return Err("center_stddev must be a finite positive number".to_string());
    }

    let dim = config.dimension as usize;
    let n = config.num_vectors as usize;
    let q = config.num_queries as usize;
    let k = config.num_clusters as usize;

    // 1. Cluster centers: drawn componentwise from N(center_mean, center_stddev²).
    let mut center_prng = Prng::new(sub_seed(config.seed, 10));
    let mut centers: Vec<f32> = Vec::with_capacity(k * dim);
    for _ in 0..k * dim {
        centers.push(center_prng.next_normal(config.center_mean, config.center_stddev));
    }

    // 2. Per-vector cluster assignment (uniform over K clusters) and metadata category.
    let mut assign_prng = Prng::new(sub_seed(config.seed, 11));
    let assignments: Vec<u32> = (0..n)
        .map(|_| (assign_prng.next_u64() % k as u64) as u32)
        .collect();
    let mut cat_prng = Prng::new(sub_seed(config.seed, 12));
    let categories: Vec<u16> = (0..n)
        .map(|_| cat_prng.next_category(config.num_categories))
        .collect();

    // 3. Corpus vectors: cluster_center + N(0, cluster_stddev²) per component.
    let mut noise_prng = Prng::new(sub_seed(config.seed, 13));
    let mut corpus: Vec<f32> = Vec::with_capacity(n * dim);
    for row in 0..n {
        let center =
            &centers[assignments[row] as usize * dim..assignments[row] as usize * dim + dim];
        for &c in center.iter().take(dim) {
            corpus.push(c + noise_prng.next_normal(0.0, config.cluster_stddev));
        }
    }

    // 4. Query vectors: also drawn from the same mixture so nearest neighbors
    //    exist in-cluster (not in some out-of-distribution tail). One in-K
    //    random cluster per query, then center + N(0, cluster_stddev²).
    let mut query_assign_prng = Prng::new(sub_seed(config.seed, 14));
    let mut query_noise_prng = Prng::new(sub_seed(config.seed, 15));
    let mut queries: Vec<f32> = Vec::with_capacity(q * dim);
    for _ in 0..q {
        let cluster_idx = (query_assign_prng.next_u64() % k as u64) as usize;
        let center = &centers[cluster_idx * dim..cluster_idx * dim + dim];
        for &c in center.iter().take(dim) {
            queries.push(c + query_noise_prng.next_normal(0.0, config.cluster_stddev));
        }
    }

    // 5. Exact ground truth per query.
    let mut ground_truth = Vec::with_capacity(q);
    for qi in 0..q {
        let qv = &queries[qi * dim..qi * dim + dim];
        let entry = exact_top_k(qv, &corpus, dim, config.top_k as usize, config.metric);
        ground_truth.push(entry);
    }

    Ok(ClusteredArtifacts {
        corpus,
        categories,
        queries,
        ground_truth,
    })
}

fn exact_top_k(query: &[f32], corpus: &[f32], dim: usize, k: usize, metric: Metric) -> GroundTruth {
    let mut best: Vec<(f32, u32)> = Vec::with_capacity(k);
    let n = corpus.len() / dim;
    for row in 0..n {
        let candidate = &corpus[row * dim..row * dim + dim];
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

/// Selectivity tiers: same construction as the uniform corpus (1.0 / 0.1 /
/// 0.01 / 0.001) so filtered runs are comparable across the two corpora.
pub fn selectivity_tiers(num_categories: u32) -> Vec<f32> {
    let mut tiers = Vec::new();
    for div in [1u32, 10, 100, 1000] {
        let cats = (num_categories / div).max(1);
        tiers.push(1.0 / cats as f32);
    }
    tiers
}

// ---------------------------------------------------------------------------
// Serialization (mirrors `crate::serialize` so the same `load` function
// reads both fixture families. The manifest records the distribution family
// and clustering parameters so downstream consumers can tell them apart.)
// ---------------------------------------------------------------------------

const CLUSTERED_CORPUS_MAGIC: &[u8; 4] = b"FRC2";
const CLUSTERED_META_MAGIC: &[u8; 4] = b"FRM2";
const CLUSTERED_QUERY_MAGIC: &[u8; 4] = b"FRQ2";
const CLUSTERED_TRUTH_MAGIC: &[u8; 4] = b"FRG2";
const CLUSTERED_FORMAT_VERSION: u32 = 1;

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn serialize(config: &ClusteredConfig, artifacts: &ClusteredArtifacts) -> Serialized {
    let dim = config.dimension;

    let mut corpus_bytes = Vec::new();
    corpus_bytes.extend_from_slice(CLUSTERED_CORPUS_MAGIC);
    put_u32(&mut corpus_bytes, CLUSTERED_FORMAT_VERSION);
    put_u32(&mut corpus_bytes, dim);
    put_u64(
        &mut corpus_bytes,
        artifacts.corpus.len() as u64 / dim as u64,
    );
    for &v in &artifacts.corpus {
        corpus_bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut meta_bytes = Vec::new();
    meta_bytes.extend_from_slice(CLUSTERED_META_MAGIC);
    put_u32(&mut meta_bytes, CLUSTERED_FORMAT_VERSION);
    put_u64(&mut meta_bytes, artifacts.categories.len() as u64);
    for &c in &artifacts.categories {
        meta_bytes.extend_from_slice(&c.to_le_bytes());
    }

    let mut queries_bytes = Vec::new();
    queries_bytes.extend_from_slice(CLUSTERED_QUERY_MAGIC);
    put_u32(&mut queries_bytes, CLUSTERED_FORMAT_VERSION);
    put_u32(&mut queries_bytes, dim);
    put_u64(
        &mut queries_bytes,
        artifacts.queries.len() as u64 / dim as u64,
    );
    for &v in &artifacts.queries {
        queries_bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut ground_truth_bytes = Vec::new();
    ground_truth_bytes.extend_from_slice(CLUSTERED_TRUTH_MAGIC);
    put_u32(&mut ground_truth_bytes, CLUSTERED_FORMAT_VERSION);
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

fn manifest_json(config: &ClusteredConfig) -> String {
    let tiers = selectivity_tiers(config.num_categories);
    let tiers_str = tiers
        .iter()
        .map(|t| format!("{:.6}", t))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{{\n  \"format_version\": 1,\n  \"generator\": \"corpus-gen-clustered\",\n  \"metric\": \"{:?}\",\n  \"dimension\": {},\n  \"num_vectors\": {},\n  \"num_queries\": {},\n  \"top_k\": {},\n  \"seed\": {},\n  \"distribution\": \"gaussian_mixture\",\n  \"num_clusters\": {},\n  \"cluster_stddev\": {:.6},\n  \"center_mean\": {:.6},\n  \"center_stddev\": {:.6},\n  \"num_categories\": {},\n  \"selectivity_tiers\": [{}],\n  \"files\": {{\n    \"corpus\": \"corpus.bin\",\n    \"meta\": \"meta.bin\",\n    \"queries\": \"queries.bin\",\n    \"ground_truth\": \"ground_truth.bin\"\n  }}\n}}",
        config.metric,
        config.dimension,
        config.num_vectors,
        config.num_queries,
        config.top_k,
        config.seed,
        config.num_clusters,
        config.cluster_stddev,
        config.center_mean,
        config.center_stddev,
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

/// Mirrors the uniform [`crate::load`] but reads the `FR?2` magic so the two
/// families cannot be confused at load time. Same `LoadedCorpus` shape — the
/// harness does not need to know which family it consumed.
pub fn load(dir: &std::path::Path) -> GenResult<crate::LoadedCorpus> {
    let corpus_raw =
        std::fs::read(dir.join("corpus.bin")).map_err(|e| format!("reading corpus.bin: {e}"))?;
    let meta_raw =
        std::fs::read(dir.join("meta.bin")).map_err(|e| format!("reading meta.bin: {e}"))?;
    let queries_raw =
        std::fs::read(dir.join("queries.bin")).map_err(|e| format!("reading queries.bin: {e}"))?;
    let truth_raw = std::fs::read(dir.join("ground_truth.bin"))
        .map_err(|e| format!("reading ground_truth.bin: {e}"))?;
    let manifest_text = String::from_utf8(
        std::fs::read(dir.join("manifest.json"))
            .map_err(|e| format!("reading manifest.json: {e}"))?,
    )
    .map_err(|_| "manifest.json is not valid UTF-8".to_string())?;

    let mut r = Reader::new(&corpus_raw);
    r.magic(CLUSTERED_CORPUS_MAGIC)?;
    expect_version(r.u32()?)?;
    let dimension = r.u32()?;
    let num_vectors = r.u64()?;
    let corpus = r.f32s(num_vectors as usize * dimension as usize)?;

    let mut r = Reader::new(&meta_raw);
    r.magic(CLUSTERED_META_MAGIC)?;
    expect_version(r.u32()?)?;
    let meta_count = r.u64()?;
    let categories = r.u16s(meta_count as usize)?;

    let mut r = Reader::new(&queries_raw);
    r.magic(CLUSTERED_QUERY_MAGIC)?;
    expect_version(r.u32()?)?;
    let query_dim = r.u32()?;
    let num_stored_queries = r.u64()?;
    let queries = r.f32s(num_stored_queries as usize * query_dim as usize)?;

    let mut r = Reader::new(&truth_raw);
    r.magic(CLUSTERED_TRUTH_MAGIC)?;
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
        ground_truth.push(crate::GroundTruth { indices, distances });
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

    Ok(crate::LoadedCorpus {
        dimension,
        metric: match metric {
            Metric::Cosine => crate::Metric::Cosine,
            Metric::L2 => crate::Metric::L2,
            Metric::Dot => crate::Metric::Dot,
        },
        num_categories,
        corpus,
        categories,
        queries,
        ground_truth,
        top_k,
    })
}

fn expect_version(v: u32) -> GenResult<()> {
    if v != CLUSTERED_FORMAT_VERSION {
        return Err(format!(
            "clustered fixture format version {v} != supported {CLUSTERED_FORMAT_VERSION}"
        ));
    }
    Ok(())
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
            return Err("clustered fixture magic mismatch".to_string());
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
                "clustered fixture truncated: wanted {} bytes at offset {}, file has {}",
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

    fn small_config() -> ClusteredConfig {
        ClusteredConfig {
            num_vectors: 200,
            dimension: 8,
            num_queries: 20,
            top_k: 5,
            seed: 42,
            num_categories: 10,
            num_clusters: 4,
            cluster_stddev: 0.1,
            center_mean: 0.5,
            center_stddev: 0.15,
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
    fn different_num_clusters_changes_output() {
        let mut cfg = small_config();
        let a = serialize(&cfg, &generate(&cfg).unwrap());
        cfg.num_clusters = 8;
        let b = serialize(&cfg, &generate(&cfg).unwrap());
        assert_ne!(a.corpus_bytes, b.corpus_bytes);
    }

    #[test]
    fn ground_truth_is_exact_top_k() {
        let cfg = small_config();
        let art = generate(&cfg).unwrap();
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
    fn manifest_reports_clustering_params_and_selectivity_tiers() {
        let cfg = small_config();
        let s = serialize(&cfg, &generate(&cfg).unwrap());
        for key in [
            "\"distribution\": \"gaussian_mixture\"",
            "\"num_clusters\": 4",
            "\"cluster_stddev\": 0.100000",
            "\"center_mean\": 0.500000",
            "\"center_stddev\": 0.150000",
            "\"selectivity_tiers\"",
            "\"num_categories\": 10",
            "\"top_k\": 5",
        ] {
            assert!(s.manifest.contains(key), "manifest missing {key}");
        }
    }

    #[test]
    fn rejects_zero_cluster_stddev() {
        let mut cfg = small_config();
        cfg.cluster_stddev = 0.0;
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn rejects_zero_num_clusters() {
        let mut cfg = small_config();
        cfg.num_clusters = 0;
        assert!(generate(&cfg).is_err());
    }

    fn temp_fixture_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "corpus-gen-clustered-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            tag
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
        let expected_metric = match cfg.metric {
            Metric::Cosine => crate::Metric::Cosine,
            Metric::L2 => crate::Metric::L2,
            Metric::Dot => crate::Metric::Dot,
        };
        assert_eq!(loaded.metric, expected_metric);
        assert_eq!(loaded.num_categories, cfg.num_categories);
        assert_eq!(loaded.top_k, cfg.top_k);
        assert_eq!(loaded.corpus, art.corpus);
        assert_eq!(loaded.categories, art.categories);
        assert_eq!(loaded.queries, art.queries);
        assert_eq!(loaded.ground_truth.len(), cfg.num_queries as usize);
        for (a, b) in loaded.ground_truth.iter().zip(&art.ground_truth) {
            assert_eq!(a.indices, b.indices);
            assert_eq!(a.distances, b.distances);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_rejects_uniform_magic() {
        // A uniform-corpus fixture directory (FRC1 magic) must be rejected by
        // the clustered loader; the two families cannot be confused.
        let cfg = small_config();
        let dir = temp_fixture_dir("wrong-magic");
        // Write uniform-style corpus.bin header manually.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"FRC1");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&cfg.dimension.to_le_bytes());
        bytes.extend_from_slice(&cfg.num_vectors.to_le_bytes());
        for _ in 0..(cfg.num_vectors as usize * cfg.dimension as usize) {
            bytes.extend_from_slice(&0.0f32.to_le_bytes());
        }
        std::fs::write(dir.join("corpus.bin"), &bytes).unwrap();
        assert!(load(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clustered_nearest_neighbour_is_in_same_cluster() {
        // For tight enough clusters and top_k <= cluster size, every query's
        // nearest neighbour must come from the same cluster. This is the
        // structural property that uniform data lacks and that graph ANN
        // requires to express meaningful recall headroom.
        let cfg = ClusteredConfig {
            num_vectors: 1000,
            dimension: 4,
            num_queries: 50,
            top_k: 5,
            seed: 7,
            num_categories: 1000,
            num_clusters: 10,
            cluster_stddev: 0.01,
            center_mean: 0.5,
            center_stddev: 0.3,
            metric: Metric::L2,
        };
        let art = generate(&cfg).unwrap();
        let _dim = cfg.dimension as usize;
        // Build (row → cluster) map by replaying the assignment stream.
        let mut assign_prng = Prng::new(sub_seed(cfg.seed, 11));
        let assignments: Vec<u32> = (0..cfg.num_vectors as usize)
            .map(|_| (assign_prng.next_u64() % cfg.num_clusters as u64) as u32)
            .collect();
        // Build (query → cluster) map by replaying the query-assignment stream.
        let mut q_assign_prng = Prng::new(sub_seed(cfg.seed, 14));
        let q_assignments: Vec<u32> = (0..cfg.num_queries as usize)
            .map(|_| (q_assign_prng.next_u64() % cfg.num_clusters as u64) as u32)
            .collect();
        for (qi, &q_cluster) in q_assignments.iter().enumerate() {
            for &hit in &art.ground_truth[qi].indices {
                let row_cluster = assignments[hit as usize];
                assert_eq!(
                    row_cluster, q_cluster,
                    "top-{} hit at row {} (cluster {}) is not in query's cluster {} (qi={})",
                    cfg.top_k, hit, row_cluster, q_cluster, qi
                );
            }
        }
    }
}
