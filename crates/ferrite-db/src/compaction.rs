//! Lifecycle/Compaction — background merge absorbing Deltas and discarding
//! Tombstoned vectors; manual escape hatch (AGENTS.md §4). Owned by
//! ROADMAP FDB-040.
//!
//! ## Ratified triggers (M4)
//! - accumulated-change ≥ **max(1% of Table rows, 100k)**, or
//! - **≥ 4** unmerged Deltas, or
//! - Tombstone ratio > **20%** gates physical vector removal at merge time.
//!
//! §13 design-time interpretation (recorded here because the deliverable
//! demands it be confirmed): *accumulated change* counts rows living in
//! sealed Deltas plus known Tombstoned ids — everything a reader would lose
//! if history were collapsed. The Table-row base is the current total row
//! count. Physical removal at merge keeps every id's newest *visible*
//! version; when the ratio is at or below 20% the merged Segment also
//! physically retains each fully-Tombstoned id's newest hidden row (hidden
//! through `delete`, which the public Delta API can express), while
//! superseded dead generations necessarily collapse during any rebuild.
//! This reading was forced by the public Delta surface and is flagged for
//! Harlan's review in the FDB-040 resolution.
//!
//! Composition: [`Lifecycle`] wraps one [`TableCoordinator<Delta>`] — readers
//! clone immutable snapshots and search them; writers clone-mutate-commit
//! under the writer lock (ADR 0005: exactly one writer); background
//! compaction runs on its own thread and publishes atomically like any other
//! Commit (ADR 0003 semantics preserved: merges never tear).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::concurrency::TableCoordinator;
use crate::errors::Result;
use crate::search::{SearchOptions, SearchResult, search};
use crate::table::Table;
use crate::write_path::{Delta, InsertRecord};

/// Accumulated-change fraction of total Table rows that arms the trigger.
pub const COMPACTION_CHANGE_FRACTION: f64 = 0.01;
/// Absolute accumulated-change floor of the trigger ("max(1%, this)").
pub const COMPACTION_MIN_CHANGED_ROWS: u64 = 100_000;
/// Unmerged Delta-segment count that arms the trigger independently.
pub const COMPACTION_DELTA_COUNT_TRIGGER: usize = 4;
/// Tombstone-to-row ratio above which merge physically drops hidden vectors.
pub const TOMBSTONE_PURGE_RATIO: f64 = 0.20;

/// Tunable thresholds. The defaults encode the ratified M4 numbers; crafted
/// tests shrink them to reach boundaries quickly.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    pub change_fraction: f64,
    pub min_changed_rows: u64,
    pub delta_count_trigger: usize,
    pub purge_ratio: f64,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            change_fraction: COMPACTION_CHANGE_FRACTION,
            min_changed_rows: COMPACTION_MIN_CHANGED_ROWS,
            delta_count_trigger: COMPACTION_DELTA_COUNT_TRIGGER,
            purge_ratio: TOMBSTONE_PURGE_RATIO,
        }
    }
}

/// Why a compaction ran (or that none was needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// Explicit `compact()` call — always runs regardless of thresholds.
    Manual,
    /// Accumulated change reached max(fraction × rows, floor).
    ChangeThreshold,
    /// Sealed Delta-segment count reached the configured trigger.
    DeltaCount,
    /// Thresholds not met (manual calls ignore this and run anyway).
    None,
}

/// Outcome statistics of one compaction.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionReport {
    pub trigger: TriggerKind,
    pub segments_before: usize,
    pub segments_after: usize,
    pub rows_before: usize,
    pub rows_after: usize,
    /// Dead rows physically dropped by the purge path (>20% ratio).
    pub tombstones_purged: usize,
    /// Fully-hidden ids physically retained below the purge ratio.
    pub tombstones_retained: usize,
}

/// Pure trigger evaluation over a Delta snapshot.
pub fn evaluate_trigger(config: &CompactionConfig, delta: &Delta) -> TriggerKind {
    let sealed_rows: u64 = delta
        .sealed_segments()
        .iter()
        .map(|segment| segment.len() as u64)
        .sum();
    let changed = sealed_rows + delta.tombstoned_ids().count() as u64;
    let fraction_rows = (delta.len() as f64 * config.change_fraction) as u64;
    let threshold = fraction_rows.max(config.min_changed_rows);
    if changed >= threshold {
        TriggerKind::ChangeThreshold
    } else if delta.sealed_segments().len() >= config.delta_count_trigger {
        TriggerKind::DeltaCount
    } else {
        TriggerKind::None
    }
}

