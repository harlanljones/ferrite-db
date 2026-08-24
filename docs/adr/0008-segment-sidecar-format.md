# Ferrite-owned `.fseg` Segment sidecar format, version-gated

Segment payloads live in Ferrite-controlled `.fseg` files — magic `FRSG`, a format-version field, reserved bytes for evolution, CRC-32 over header and payload — kept strictly outside Lance-owned storage (risk R2). Readers validate structure and both checksums eagerly at open and refuse any version other than the one they were built against; corruption is rejected before any payload access.

## Considered Options

- Embedding Tombstone bitmaps and footers inside `.lance` payloads: rejected — Lance owns that format's evolution (ADR 0002), so our commit-critical metadata would track upstream churn.
- Lazy/streaming validation: deferred — eager validation makes "rejected before use" absolute; can be revisited with the mmap work.

## Consequences

Migration between format versions happens by rewriting Segments during Compaction, never by in-place mutation. Old-format files must pass through Compaction before newer code reads them. Every reader change must keep the version gate honest.
