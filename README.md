# Ferrite DB

**Fast, embedded nearest-neighbor search for Rust.**

Ferrite DB is a low-latency vector search library embedded directly into your Rust application. No network calls, no separate service—just synchronous, blocking APIs linked into your process.

- **Sub-millisecond latency** — search without the overhead of serialization or IPC
- **Approximate nearest-neighbor (ANN)** — trade perfect recall for speed; control the tradeoff
- **Metadata filtering** — predicate-based result refinement evaluated during index traversal
- **Zero operational complexity** — no separate database to deploy or manage
- **Production-ready** — designed for predictable, low-latency workloads

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
ferrite-db = "0.1"
```

Create a table, insert vectors, and search:

```rust
use ferrite_db::Table;

// Create a table for 768-dimensional cosine-similarity search
let mut table = Table::new("embeddings", 768, Metric::Cosine)?;

// Insert vectors
table.insert(vec![0.1, 0.2, 0.3, /* ... */])?;

// Search
let results = table.search(&query_vector, SearchOptions::default())?;
for (distance, metadata) in results {
    println!("Nearest neighbor: distance={}", distance);
}
```

## What Ferrite DB is

An **embedded Rust library** — not a service, not a network daemon. Ferrite DB runs in your process, using a library-owned thread pool (via Rayon) for parallelism while keeping the public API simple and synchronous.

### Key Concepts

- **Table** — A named collection of vectors sharing one dimensionality and one metric (Cosine, L2, or Dot).
- **Segment** — An immutable unit of durable storage holding committed vectors.
- **Delta** — Recent inserts buffered until compaction merges them into stable segments.
- **Tombstone** — Marks a deleted vector until background compaction removes it physically.
- **Predicate Tree** — Typed metadata filters evaluated during search traversal.
- **Probe** — One partition of the inverted index examined during search; higher probes mean higher recall but more latency.

## Why Ferrite DB

**Low latency matters.** Networked databases add serialization, kernel switching, and round-trip time. Ferrite DB eliminates all three by living in your process.

**Simplicity compounds.** No separate deployment, no connection pooling, no service restarts. One library, one process, one source of truth.

**Predictability wins.** Synchronous APIs mean no surprise async stack traces. Library-owned threading means no executor wars between your code and the search engine.

## Use Cases

- **Real-time semantic search** — RAG, recommendation engines, instant similarity lookup
- **Embedded analytics** — Vector analytics baked into an application server
- **Local-first AI** — on-device neural search without cloud calls
- **Low-latency filtering** — combine ANN with predicate pushdown for subset search

## Architecture

Ferrite DB is built on clear architectural boundaries:

- **Table management** — Table lifecycle, metadata schema, dimensionality and metric validation
- **Storage** — Immutable segment files, atomic commits via renaming, tombstone bitmaps
- **Write path** — Append-only inserts, delta buffering, auto-chunking, delete-as-tombstone
- **Concurrency** — Per-table single-writer/many-readers coordination with atomic publication snapshots
- **Search** — Exhaustive scanning, predicate evaluation, probe budgeting, and result assembly
- **Index substrate** — The only module depending on upstream libraries (LanceDB); absorbs API churn for the rest of the codebase
- **Compaction** — Background merging of deltas and removal of tombstoned vectors

See `AGENTS.md` and `docs/adr/` for detailed design rationale.

## Status

Ferrite DB is under active development. See `ROADMAP.md` for current priorities and milestones.

## License

Licensed under the MIT OR Apache 2.0 license.

## Contributing

Contributions welcome. Please review `AGENTS.md` for standing development instructions and `docs/adr/` for architectural decisions.

---

**Learn more:** See `CONTEXT.md` for canonical vocabulary, `ROADMAP.md` for the development roadmap, and `docs/adr/` for architecture decision records.