/// Whether the thresholds arm an automatic compaction for this snapshot.
pub fn should_compact(config: &CompactionConfig, delta: &Delta) -> bool {
    evaluate_trigger(config, delta) != TriggerKind::None
}

/// Merges one snapshot into a fresh Delta: absorbs sealed Deltas and the
/// active set into a single rebuilt lineage, keeping each id's newest visible
/// version. Above the purge ratio, fully-tombstoned vectors are dropped;
/// at or below it they are carried hidden so the 20% gate remains observable.
///
/// Returns the merged Delta plus the counts needed for reporting.
pub fn merge(delta: &Delta) -> Result<(Delta, CompactionReport)> {
    // Classify records in insertion order (sealed, then active): the last
    // occurrence of an id wins; generations only ever increase within one
    // Delta lineage.
    let mut latest_visible: BTreeMap<u64, &InsertRecord> = BTreeMap::new();
    let mut latest_hidden: BTreeMap<u64, &InsertRecord> = BTreeMap::new();
    let mut dead_rows = 0usize;
    for record in delta.records() {
        if delta.is_record_tombstoned(record) {
            dead_rows += 1;
            latest_hidden.insert(record.id(), record);
        } else {
            latest_visible.insert(record.id(), record);
            latest_hidden.remove(&record.id());
        }
    }

    let total_rows = delta.len();
    let ratio = if total_rows == 0 {
        0.0
    } else {
        dead_rows as f64 / total_rows as f64
    };
    let purge = ratio > TOMBSTONE_PURGE_RATIO;

    let mut merged = Delta::new(delta.table().clone());
    let carried: Vec<&InsertRecord> = latest_visible.values().copied().collect();
    let batch_size = 4096;
    let flush = |batch: &mut Vec<InsertRecord>, merged: &mut Delta| -> Result<()> {
        if !batch.is_empty() {
            let taken = std::mem::take(batch);
            merged.insert(taken)?;
        }
        Ok(())
    };
    let mut batch: Vec<InsertRecord> = Vec::with_capacity(batch_size);
    for record in carried {
        batch.push(InsertRecord::new(
            record.id(),
            record.vector().to_vec(),
            record.metadata().clone(),
        ));
        if batch.len() == batch_size {
            flush(&mut batch, &mut merged)?;
        }
    }

    let mut retained_ids: Vec<u64> = Vec::new();
    if !purge {
        // Carry each fully-hidden id's newest row, then hide it again in the
        // rebuilt lineage so visibility semantics are unchanged.
        for (&id, record) in &latest_hidden {
            merged.insert(vec![InsertRecord::new(
                id,
                record.vector().to_vec(),
                record.metadata().clone(),
            )])?;
            retained_ids.push(id);
        }
        if !retained_ids.is_empty() {
            merged.delete(&retained_ids)?;
        }
    }
    flush(&mut batch, &mut merged)?;

    let report = CompactionReport {
        trigger: TriggerKind::None,
        segments_before: delta.sealed_segments().len(),
        segments_after: merged.sealed_segments().len(),
        rows_before: total_rows,
        rows_after: merged.len(),
        tombstones_purged: if purge { dead_rows } else { 0 },
        tombstones_retained: if purge { 0 } else { retained_ids.len() },
    };
    Ok((merged, report))
}

/// Per-Table lifecycle: single-writer ingestion, many-reader search, manual
/// and background Compaction — all through atomic publication snapshots.
#[derive(Debug)]
pub struct Lifecycle {
    coordinator: Arc<TableCoordinator<Delta>>,
    config: CompactionConfig,
    last_report: Arc<std::sync::Mutex<Option<CompactionReport>>>,
}

impl Lifecycle {
    /// Creates a lifecycle around an empty Table lineage.
    pub fn new(table: Table) -> Result<Self> {
        Self::with_config(table, CompactionConfig::default())
    }

