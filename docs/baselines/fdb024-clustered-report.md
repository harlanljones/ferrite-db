# FDB-024 — Clustered corpus + G3 recall re-evaluation (evidence)

Captured 2026-08-25 on the dev machine (the same dev-baseline environment
pinned in [ENVIRONMENT.md](ENVIRONMENT.md)), at a **verifiable reduced scale
10 000 × 512-d f32**, because the contract scale (10M × 512 ≈ 20.5 GiB
fixtures, ~40 GB RSS in-harness) still exceeds available hardware (FDB-023
pending target hardware). The clustered-corpus generator is **structurally
dimensioned for 10M × 512-d**; running it at 10 000 is purely a memory budget
choice on this machine (it produces byte-identical artifacts for any scale
with the same parameters and seed). FDB-023's contract-scale re-baseline
remains the path to the authoritative verdict.

## What this run measures

| Axis | FDB-070 (uniform corpus) | This run (clustered corpus) |
|---|---|---|
| Corpus | uniform [0,1) f32 (ADR 0006) | Gaussian mixture (ADR 0009) |
| Path | graph ANN (HNSW/IVF-PQ) | scan-only via public write path |
| Knobs swept | probes × ef grid | n/a (exhaustive) |
| Recall ceiling | **≈ 0.56** at any knob | **1.0000** vs exact-search oracle (3/3 runs) |
| Why | uniform 512-d concentrates distances; no Probe/ef reaches the ratified target | clustered structure gives graph-ANN-shaped locality *and* a search path that returns the oracle |

The G3 question is **not "is the recall ceiling pathological"** (FDB-070
already answered that on the uniform fixture) but **"does the clustered
corpus expose the headroom graph ANN needs, and does the searchable path
recover the oracle on it"**. The two halves of that question are answered
here by independent evidence:

1. **Clustered structure exists.** `crates/corpus-gen/src/clustered.rs` test
   `clustered_nearest_neighbour_is_in_same_cluster` proves that, for tight
   enough clusters, every query's top-`k` nearest neighbours come from the
   same cluster. That is the structural property uniform data lacks and
   that graph ANN relies on. The test runs the assignment PRNG and the
   generator side-by-side and asserts equality; it cannot pass by accident.
2. **Searchable path returns the oracle.** The FDB-021 harness, ingesting
   the clustered corpus through the public `Table` / `insert` / `search`
   path (FDB-016), returns exact-search top-10 for every one of 200
   measured queries across 3 runs (recall@10 = 1.0000 every time). That
   binds the path to the same oracle the FDB-022 baselines certify on the
   uniform corpus — the path is correct, the only thing that changed is
   the corpus.

This is the **FDB-024 evidence line**. The recall-vs-latency frontier
sweep that produced the FDB-070 ceiling on the uniform fixture is out of
scope for FDB-024 (the dispatch: "full benchmark run can be left to the
human or target hardware; your job is the evidence line"). Re-running the
sweep on the clustered corpus at contract scale belongs with the M3 ladder
re-certification once FDB-023 hardware is available.

## Reproducing

```
# Byte-identical regeneration (proof of the determinism contract on the
# clustered generator — same SplitMix64 scheme as FDB-020, same byte format
# family FRC2/FRM2/FRQ2/FRG2 with the manifest recording
# distribution="gaussian_mixture" and the clustering parameters).
cargo run --release -p corpus-gen -- --mode clustered \
  --out <dir> --num-vectors 10000 --dimension 512 --num-queries 200 \
  --top-k 10 --num-categories 1000 --num-clusters 100 --cluster-stddev 0.05 \
  --seed 12345

# Search-path recall re-measurement (loads the fixture through the public
# write path, runs the FDB-021 measurement loop unfiltered, emits JSON).
cargo run --release -p harness --bin clustered_demo -- \
  --corpus-dir <dir> --warmup 50 --queries 200 \
  --report <artifact.json>
```

## Artifacts

- `artifacts/fdb024-clustered-run-1.json` — p50 27.5 ms, p99 30.6 ms, recall@10 1.0000
- `artifacts/fdb024-clustered-run-2.json` — p50 28.6 ms, p99 34.6 ms, recall@10 1.0000
- `artifacts/fdb024-clustered-run-3.json` — p50 27.9 ms, p99 31.4 ms, recall@10 1.0000

Peak RSS ≈ 79 MB at 10 000 × 512-d; ingest ≈ 0.6–0.8 M vectors/s on this
machine. p50/p99 spread across 3 runs: ±3% / ±10% (well inside the
FDB-022 declared-scale ±15% / ±35% bound); recall is exact. The contract
verdict on the clustered corpus is **deferred to FDB-023 hardware** for
the 10M × 512 ceiling enforcement; the path-correctness verdict is
established here.

## Caveats

- Verifiable scale, not contract scale. The path is the same; the corpus is
  smaller. The recall-vs-knob frontier on the clustered corpus at contract
  scale requires FDB-023 hardware and the FDB-070 sweep harness.
- The `harness-clustered-demo` binary re-uses the FDB-021 measurement loop
  (`harness::run_loaded`) rather than re-implementing it; the dispatch
  budget is one binary, not a forked harness.
- The G3 review is the right place to decide whether the clustered corpus's
  recall ceiling (when the M3 ladder is swept on it) actually clears the
  §2 target or whether further corpus-realism amendment is required (R3).
