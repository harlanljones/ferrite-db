# FDB-023 — target-hardware spec and re-baseline runbook

FDB-023 (contract-scale re-baseline, ROADMAP §9) is blocked on one dependency: **target benchmark
hardware**. This document fixes what that machine must provide so the campaign can execute
immediately when it lands, using the FDB-021 harness unchanged.

## Why the dev-baseline machine cannot capture contract scale

RSS scaling of the harness was probed empirically on the pinned dev environment
(i7-14700K / 46 GB RAM) at four corpus scales, synthetic 512-d, selectivity 1.0, top-k 10:

| Corpus scale | Peak RSS (`VmHWM`) | p50 ms | p99 ms | Recall@10 | Artifact |
|---|---|---|---|---|---|
| 100 000 | 758.8 MB | 176.4 | 252.8 | 1.000000 | `artifacts/fdb023-rss-scaling-100000.json` |
| 250 000 | 1885.1 MB | 308.9 | 361.6 | 1.000000 | `artifacts/fdb023-rss-scaling-250000.json` |
| 500 000 | 3764.5 MB | 663.2 | 1272.7 | 1.000000 | `artifacts/fdb023-rss-scaling-500000.json` |
| 1 000 000 | 7525.9 MB | 1622.2 | 2047.3 | 1.000000 | `artifacts/fdb023-rss-scaling-1000000.json` |

Peak RSS is linear in corpus size at **≈ 7 517 bytes/vector** (successive-interval slopes
7 510 / 7 518 / 7 523 B/vec; intercept ≈ 7 MB; the 100k point reproduces FDB-022's recorded
761 MB baseline). Extrapolated contract-scale working set:

> **10M × 512 ⇒ ≈ 75 GB (≈ 70 GiB) peak RSS.**

This supersedes the earlier ~40 GB paper estimate recorded in `ENVIRONMENT.md` — the real
figure is close to **2×** that. Probes are *scaling measurements taken on a loaded desktop*;
they are not baselines and change nothing in §5.

## Target-machine requirements

| Item | Requirement | Basis |
|---|---|---|
| RAM | **≥ 80 GB available to the harness process**; 96–128 GB machine class recommended | 70 GiB extrapolated peak RSS + OS/page-cache headroom |
| Scratch disk | ≥ 32 GB free NVMe-class storage | fixtures ≈ 20 GB (measured 2.0 GB per 1M vectors) + reports/artifacts |
| CPU frequency | Pinned: `performance` governor, turbo disabled or capped (e.g. `cpupower frequency-set -g performance`; cap via `intel_pstate=passive` + `<freq>` max, or boot-time limit) | variance bound must tighten under pinned-frequency isolation (FDB-023 deliverable) |
| Isolation | Sequential runs only; no interactive/desktop load during passes | same |
| Cores | Secondary; the capture protocol runs caller-concurrency 1, so single-thread frequency stability dominates | ENVIRONMENT.md protocol |

## Re-baseline procedure (harness and generator unchanged)

1. `cargo build --release` on the MSRV toolchain (1.97).
2. Generate contract fixtures:
   `corpus-gen --out <dir> --num-vectors 10000000 --num-queries 500 --top-k 10 --dimension 512 --num-categories 1000`
   (byte-identical regeneration guaranteed by FDB-020 determinism).
3. Capture all §5 tiers exactly per the FDB-022 protocol in [README.md](README.md)
   (warmup 50, all 500 queries, selectivity 1.0/0.1/0.01/0.001). Suggested rerun count: 3,
   tightened to more only if spread exceeds the target bound.
4. Derive and record the reproducibility-variance bound as at FDB-022; refresh
   `docs/baselines/` (this directory) with contract-scale values and artifacts.
5. Evaluate the hard-ceiling gates (p50 ≤ 2 ms / p99 ≤ 8 ms / recall ≥ 94%) against the new
   §5 cells for G2 acceptance.

## Wall-clock budget (unmeasured beyond the slope; plan accordingly)

Latency also scales ~linearly here (p50 ×9.2 for ×10 vectors, memory-bandwidth bound):
extrapolated ≈ **16–19 s/query at 10M scale** on a comparable-frequency part. One full pass
(50 warmup + 500 measured) ≈ 2.5–3 h; the 4-tier × 3-rerun protocol ≈ 30–36 h of pure scan
time plus ingest (~10M at ≈ 1 M vectors/s ≈ minutes). Budget multiple days including fixture
generation, artifact review, and reruns. These figures are extrapolations from the probes
above, labeled unmeasured until the campaign runs.