    /// Creates a lifecycle with crafted thresholds (tests, tuned Tables).
    pub fn with_config(table: Table, config: CompactionConfig) -> Result<Self> {
        Ok(Self {
            coordinator: Arc::new(TableCoordinator::new(Delta::new(table))?),
            config,
            last_report: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// The currently published snapshot (readers may hold it freely).
    pub fn snapshot(&self) -> Result<Arc<Delta>> {
        self.coordinator.snapshot()
    }

    /// Writer-side insert: clone-mutate-commit under the writer lock.
    ///
    /// Single-writer contract (ADR 0005): callers must not interleave this
    /// with `compact()` from another thread.
    pub fn insert(&self, records: Vec<InsertRecord>) -> Result<()> {
        let snapshot = self.coordinator.snapshot()?;
        let mut next = (*snapshot).clone();
        next.insert(records)?;
        self.coordinator.commit(next)
    }

    /// Writer-side delete-as-Tombstone.
    pub fn delete(&self, ids: &[u64]) -> Result<()> {
        let snapshot = self.coordinator.snapshot()?;
        let mut next = (*snapshot).clone();
        next.delete(ids)?;
        self.coordinator.commit(next)
    }

    /// Reader-side exhaustive search over the published snapshot. Readers
    /// never block on writers and observe only complete pre- or post-merge
    /// states.
    pub fn search(
        &self,
        query: &[f32],
        predicate: Option<&crate::search::Predicate>,
        options: SearchOptions,
    ) -> Result<Vec<SearchResult>> {
        let snapshot = self.coordinator.snapshot()?;
        search(&snapshot, query, predicate, options)
    }

    /// Whether the automatic thresholds are armed on the current snapshot.
    pub fn compaction_needed(&self) -> Result<bool> {
        Ok(should_compact(&self.config, self.snapshot()?.as_ref()))
    }

    /// Manual escape hatch: merges regardless of thresholds. Idempotent —
    /// merging an already-merged lineage reproduces the same visible state
    /// and reports zero segments absorbed.
    pub fn compact(&self) -> Result<CompactionReport> {
        self.compact_with_trigger(TriggerKind::Manual)
    }

    fn compact_with_trigger(&self, trigger: TriggerKind) -> Result<CompactionReport> {
        let snapshot = self.coordinator.snapshot()?;
        let effective = match trigger {
            TriggerKind::Manual => TriggerKind::Manual,
            _ => evaluate_trigger(&self.config, &snapshot),
        };
        let (merged, mut report) = merge(&snapshot)?;
        report.trigger = effective;
        report.segments_before = snapshot.sealed_segments().len();
        report.rows_before = snapshot.len();
        self.coordinator.commit(merged)?;
        if let Ok(mut slot) = self.last_report.lock() {
            *slot = Some(report.clone());
        }
        Ok(report)
    }

    /// The most recent completed compaction, manual or background.
    /// Observability for the background path.
    pub fn last_compaction(&self) -> Option<CompactionReport> {
        self.last_report.lock().ok().and_then(|slot| slot.clone())
    }

    fn maybe_compact_background(&self) -> Result<Option<CompactionReport>> {
        let snapshot = self.coordinator.snapshot()?;
        let trigger = evaluate_trigger(&self.config, &snapshot);
        if trigger == TriggerKind::None {
            return Ok(None);
        }
        Ok(Some(self.compact_with_trigger(trigger)?))
    }

    /// Spawns the background per-Table job: wakes on `interval`, evaluates
    /// the triggers against the published snapshot, and compacts when armed.
    pub fn spawn_background(
        self: &Arc<Self>,
        interval: Duration,
    ) -> std::io::Result<BackgroundCompaction> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Arc::clone(self);
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("ferrite-compaction".to_string())
            .spawn(move || {
                while !stop_flag.load(Ordering::Acquire) {
                    // Sleep in slices so stop requests land promptly.
                    let slice = Duration::from_millis(10);
                    let mut waited = Duration::ZERO;
                    while waited < interval && !stop_flag.load(Ordering::Acquire) {
                        std::thread::sleep(slice.min(interval - waited));
                        waited += slice;
                    }
                    if stop_flag.load(Ordering::Acquire) {
                        break;
                    }
                    // A failed cycle is retried on the next wake; background
                    // compaction never panics the process on operational
                    // errors.
                    let _ = worker.maybe_compact_background();
                }
            })?;
        Ok(BackgroundCompaction {
            stop,
            thread: Some(handle),
        })
    }
}

/// Handle joining/stopping the background per-Table compaction job.
#[derive(Debug)]
pub struct BackgroundCompaction {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl BackgroundCompaction {
    /// Signals stop and joins the worker.
    pub fn stop(mut self) -> std::thread::Result<()> {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            handle.join()?;
        }
        Ok(())
    }
}

impl Drop for BackgroundCompaction {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchOptions;
    use crate::table::{ColumnType, MetadataColumn, MetadataSchema, Metric, TableManager};
    use crate::write_path::MetadataValue;
    use std::collections::BTreeMap;

    const DIM: u32 = 8;

    fn schema() -> crate::table::MetadataSchema {
        MetadataSchema::new(vec![MetadataColumn::new(
            "category".to_string(),
            ColumnType::I64,
        )])
        .unwrap()
    }

    fn record(id: u64, seed: u64) -> InsertRecord {
        let vector: Vec<f32> = (0..u64::from(DIM))
            .map(|col| ((id * 31 + col * 7 + seed) % 89) as f32 / 89.0)
            .collect();
        let mut metadata = BTreeMap::new();
        metadata.insert("category".to_string(), MetadataValue::I64((id % 7) as i64));
        InsertRecord::new(id, vector, metadata)
    }

    fn tiny_config() -> CompactionConfig {
        CompactionConfig {
            change_fraction: 1.1, // effectively disabled unless overridden
            min_changed_rows: u64::MAX,
            delta_count_trigger: usize::MAX,
            purge_ratio: TOMBSTONE_PURGE_RATIO,
        }
    }

    fn oracle_top_k(snapshot: &Delta, query: &[f32], k: usize) -> Vec<u64> {
        let mut scored: Vec<(f32, u64)> = snapshot
            .records()
            .filter(|record| !snapshot.is_record_tombstoned(record))
            .map(|record| {
                (
                    crate::search::distance(Metric::Cosine, query, record.vector()),
                    record.id(),
                )
            })
            .collect();
        scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        scored.into_iter().take(k).map(|(_, id)| id).collect()
    }

    #[test]
    fn trigger_fires_at_change_threshold_boundaries() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        // Threshold formula: max(change_fraction x rows, min_changed_rows).
        // With 100 rows and fraction 0.9 the formula boundary sits at 90.
        let mut config = tiny_config();
        config.change_fraction = 0.9;
        config.min_changed_rows = 10;

        let mut delta = Delta::new(table);
        delta
            .insert((0..100).map(|id| record(id, 0)).collect())
            .unwrap();

        // 89 tombstoned ids: changed = 89 < 90 → quiet.
        delta.delete(&(0..89).collect::<Vec<_>>()).unwrap();
        assert_eq!(evaluate_trigger(&config, &delta), TriggerKind::None);

        // One more: changed = 90 = threshold → armed (>=).
        delta.delete(&[89]).unwrap();
        assert_eq!(
            evaluate_trigger(&config, &delta),
            TriggerKind::ChangeThreshold
        );

        // Absolute-floor arm: a fresh lineage where the floor exceeds any
        // reachable fraction product arms purely off tombstone count.
        let table = TableManager::new()
            .create("t2".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let mut config = tiny_config();
        config.change_fraction = 0.001;
        config.min_changed_rows = 30;
        let mut delta = Delta::new(table);
        delta
            .insert((0..40).map(|id| record(id, 1)).collect())
            .unwrap();
        delta.delete(&(0..29).collect::<Vec<_>>()).unwrap(); // 29 < 30
        assert_eq!(evaluate_trigger(&config, &delta), TriggerKind::None);
        delta.delete(&[29]).unwrap(); // 30 = floor → armed
        assert_eq!(
            evaluate_trigger(&config, &delta),
            TriggerKind::ChangeThreshold
        );
    }

    #[test]
    fn trigger_fires_at_delta_count_boundary() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let mut config = tiny_config();
        config.change_fraction = 1.1;
        config.min_changed_rows = u64::MAX;
        config.delta_count_trigger = 4;

        let mut delta = Delta::new(table);
        let per_segment = crate::write_path::TARGET_SEGMENT_ROWS;
        for generation in 0..3u64 {
            delta
                .insert(
                    (generation * per_segment as u64..(generation + 1) * per_segment as u64)
                        .map(|id| record(id, generation))
                        .collect(),
                )
                .unwrap();
        }
        assert_eq!(delta.sealed_segments().len(), 3);
        assert_eq!(evaluate_trigger(&config, &delta), TriggerKind::None);

        delta
            .insert(
                (3 * per_segment as u64..4 * per_segment as u64)
                    .map(|id| record(id, 9))
                    .collect(),
            )
            .unwrap();
        assert_eq!(delta.sealed_segments().len(), 4);
        assert_eq!(evaluate_trigger(&config, &delta), TriggerKind::DeltaCount);
    }

    #[test]
    fn merge_is_oracle_equivalent_across_deletes_and_updates() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let lifecycle = Lifecycle::new(table).unwrap();

        lifecycle
            .insert((0..300).map(|id| record(id, 0)).collect())
            .unwrap();
        lifecycle.delete(&[5, 17, 250]).unwrap();
        lifecycle
            .insert(vec![record(42, 99), record(301, 7)])
            .unwrap(); // updates + append

        let query: Vec<f32> = (0..DIM).map(|_| 0.4f32).collect();
        let options = SearchOptions::new().with_top_k(10).unwrap();
        let before = lifecycle.search(&query, None, options).unwrap();
        let expected = oracle_top_k(&lifecycle.snapshot().unwrap(), &query, 10);

        let report = lifecycle.compact().unwrap();
        assert_eq!(report.trigger, TriggerKind::Manual);

        let after = lifecycle.search(&query, None, options).unwrap();
        let ids_before: Vec<u64> = before.iter().map(|hit| hit.id()).collect();
        let ids_after: Vec<u64> = after.iter().map(|hit| hit.id()).collect();
        assert_eq!(
            ids_before, expected,
            "pre-merge results drifted from oracle"
        );
        assert_eq!(
            ids_after, expected,
            "post-merge results drifted from oracle"
        );
        // Deleted ids stay gone; updated id reflects the new seed vector.
        assert!(!ids_after.contains(&5) && !ids_after.contains(&17) && !ids_after.contains(&250));

        // Row accounting: 302 input records → 298 newest-visible ids plus the
        // 3 fully-tombstoned ids physically retained below the 20% gate.
        assert_eq!(report.rows_before, 302);
        assert_eq!(report.rows_after, 301);
        assert_eq!(report.tombstones_retained, 3);
        assert_eq!(report.tombstones_purged, 0);
    }

