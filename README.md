# Ferrite DB

**Fast, embedded nearest-neighbor search for Rust.**

Ferrite DB is a low-latency vector search library embedded directly into your Rust application. No network calls, no separate service—just synchronous, blocking APIs linked into your process.

**[Try the live demo →](https://ferritedb.harlanljones.com/)**

---

## Why Ferrite DB

**Low latency matters.** Networked databases add serialization, kernel switching, and round-trip time. Ferrite DB eliminates all three by living in your process.

**Simplicity compounds.** No separate deployment, no connection pooling, no service restarts. One library, one process, one source of truth.

**Predictability wins.** Synchronous APIs mean no surprise async stack traces. Library-owned threading (via Rayon) means no executor wars between your code and the search engine.

### Key Features

- **Sub-millisecond latency** — search without the overhead of serialization or IPC
- **Approximate nearest-neighbor (ANN)** — trade perfect recall for speed; control the tradeoff
- **Metadata filtering** — predicate-based result refinement evaluated during index traversal
- **Zero operational complexity** — no separate database to deploy or manage
- **Production-ready** — designed for predictable, low-latency workloads

---

## Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
ferrite-db = "0.1"
```

### Basic Usage

Create a table, insert vectors, and search:

```rust
use ferrite_db::{Table, Metric, SearchOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a table for 768-dimensional cosine-similarity search
    let mut table = Table::new("embeddings", 768, Metric::Cosine)?;

    // Insert vectors
    table.insert(vec![0.1, 0.2, 0.3, /* ... 765 more dimensions ... */])?;

    // Search
    let query = vec![0.15, 0.25, 0.35, /* ... */];
    let results = table.search(&query, SearchOptions::default())?;
    
    for (distance, metadata) in results {
        println!("Nearest neighbor: distance={}", distance);
    }
    
    Ok(())
}
```

---

## Use Cases

- **Real-time semantic search** — RAG systems, recommendation engines, instant similarity lookup
- **Embedded analytics** — Vector analytics baked into an application server
- **Local-first AI** — on-device neural search without cloud calls
- **Low-latency filtering** — combine ANN with predicate pushdown for subset search
- **In-process ML** — Feature stores, embedding caches, similarity matching

---

## Architecture & Design

Ferrite DB is built on clear, maintainable architectural boundaries:

| Component | Responsibility |
|-----------|-----------------|
| **Table management** | Table lifecycle, metadata schema, dimensionality and metric validation |
| **Storage** | Immutable segment files, atomic commits via renaming, tombstone bitmaps |
| **Write path** | Append-only inserts, delta buffering, auto-chunking, delete-as-tombstone updates |
| **Concurrency** | Per-table single-writer/many-readers coordination with atomic publication snapshots |
| **Search** | Exhaustive scanning, predicate evaluation, probe budgeting, result assembly |
| **Index substrate** | LanceDB/Arrow integration layer (absorbs upstream API churn) |
| **Compaction** | Background merging of deltas and removal of tombstoned vectors |

### Core Concepts

| Term | Meaning |
|------|---------|
| **Table** | Named collection of vectors sharing one dimensionality and one metric (Cosine, L2, or Dot) |
| **Segment** | Immutable unit of durable storage holding committed vectors |
| **Delta** | Recent inserts buffered until compaction merges them into stable segments |
| **Tombstone** | Marks a deleted vector until background compaction removes it physically |
| **Predicate Tree** | Typed metadata filters evaluated during search traversal |
| **Probe** | One partition of the inverted index examined during search; higher probes mean higher recall |
| **Metric** | Distance function: Cosine, L2, or Dot similarity |

See [`CONTEXT.md`](CONTEXT.md) for the complete canonical vocabulary.

---

## Performance

### Baseline Results (100k × 512-d)

| Metric | Value |
|--------|-------|
| **p50 search latency** | 141.5 ms |
| **p99 search latency** | 279.7 ms |
| **Recall@10** | 1.0000 (exact match) |
| **Ingest throughput** | ≈1.15 M vectors/s |
| **Peak memory** | ≈761 MB |

See [`docs/baselines/`](docs/baselines/) for detailed benchmarks, methodology, and reproducibility.

---

## Development

### Documentation

- **[`AGENTS.md`](AGENTS.md)** — Standing development instructions and architectural ownership
- **[`ROADMAP.md`](ROADMAP.md)** — Current priorities, waves, and milestones
- **[`docs/adr/`](docs/adr/)** — Architecture Decision Records (ratified decisions)
- **[`CONTEXT.md`](CONTEXT.md)** — Canonical vocabulary and domain terminology

### Contributing

Contributions welcome. Before starting, please review:

1. [`AGENTS.md`](AGENTS.md) for standing development instructions
2. [`docs/adr/`](docs/adr/) for architectural decisions (binding)
3. The relevant section in [`ROADMAP.md`](ROADMAP.md)

---

## Status

Ferrite DB is under active development. See [`ROADMAP.md`](ROADMAP.md) for current priorities and milestones.

---

## License

Licensed under the MIT OR Apache 2.0 license.

---

**Try it now:** [Live demo →](https://ferritedb.harlanljones.com/)