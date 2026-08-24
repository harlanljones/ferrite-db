# Recall and latency gates bind against a synthetic corpus

Recall@10 and latency SLOs are contracts against a reproducible synthetic 10M×512-d corpus with exact-search ground truth. SIFT1M runs only as a comparability sanity check against published results — never as the gate.

## Considered Options

- Standard benchmarks as the gate: rejected — their dimensionalities (128d SIFT, 96d GIST) don't match the 512-d design point, so passing them wouldn't validate our operating regime.
- Real customer embeddings: rejected — not redistributable, not reproducible in CI.

## Consequences

Our headline numbers won't be directly comparable to literature; the SIFT1M sanity run exists to answer exactly that objection.
