# FDB-070 — SLO tuning campaign report (G3 escalation + G4 recommendation)

Campaign executed 2026-08-24 on the pinned dev-baseline environment
(`ENVIRONMENT.md`), declared reduced scale 100k × 512-d Cosine, 200 queries ×
(1 warmup + 2 pooled measured) passes per configuration. Raw artifacts:
`artifacts/fdb070-sweep-{system,mimalloc,jemalloc}.json`.

## 1. Knob sweep results (§2 target pursuit)

Best frontier points per family (full grid in artifacts):

| family | config | p50 ms | p99 ms | recall@10 |
|---|---|---|---|---|
| IvfHnswFlat | probes=1 ef=256 | 3.26 | 5.57 | **0.558** |
| IvfHnswFlat | probes=1 ef=128 | 2.09 | 3.46 | 0.413 |
| IvfHnswFlat | probes=1 ef=64 | 1.67 | 2.78 | 0.296 |
| IvfHnswFlat | probes=1 ef=16 | 1.02 | 1.69 | 0.127 |
| IvfPq | probes=4 | 2.45 | 3.54 | 0.054 |

Findings that shape tuning guidance:
- Recall is governed almost entirely by **ef** on this fixture; probe count
  adds latency without moving recall — calibration's parity rule exploits this.
- **IVF-PQ collapses on uniform-512d data** (recall ≤ 0.056): product
  quantization erases the tiny distance differences of uncorrelated vectors.
  IVF-PQ is the ≥1M ladder choice by design; this fixture cannot validate its
  accuracy end.
- Ladder/calibration knobs behave as designed; no knob setting approaches the
  absolute §2 recall target on this fixture (see §2).

## 2. Targets vs achieved — misses documented, escalated to G3

§2/§5 targets: p50 ≤ 1.2 ms, p99 ≤ 4.5 ms, recall@10 ≥ 96.5% against the
**10M × 512 contract corpus**.

| target | achieved here | verdict |
|---|---|---|
| p50 ≤ 1.2 ms | 1.02–1.75 ms in the useful-recall band | borderline at low-recall corner only |
| p99 ≤ 4.5 ms | 1.69–5.57 ms across frontier | met below ef≈128 |
| recall@10 ≥ 96.5% | ceiling **≈ 0.56** at any knob setting | **MISS — escalated** |

Escalation grounds (R3 realized as predicted):
1. **Uniform synthetic fixtures are pathological for graph ANN** — neighbor
   distances concentrate, so no knob setting reaches high recall. ADR 0006's
   generator is doing its job as a *determinism* contract but cannot certify
   *accuracy* SLOs.
2. **Contract-scale capture remains unavailable** on this machine
   (FDB-023 pending target hardware); §5 numbers are declared-scale.
3. **Ladder↔search wiring is still open**: `search()` serves the exhaustive
   Delta path; ANN results come through the substrate seam directly. Absolute
   SLO verdicts need the integrated pipeline.

**Targets are NOT relaxed.** Per M7/G3 discipline they remain exactly as
ratified; this report documents why they are neither met nor falsifiable here,
and requests the G3 session decide among: realistic-fixture amendment
(clustered generator), contract-scale re-baseline priority (FDB-023), or
ladder-integration scope addition.

## 3. Allocator comparison (U1 / G4 evidence)

Peak RSS and representative frontier latencies:

| allocator | peak RSS | HNSW p50 range | HNSW p99 range | recall impact |
|---|---|---|---|---|
| system | 1403 MB | 1.01–3.86 ms | 1.69–7.31 ms | none |
| mimalloc | 1504 MB | 0.96–4.29 ms | 1.57–7.50 ms | none |
| **jemalloc** | **1047 MB** | **0.78–4.14 ms** | **1.16–8.77 ms** | none |

**Recommendation to G4: ratify `jemalloc`** (feature `jemalloc`) — best or
tied p50/p99 on most configurations and ≈25% lower peak RSS than the default;
recall is allocator-independent (residual spread traces to upstream's
unseeded kmeans between builds). Caveats: one campaign run per allocator on a
shared desktop; reruns should confirm before final sign-off. Features ship
compile-time (`--features jemalloc|mimalloc`, mutually exclusive by compiler).

## 4. Hard-ceiling gates in CI

CI gains an `slo-smoke` job: generates a tiny fixture inline, runs the harness
with `--enforce-ceilings` so any gross latency/recall regression fails the
build (loose tripwire ceilings, documented); authoritative ceiling enforcement
stays bound to recorded environments via the same flag.
