//! Write path — append-only inserts, Delta buffering, auto-chunking,
//! delete-as-Tombstone, update-as-delete-plus-insert (AGENTS.md §4). Sole
//! producer of new Segments. Owned by ROADMAP FDB-013/FDB-016; intentionally
//! empty until then.