    #[test]
    fn purge_gates_physical_removal_at_twenty_percent() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();

        // 100 rows, delete 19 → ratio 19% ≤ 20%: retained hidden rows.
        let mut delta = Delta::new(table.clone());
        delta
            .insert((0..100).map(|id| record(id, 0)).collect())
            .unwrap();
        delta.delete(&(0..19).collect::<Vec<_>>()).unwrap();
        let (merged_below, report_below) = merge(&delta).unwrap();
        assert_eq!(report_below.tombstones_retained, 19);
        assert_eq!(report_below.tombstones_purged, 0);
        assert_eq!(
            merged_below.len(),
            100,
            "hidden rows are physically retained"
        );
        // Visibility check: deleted ids invisible in the merged lineage.
        let options = SearchOptions::new().with_top_k(1000).unwrap();
        let probe: Vec<f32> = (0..DIM).map(|_| 0.5f32).collect();
        let hits = crate::search::search(&merged_below, &probe, None, options).unwrap();
        assert!(hits.iter().all(|hit| hit.id() >= 19));

        // Delete one more → 20% exactly: still ≤ gate (strictly greater fires).
        delta.delete(&[19]).unwrap();
        let (_, report_at_gate) = merge(&delta).unwrap();
        assert_eq!(report_at_gate.tombstones_retained, 20);

