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
// The native substrate (LanceDB/tokio/Rayon), on-disk Segment storage, and the
// background-thread Lifecycle/Concurrency concerns are unavailable on WASM:
// there is no filesystem, no threads, and no LanceDB C++ dependency. They are
// gated out on the `wasm32` target so the crate builds there as a threadless,
// in-memory core — FDB-EXP-01. Gating by target (not by feature) keeps native
// consumers (`harness`, `explorer`) intact under `--all-features` unification,
// where a `wasm` feature flag would otherwise strip modules they depend on.
// The `wasm` Cargo feature is the documented opt-in marker for reduced-core
// builds; the `wasm32` target is what actually engages it.
#[cfg(all(feature = "substrate", not(target_arch = "wasm32")))]
pub mod compaction;
#[cfg(all(feature = "substrate", not(target_arch = "wasm32")))]
pub mod concurrency;
pub mod errors;
#[cfg(all(feature = "substrate", not(target_arch = "wasm32")))]
pub mod index_substrate;
pub mod observability;
pub mod search;
#[cfg(not(target_arch = "wasm32"))]
pub mod storage;
pub mod table;
pub mod write_path;
