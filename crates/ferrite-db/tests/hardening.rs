//! FDB-051 — error/crash hardening integration tests.
//!
//! Owned by the audit/integration-test tree only (no production-file edits).
//! These tests prove the FDB-051 exit criteria against the already-shipped
//! public API:
//!
//! * mutated Segment corpora fed to [`SegmentReader`] never panic — they return
//!   `Ok` or an `Err` (no `unwrap`/index panic on hostile bytes);
//! * the public search entry never panics under randomized input;
//! * crash recovery honors ADR 0003: a Segment is committed-or-absent, never
//!   torn/partially visible.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use ferrite_db::errors::Error;
use ferrite_db::search::{Predicate, SearchOptions, search};
use ferrite_db::storage::{SegmentReader, SegmentWriter};
use ferrite_db::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};
use ferrite_db::write_path::{InsertRecord, MetadataValue, WritePath};

static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

fn tempdir() -> std::path::PathBuf {
    let id = std::process::id();
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("fdb-hardening-{id}-{seq}"));
    fs::create_dir_all(&dir).expect("mkdir tempdir");
    dir
}

/// Writes one fully valid Segment and returns its committed path.
fn write_valid_segment(dir: &Path, id: u64, rows: u32, dim: u32) -> std::path::PathBuf {
    let mut writer = SegmentWriter::create(dir, id, dim).expect("create writer");
    let mut data = Vec::with_capacity((rows * dim) as usize);
    for row in 0..rows {
        for dim_i in 0..dim {
            data.push(row as f32 + dim_i as f32 * 0.25);
        }
    }
    writer.append_vectors(&data).expect("append vectors");
    writer.commit().expect("commit segment")
}

/// Small deterministic PRNG so the fuzz is reproducible (no flakiness).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(0x2545_F491_4F6C_DD1D)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// Produces a hostile mutation of `base`: bit flips, byte zeroing/overwrite,
/// insertion, removal, and truncation. Every mutation is valid UTF-8-free
/// binary input to the reader.
fn mutate(base: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = base.to_vec();
    let ops = 1 + rng.below(3);
    for _ in 0..ops {
        match rng.below(6) {
            0 if !out.is_empty() => {
                let i = rng.below(out.len() as u64) as usize;
                let bit = rng.below(8) as u8;
                out[i] ^= 1u8 << bit;
            }
            1 if !out.is_empty() => {
                let i = rng.below(out.len() as u64) as usize;
                out[i] = 0;
            }
            2 if !out.is_empty() => {
                let i = rng.below(out.len() as u64) as usize;
                out.remove(i);
            }
            3 => {
                let i = rng.below((out.len() + 1) as u64) as usize;
                out.insert(i, (rng.next() & 0xff) as u8);
            }
            4 if !out.is_empty() => {
                let i = rng.below(out.len() as u64) as usize;
                out[i] = (rng.next() & 0xff) as u8;
            }
            _ if !out.is_empty() => {
                let keep = rng.below(out.len() as u64) as usize;
                out.truncate(keep);
            }
            _ => {}
        }
    }
    out
}

#[test]
fn mutated_segment_corpus_never_panics() {
    let dir = tempdir();
    let good = write_valid_segment(&dir, 1, 8, 4);
    let base = fs::read(&good).expect("read valid segment");
    let target = dir.join("mutated.fseg");

    let mut rng = Rng(0x1111_2222_3333_4444);
    let mut resolved = 0usize;
    const ITERATIONS: usize = 5000;

    for _ in 0..ITERATIONS {
        let mutated = mutate(&base, &mut rng);
        fs::write(&target, &mutated).expect("write mutated bytes");
        // The reader must resolve every hostile input to a Result. A panic here
        // would abort the test, so a passing run proves no panic path.
        if let Ok(reader) = SegmentReader::open(&target) {
            let row_count = reader.header().row_count;
            for _ in 0..4 {
                let row = rng.below(row_count + 4);
                // Out-of-range rows return Err, never panic.
                let _ = reader.vector(row);
            }
        }
        resolved += 1;
    }
    assert_eq!(resolved, ITERATIONS, "every mutated input was resolved");
}

#[test]
fn public_search_api_does_not_panic_under_random_input() {
    let schema = MetadataSchema::new(vec![
        MetadataColumn::new("active".to_string(), ColumnType::Bool),
        MetadataColumn::new("rank".to_string(), ColumnType::I64),
    ])
    .expect("schema");
    let table = TableManager::new()
        .create("v".to_string(), 4, Metric::L2, schema)
        .expect("create table");
    let mut path = WritePath::new(table);
    for id in 0..16u64 {
        let vector = vec![id as f32, (id + 1) as f32, (id * 2) as f32, (id * 3) as f32];
        let metadata = BTreeMap::from([
            ("active".to_string(), MetadataValue::Bool(id % 2 == 0)),
            ("rank".to_string(), MetadataValue::I64(id as i64)),
        ]);
        path.insert(vec![InsertRecord::new(id, vector, metadata)])
            .expect("insert");
    }

    let mut rng = Rng(0xCAFE_BABE_1234_5678);
    for _ in 0..2000 {
        let qlen = 1 + rng.below(8) as usize;
        let query: Vec<f32> = (0..qlen)
            .map(|_| (rng.next() as f32) / u32::MAX as f32)
            .collect();
        let predicate = if rng.below(2) == 0 {
            Some(Predicate::gte(
                "rank".to_string(),
                MetadataValue::I64((rng.below(32) as i64) - 16),
            ))
        } else {
            None
        };
        let top_k = 1 + rng.below(20) as u32;
        let options = SearchOptions::new().with_top_k(top_k).expect("options");
        // No panic: returns Ok or Err (dimension / admission / schema).
        let _ = search(path.delta(), &query, predicate.as_ref(), options);
    }
}

#[test]
fn crash_recovery_is_committed_or_absent() {
    let dir = tempdir();

    // A committed Segment is fully visible and oracle-consistent.
    let path = write_valid_segment(&dir, 1, 8, 4);
    let reader = SegmentReader::open(&path).expect("committed segment opens");
    assert_eq!(reader.header().row_count, 8);
    assert_eq!(reader.all_vectors().len(), 8 * 4);
    for row in 0..8u64 {
        assert_eq!(reader.vector(row).expect("live row").len(), 4);
    }

    // Crash *before* commit: the final path stays absent and the abandoned tmp
    // is never a readable Segment (ADR 0003 — no WAL, commit-or-absent).
    let crash_dir = tempdir();
    {
        let mut writer = SegmentWriter::create(&crash_dir, 99, 4).expect("create writer");
        writer
            .append_vectors(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])
            .expect("append");
        drop(writer); // simulated crash before rename
    }
    assert!(
        !crash_dir.join("99.fseg").exists(),
        "final path must be absent after crash"
    );
    match SegmentReader::open(&crash_dir.join("99.fseg.tmp")) {
        Err(Error::CorruptSegment { .. }) => {}
        other => panic!("abandoned tmp must not open as a Segment, got {other:?}"),
    }

    // Atomic rename guarantees a reader opening the final path never observes
    // a torn/partial file: a freshly committed Segment validates byte-for-byte
    // with a matching payload CRC and full vector block.
    let path2 = write_valid_segment(&dir, 2, 4, 4);
    let reader2 = SegmentReader::open(&path2).expect("open committed");
    assert_eq!(reader2.all_vectors().len(), 4 * 4);
    assert_eq!(reader2.live_row_count(), 4);
}
