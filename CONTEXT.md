# Ferrite DB

An embedded Rust library for low-latency approximate nearest-neighbor search over dense vector embeddings, linked directly into the host process.

## Language

**Ferrite DB**:
The embedded search library itself: a Rust crate running inside the host process. _Avoid_: engine, server, service

### Storage

**Table**:
A named collection of vectors sharing one dimensionality and one metric, created explicitly before insert. _Avoid_: collection, namespace, index

**Metadata Schema**:
The typed scalar columns declared with a Table, against which Predicate Trees are evaluated. _Avoid_: metadata blob, payload schema

**Segment**:
An immutable unit of storage holding a batch of committed vectors and their index structures. _Avoid_: file, shard, partition

**Delta**:
A segment of recent inserts searched exhaustively until compaction merges it away. _Avoid_: memtable, staging buffer

**Tombstone**:
A marker that hides a deleted vector from results until compaction removes it physically. _Avoid_: soft delete, delete flag

**Commit**:
The act of atomically publishing a segment, making its vectors durable and searchable. _Avoid_: flush, WAL write

**Compaction**:
The background merge that absorbs deltas and physically discards tombstoned vectors.

### Querying

**Metric**:
The single distance function of a table: cosine, L2, or dot product. _Avoid_: similarity, score type

**Predicate Tree**:
A structured filter evaluated during index traversal, composed in code rather than parsed from text. _Avoid_: filter string, WHERE clause

**Probe**:
One visited partition of a table's inverted index during a search; probe count trades latency against recall. _Avoid_: nprobe setting
