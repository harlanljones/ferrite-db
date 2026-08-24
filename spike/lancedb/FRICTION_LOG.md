# FDB-004 LanceDB feasibility spike

## Scope

This is a throwaway, consume-only driver. It is intentionally outside the shipped workspace
and does not import Ferrite DB production modules. It uses a deterministic 512-dimensional
fixture and reports capability facts only; it does not collect or cite performance numbers.

## Results

| Capability | Result | Evidence |
| --- | --- | --- |
| Build an IVF-PQ vector index | Yes | `main.rs` creates `Index::IvfPq` with explicit partition and sub-vector settings. |
| Build an HNSW-backed vector index | Yes | `main.rs` creates `Index::IvfHnswFlat` with explicit partition and construction settings. |
| Query IVF-PQ with a per-query probe knob | Yes | `search` calls `nprobes` on the query builder. |
| Query HNSW with per-query probe and `ef` knobs | Yes | `search` calls `nprobes` and conditionally calls `ef`. |
| Keep an externally managed immutable Segment sidecar beside Lance storage | Yes | The driver writes `segment-0001.fseg` beside the Lance database directory and verifies it survives both table/index workflows. |

## Friction log

- LanceDB's current Rust API exposes HNSW as an IVF-backed family (`IVF_HNSW_FLAT`,
  `IVF_HNSW_PQ`, or `IVF_HNSW_SQ`), rather than as a top-level HNSW index. This satisfies the
  HNSW capability while the integration seam must name the concrete IVF-HNSW variant.
- The spike pins `lancedb` to `0.37.1`, whose published crate declares Rust 1.91 as its minimum;
  this is compatible with Ferrite DB's Rust 1.97 MSRV. Version pinning remains the G-Lance
  decision owned by FDB-030.
- Lance owns only its database directory in this exercise. The sidecar remains a sibling path,
  so Ferrite's atomic Commit and Lance index artifacts have separate ownership boundaries.

## Reproduction

```text
cargo run --manifest-path spike/lancedb/Cargo.toml
```

Expected output includes `capability.hnsw.build=true`, `capability.ivf_pq.build=true`, and
`capability.external_segment_coexistence=true`. No timings are emitted.
