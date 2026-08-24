# First baselines (FDB-022)

Established 2026-08-24 on the environment pinned in [ENVIRONMENT.md](ENVIRONMENT.md), using the
FDB-021 harness against FDB-020 fixtures regenerated at the **declared reduced scale
100 000 × 512-d** (see ENVIRONMENT.md for why, and ROADMAP FDB-023 for the contract-scale
re-baseline). All numbers below are **medians of 5 reruns**; raw artifacts live in
[artifacts/](artifacts/) (`tier-<sel>-run-<n>.json`, machine-readable).

## Headline results

| Metric (top-k = 10) | Value (median, n=5) | Artifacts |
|---|---|---|
| p50 search latency, unfiltered | **141.5 ms** | `artifacts/tier-1.0-run-{1..5}.json` |
| p99 search latency, unfiltered | **279.7 ms** | same |
| Recall@10, unfiltered | **1.0000** (exact scan reproduces the oracle bit-for-bit) | same |
| Recall@10 @ selectivity 0.1 / 0.01 / 0.001 | **1.0000 / 1.0000 / 1.0000** vs filtered oracle | `artifacts/tier-0.1-*`, `tier-0.01-*`, `tier-0.001-*` |
| p50 / p99 @ selectivity 0.1 | 19.2 ms / 41.3 ms | `artifacts/tier-0.1-run-{1..5}.json` |
| p50 / p99 @ selectivity 0.01 | 5.0 ms / 10.8 ms | `artifacts/tier-0.01-run-{1..5}.json` |
| p50 / p99 @ selectivity 0.001 | 3.7 ms / 9.7 ms | `artifacts/tier-0.001-run-{1..5}.json` |
| Ingest throughput | **≈ 1.15 M vectors/s** (per-tier medians 0.97–1.17 M) | all artifacts (`ingest.throughput_vectors_per_sec`) |
| Peak RSS, unfiltered pass | **≈ 761 MB** (`VmHWM`; includes fixture buffers + Delta copy + per-query result materialization) | `artifacts/tier-1.0-run-{1..5}.json` |
| Peak RSS, filtered passes | ≈ 481–508 MB (predicate filtering happens before per-candidate cloning, so the transient clone peak shrinks) | filtered artifacts |

Latency figures are dominated by the current scan-only path's per-candidate result
materialization (vector + metadata cloned for every visited record before truncation); that cost
is part of what the Wave-5 index ladder improves.

## Recorded reproducibility-variance bound

Validation rule: a rerun on the pinned environment reproduces these baselines iff **all** of:

| Metric | Bound | Derivation (observed across the 20 captured runs) |
|---|---|---|
| Recall@10 (all tiers) | **exact equality** | 1.0000 in every run; deterministic oracle agreement |
| Peak RSS | **±1 %** | < 0.03 % spread within every configuration |
| p50 latency | **±15 %** | max deviation from per-tier median observed: 7.6 % |
| p99 latency | **±35 %** | 11.2 % at the SLO-relevant unfiltered tier; up to 33 % at sub-10 ms tiers where single scheduler spikes dominate index-495 samples |
| Ingest throughput | **±50 %** | per-tier medians stable, but isolated outliers to −43.5 % (transient desktop contention: two mid-sequence dips to ≈ 0.65 M/s) |

Bounds are observed-worst-case rounded up to a working margin. They are deliberately wide for
ingest/p99 because this environment pins neither CPU frequency nor isolation; FDB-023's
target-hardware re-baseline is expected to tighten them.

## Reproducing

```
cargo build --release
cargo run --release -p corpus-gen -- --out <fixtures-dir> \
  --num-vectors 100000 --num-queries 500 --top-k 10 --dimension 512 --num-categories 1000
for sel in 1.0 0.1 0.01 0.001; do
  ./target/release/harness --corpus-dir <fixtures-dir> --warmup 50 \
    --selectivity $sel --report tier-$sel-run-N.json
done
```

Fixtures are not committed; corpus-gen regeneration is byte-identical by construction (FDB-020).

## FDB-032 — calibrated vs naive fixed knobs (ANN evidence run)

`artifacts/fdb032-ann-compare.json` records the M3 comparison on the identical 100k × 512-d
fixture: HNSW (partitions=4, ef_construction=64), 500 queries, 5 interleaved A/B passes,
pooled medians. Verdict **PARETO_DOMINATES=true** — calibration selected probes=1/ef=64
against the naive probes=4/ef=64 default at identical recall@10 with lower p50 and p99.
Runner: `cargo run --release -p harness --bin ann_compare -- --corpus-dir <fixtures> --rows
100000 --queries 500`. Caveats recorded in ROADMAP §9 FDB-032 (pathological uniform data;
upstream unseeded kmeans ⇒ knob choice can vary across index rebuilds).
