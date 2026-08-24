# Build on LanceDB's indexes instead of writing our own ANN structures

The design doc specified IVF-PQ and HNSW as though we would implement them, while its Phase 1 said "integrate lancedb". We decided to consume LanceDB's existing IVF-PQ/HNSW implementations over Arrow as the indexing substrate.

## Considered Options

- Custom IVF-PQ/HNSW implementations: rejected — quarters of specialized engineering for a solved problem, delaying every other phase.
- Other embedded backends (usearch, hnswlib bindings): rejected — LanceDB alone provides Arrow-native zero-copy storage plus both index families in one dependency.

## Consequences

Phase 3 narrows to parameter calibration and targeted SIMD kernels only where profiling shows LanceDB falling off the latency-recall frontier; `.lance` becomes an on-disk format we do not control.
