# G2 close-out brief (prepared by FDB-022, 2026-08-24)

**Status: ready to convene — the session itself is Harlan's call.**

## What G2 decides (per ROADMAP)

1. **Accept baselines into §5 against the G2-entry spec.**
   Done and ready for acceptance: §5 TBDs replaced with measured values; artifacts in
   `docs/baselines/artifacts/` (20 runs = 4 tiers × 5 reruns); environment pinned in
   `docs/baselines/ENVIRONMENT.md`; per-metric variance bound derived and recorded in
   `docs/baselines/README.md`. Deviation to ratify or reject: baselines captured at declared
   reduced scale 100k × 512 on the dev machine (contract scale exceeds available RAM);
   contract-scale re-baseline tracked as FDB-023.
2. **Set filtered-recall gate values (U2).**
   Input for the decision: filtered recall@10 is exactly 1.0000 at all four tiers (scan path
   reproduces the filtered oracle bit-for-bit), so gate values can be set without accuracy
   headroom concerns until the index ladder lands.
3. **Confirm ceilings are enforceable in CI.**
   Demonstrated by FDB-021: `--enforce-ceilings` fails the process on violation. Open item:
   ceilings (p50 ≤ 2 ms / p99 ≤ 8 ms) cannot be meaningfully enforced until FDB-023 provides
   contract-scale numbers — at declared scale the scan-only path sits at 141/280 ms.

## Facts the session should note

- Ingest throughput median ≈ 1.15 M vectors/s at declared scale (R4 monitor: corpus loading
  remains practical).
- Variance bounds for ingest (±50 %) and small-tier p99 (±35 %) are wide because the dev
  machine pins neither frequency nor isolation; FDB-023 should tighten both.
- The stale "admission control not yet present" limitation in the FDB-022 deliverable text was
  moot at capture time (FDB-050 landed earlier; sequential dispatch never sheds).
