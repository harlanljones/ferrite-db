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
