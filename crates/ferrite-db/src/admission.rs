//! Admission control — search-admission semaphore (~2× cores in flight);
//! sheds with `Busy` rather than queueing (AGENTS.md §4, ADR 0007). The ONLY
//! component allowed to return `Busy` for capacity reasons. Owned by ROADMAP
//! FDB-050; intentionally empty until then.