        // 21% > 20%: purge path removes hidden rows entirely.
        delta.delete(&[20]).unwrap();
        let (merged_above, report_above) = merge(&delta).unwrap();
        assert_eq!(report_above.tombstones_purged, 21);
        assert_eq!(report_above.tombstones_retained, 0);
        assert_eq!(merged_above.len(), 79);
        let hits = crate::search::search(&merged_above, &probe, None, options).unwrap();
        assert!(hits.iter().all(|hit| hit.id() > 20));
    }

    #[test]
    fn compact_is_idempotent() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let lifecycle = Lifecycle::with_config(table, tiny_config()).unwrap();
        lifecycle
            .insert((0..50).map(|id| record(id, 1)).collect())
            .unwrap();
        lifecycle.delete(&[3, 4]).unwrap();

        let first = lifecycle.compact().unwrap();
        let rows_after_first = lifecycle.snapshot().unwrap().len();
        let second = lifecycle.compact().unwrap();

        assert_eq!(lifecycle.snapshot().unwrap().len(), rows_after_first);
        assert_eq!(second.rows_after, first.rows_after);
        assert_eq!(second.segments_before, first.segments_after);
        let query: Vec<f32> = (0..DIM).map(|_| 0.25f32).collect();
        let options = SearchOptions::new().with_top_k(5).unwrap();
        let hits = lifecycle.search(&query, None, options).unwrap();
        assert_eq!(
            hits.first().expect("results").id(),
            oracle_top_k(&lifecycle.snapshot().unwrap(), &query, 5)[0]
        );
    }

    #[test]
    fn searches_stay_oracle_consistent_during_merge() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let lifecycle = Arc::new(Lifecycle::with_config(table, tiny_config()).unwrap());
        lifecycle
            .insert((0..400).map(|id| record(id, 0)).collect())
            .unwrap();
        lifecycle.delete(&[7, 70, 140, 210]).unwrap();

        let query: Vec<f32> = (0..DIM).map(|_| 0.33f32).collect();
        let expected = oracle_top_k(&lifecycle.snapshot().unwrap(), &query, 10);
        let stop = Arc::new(AtomicBool::new(false));

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let lifecycle = Arc::clone(&lifecycle);
                let query = query.clone();
                let expected = expected.clone();
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let options = SearchOptions::new().with_top_k(10).unwrap();
                    while !stop.load(Ordering::Acquire) {
                        let hits = lifecycle.search(&query, None, options).unwrap();
                        let ids: Vec<u64> = hits.iter().map(|hit| hit.id()).collect();
                        // Every observation must equal the shared oracle: the
                        // merge preserves visible state exactly, so there is
                        // no intermediate world to observe.
                        assert_eq!(ids, expected, "inconsistent view during compaction");
                    }
                })
            })
            .collect();

        let report = lifecycle.compact().unwrap();
        stop.store(true, Ordering::Release);
        for reader in readers {
            reader.join().unwrap();
        }
        assert_eq!(report.trigger, TriggerKind::Manual);
        assert_eq!(
            oracle_top_k(&lifecycle.snapshot().unwrap(), &query, 10),
            expected,
            "oracle drifted across merge"
        );
    }

    #[test]
    fn background_job_compacts_when_armed_and_stops_cleanly() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        // Floor below one sealed Delta so a single seal arms the trigger.
        let mut config = tiny_config();
        config.change_fraction = 0.01;
        config.min_changed_rows = (crate::write_path::TARGET_SEGMENT_ROWS as u64) / 2;
        let lifecycle = Arc::new(Lifecycle::with_config(table, config).unwrap());

        let background = lifecycle
            .spawn_background(Duration::from_millis(15))
            .unwrap();

        let per_segment = crate::write_path::TARGET_SEGMENT_ROWS as u64;
        lifecycle
            .insert((0..=per_segment).map(|id| record(id, 2)).collect())
            .unwrap();
        assert_eq!(lifecycle.snapshot().unwrap().sealed_segments().len(), 1);
        assert!(lifecycle.compaction_needed().unwrap());

        // The job must record a background compaction within its wakeups.
        // (The merged output legitimately re-seals — TARGET_SEGMENT_ROWS
        // chunking applies to any large lineage — so sealed-count is not an
        // observable; the report history is.)
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match lifecycle.last_compaction() {
                Some(report) => {
                    assert_eq!(report.trigger, TriggerKind::ChangeThreshold);
                    break;
                }
                None => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "background compaction never ran"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        background.stop().unwrap();

        // Visible state survived the background merge untouched.
        let query: Vec<f32> = (0..DIM).map(|_| 0.6f32).collect();
        let options = SearchOptions::new().with_top_k(5).unwrap();
        let hits = lifecycle.search(&query, None, options).unwrap();
        assert_eq!(
            hits.iter().map(|hit| hit.id()).collect::<Vec<_>>(),
            oracle_top_k(&lifecycle.snapshot().unwrap(), &query, 5)
        );
    }

    #[test]
    fn empty_lifecycle_behaves() {
        let table = TableManager::new()
            .create("t".to_string(), DIM, Metric::Cosine, schema())
            .unwrap();
        let lifecycle = Lifecycle::new(table).unwrap();
        assert!(!lifecycle.compaction_needed().unwrap());
        let report = lifecycle.compact().unwrap();
        assert_eq!(report.rows_before, 0);
        assert_eq!(report.rows_after, 0);
        let query: Vec<f32> = vec![0.0; DIM as usize];
        let options = SearchOptions::new().with_top_k(3).unwrap();
        assert!(lifecycle.search(&query, None, options).unwrap().is_empty());
        // Unknown-id deletes succeed-and-ignore (§13-1).
        lifecycle.delete(&[999]).unwrap();
    }
}
