//! Ferrite DB — embedded low-latency approximate nearest-neighbor search.
//!
//! An embedded Rust library linked directly into the host process; never a
//! service (ADR 0001). The public core API is synchronous and blocking;
//! compute runs on a library-owned Rayon pool (ADR 0004). Segments are
//! immutable and published by atomic rename; there is no WAL (ADR 0003).
//!
//! Module layout mirrors the fixed concern boundaries of `AGENTS.md` §4.
//! Each module is a stub pending its owning ROADMAP work item; no logic,
//! no dependencies (FDB-002).

#![forbid(unsafe_code)]

/// U1/G4 (ROADMAP FDB-070): allocator selection as compile-time features.
/// With neither set, the system allocator is inherited. If BOTH are set
/// (e.g. under `--all-features`), mimalloc takes precedence by the guards
/// below — deterministic, but treat it as a misconfiguration.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "jemalloc", not(feature = "mimalloc")))]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

pub mod admission;
pub mod compaction;
pub mod concurrency;
pub mod errors;
pub mod index_substrate;
pub mod observability;
pub mod search;
pub mod storage;
pub mod table;
pub mod write_path;
