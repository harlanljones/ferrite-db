# Synthetic benchmark contract: add a realistic-clustered accuracy corpus

Supersedes the corpus definition in `0006-synthetic-benchmark-contract.md` as amended here.
That ADR remains binding for everything it does not re-open; this record adds a second,
differently-purposed corpus and reconciles the gate contract. Motivation and evidence come
from the FDB-070 campaign report (`docs/baselines/fdb070-campaign.md`) and the G3 frontier
review convening (FDB-G3).

## Change

ADR 0006's synthetic corpus (uniform 10M × 512-d f32 with exact-search ground truth) stays
binding as the **determinism contract** — reproducibility, CI, comparability, and format.
It is no longer sufficient as the sole **accuracy contract**, because on uniform 512-d
vectors the neighbor distances concentrate and no Probe/ef_search setting reaches the ratified
recall target (measured ceiling ≈ 0.56 at any knob; recall ≤ 0.056 for IVF-PQ). The uniform
fixture's job is determinism, not accuracy certification.

A **realistic-clustered** 10M × 512-d f32 corpus with exact-search ground truth is added as
the second contract, and becomes the corpus against which SLO recall gates (ROADMAP §2
recall@10 ≥ 96.5%, floor ≥ 94%) are certified. Both are synthetic, reproducible, and
CI-loadable. SIFT1M remains a comparability sanity run only, never a gate.

## Considered Options

- Keep the uniform corpus as the only accuracy gate: rejected — FDB-070 demonstrated it
  cannot certify recall (clustered-realistic structure is required for graph ANN to show
  meaningful recall headroom).
- Use real customer embeddings for the accuracy contract: rejected — not redistributable,
  not reproducible in CI (same reasoning as ADR 0006).
- Use a published benchmark (SIFT/GIST) as the accuracy gate: rejected — dimensionality
  mismatch (128d/96d vs 512-d design point) means passing it would not validate our regime
  (same reasoning as ADR 0006).

## Consequences

- The benchmark harness (FDB-021) and generator (FDB-020) must produce and consume the
  clustered corpus. A new work item (FDB-024, ROADMAP §9) builds the clustered-corpus
  generator and re-evaluates the G3 recall questions on falsifiable inputs.
- §5 baseline rows are re-grounded against the clustered corpus for recall certification;
  the uniform corpus rows remain for determinism. The contract-scale re-baseline (FDB-023,
  hardware-bound) is unaffected and stays in the v1.1 window.
- Targets (ROADMAP §2) are NOT relaxed. They remain exactly as ratified; this amendment
  grounds them against a corpus that can actually certify them.
