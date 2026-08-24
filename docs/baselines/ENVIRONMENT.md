# Baseline environment spec (G2-entry, pinned)

Pinned by FDB-022 before the first measurement run, per ROADMAP §4 ("measuring first and
pinning the environment afterwards would make the baselines meaningless"). This is the
**declared dev-baseline environment**: the machine below, at a declared reduced corpus scale,
until a target-hardware re-baseline supersedes it (ROADMAP FDB-023).

## Hardware / OS

| Item | Value |
|---|---|
| CPU | Intel Core i7-14700K (28 hardware threads: 8P+12E) |
| RAM | 46 GB total (~22 GB typically available during capture) |
| OS | Linux 7.1.8-arch1-3, x86_64 (Arch Linux) |
| Toolchain | rustc 1.97.1 (MSRV 1.97), release profile (`--release`) |
| Isolation | Best-effort: sequential runs only, no concurrent load; CPU governor not pinned — frequency drift is inside the recorded variance bound |

## Measurement protocol

| Item | Pinned value | Rationale |
|---|---|---|
| Corpus scale (declared deviation) | **100 000 × 512-d f32**, Cosine, `num_categories = 1000`, default seed | Contract scale (10M × 512 ≈ 20.5 GiB fixtures, ~40 GB RSS in-harness) exceeds this machine; reduced scale uses identical code paths and scales linearly. Tracked for re-baseline by ROADMAP FDB-023. |
| Fixture regeneration | `cargo run --release -p corpus-gen -- --out <dir> --num-vectors 100000 --num-queries 500 --top-k 10 --dimension 512 --num-categories 1000` | Byte-identical regeneration guaranteed by FDB-020 determinism; fixtures themselves are not committed. |
| top-k | 10 (§13-3 default) | §5 rows are defined at top-k 10. |
| Caller concurrency | 1 (sequential per-call latency) | §5 measures per-call latency; with admission capacity = 2 × 28 = 56 permits, dispatch is never shed and therefore effectively unthrottled. The FDB-022 "known limitation" text predates FDB-050; admission control now exists but does not engage at this concurrency. |
| Warmup methodology | 50 warmup queries before every measured pass | Cache-state control: identical call shape as measured loop; code/data paths reach steady state before sampling. |
| Query count | All 500 fixture queries per measured pass (p50/p99 over 500 samples) | Percentile stability for the variance bound. |
| Selectivity tiers | 1.0 / 0.1 / 0.01 / 0.001 via category IN-predicates of size round(sel × 1000) | §5 tier definitions. |
| Variance bound derivation | 5 reruns per tier on the pinned environment; per metric, max relative spread across reruns, rounded up to the next 5 points | Recorded alongside values in README.md; validation = any future rerun on this environment must fall within the bound. |

## Known caveats

- Latency figures include the current scan-only search path's per-candidate result materialization
  (vector + metadata clone per visited record); that cost is part of what later milestones improve.
- Peak RSS is captured per harness process (`VmHWM`) and includes fixture buffers plus the Delta copy.
