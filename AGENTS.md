# AGENTS.md — Ferrite DB development instructions

Standing instructions for any development agent working in this repository. They are durable:
they do not contain milestones, schedules, or status — those live in `ROADMAP.md`.

## 1. What this project is

Ferrite DB is an **embedded Rust library** for low-latency approximate nearest-neighbor search
over dense vector embeddings, linked directly into the host process. It is **never a service**:
no socket, no serialization boundary, no networked wrapper inside this repository (ADR 0001).
The public core API is synchronous and blocking; compute runs on a library-owned Rayon pool
(ADR 0004). An async facade and any networked wrapper arrive later, as separate composition
layers — do not introduce them here ahead of an ADR amendment.

## 2. Instruction precedence

When sources of truth conflict, resolve in this order:

1. `docs/adr/*.md` — ratified decisions. Binding. Violating one requires **amending the ADR
   first** (new numbered record superseding the old); never deviate silently.
2. `AGENTS.md` (this file).
3. `ROADMAP.md` — current plan, work items, waves, gates. Volatile by design.
4. Individual judgment — only where the above three are silent, and flagged in your report.

## 3. Canonical vocabulary (mandatory)

The glossary in `CONTEXT.md` is canonical. Code identifiers, type names, module names, test
names, commit messages, and comments MUST use these terms with these exact meanings:

| Term             | Meaning (abbreviated)                                                    |
|------------------|--------------------------------------------------------------------------|
| **Table**        | Named collection sharing one dimensionality and one Metric               |
| **Metadata Schema** | Typed scalar columns declared with a Table                            |
| **Segment**      | Immutable unit of storage holding committed vectors                      |
| **Delta**        | Recent-insert Segment searched exhaustively until Compaction merges it   |
| **Tombstone**    | Marker hiding a deleted vector until Compaction removes it physically    |
| **Commit**       | Atomic act of publishing a Segment (atomic rename, per ADR 0003)         |
| **Compaction**   | Background merge absorbing Deltas and discarding Tombstoned vectors      |
| **Metric**       | The single distance function of a Table: Cosine, L2, or Dot              |
| **Predicate Tree**| Structured filter composed in code, evaluated during traversal          |
| **Probe**        | One visited partition of a Table's inverted index                        |

Banned synonyms (from CONTEXT.md) must not appear in code or docs: *collection, namespace,
file/shard/partition (for Segment), memtable, staging buffer, flush, WAL write, soft delete,
delete flag, engine, server, service, filter string, WHERE clause, nprobe, similarity,
score type*. Note: "index ladder", "inverted index", and ANN index family names (HNSW,
IVF-PQ) are legitimate — "index" is banned only as a synonym for Table. Third-party API
identifiers (e.g. LanceDB's own filter pushdown or compaction) are exempt when quoted as
upstream names. "Snapshot" is disambiguated: *publication snapshot* is the Concurrency
concern swapped atomically at Commit; *snapshot reads* (time-travel) are a deferred
non-goal.

## 4. Architectural ownership boundaries

Crate layout was ratified at G1: a root virtual workspace with the single member crate
`crates/ferrite-db`, one module per concern below. The **concern boundaries are fixed**. Each
concern has exactly one owning component;
if you touch a concern outside its owner's scope, stop and renegotiate.

- **Table management** — Table lifecycle (create/open/drop), Metadata Schema declaration,
  dimension (`u32`) and Metric validation at creation. Sole validator of schema and
  dimensionality; source of `TableNotFound` and `SchemaViolation`.
- **Storage** — Segment persistence: immutable Segment files, atomic-rename Commit, Segment
  footer, Tombstone bitmaps. Sole owner of on-disk layout. No other component reads or writes
  Segment files directly.
- **Write path** — append-only inserts, Delta buffering, auto-chunking (~64k–128k vectors per
  Segment), delete-as-Tombstone, update-as-delete-plus-insert. Sole producer of new Segments.
- **Concurrency** — per-Table single-writer/many-readers coordination, publication snapshot
  swap on Commit, library-owned Rayon pool (ADRs 0004, 0005).
- **Search** — exhaustive scanning, Predicate Tree evaluation, Probe budgeting, SearchOptions
  plumbing, result assembly. Queries are always scoped to exactly one Table; cross-Table
  queries are permanently out of scope (ADR 0005) and must be unrepresentable in the API.
- **Index substrate** — the ONLY module permitted to depend on LanceDB/Arrow types (ADR 0002).
  All other modules go through this seam. This containment is what absorbs upstream API churn;
  breaching it multiplies that churn across the codebase.
- **Lifecycle/Compaction** — background Compaction job, trigger evaluation, manual
  `compact()` escape hatch, physical Tombstone removal.
- **Admission control** — search-admission semaphore (~2× cores in flight). The ONLY component
  allowed to return `Busy` for capacity reasons, and it must shed rather than queue (ADR 0007).
- **Errors** — the error taxonomy and its types. Every public fallible API returns these types.
- **Observability** — optional `tracing` feature: spans and histograms. Other components emit
  only through its gated macros; the default build carries zero observability cost.

## 5. Toolchain and verification commands

- `rustc`/`cargo` **1.97.1** are installed.
- The workspace scaffold landed via ROADMAP FDB-002 and all four commands below were
  demonstrated passing on it. Keep them green; adjust flags only through a recorded ROADMAP
  change if the workspace layout demands it.
- Git is **initialized, but commits belong to humans**: this workspace's policy denies
  `git commit` to agents. Stage your changes (`git add`) and report; never claim a commit
  exists unless you observed it being made.
- **MSRV is 1.97** (stable at project start). Until the first 1.0, minor releases may contain
  breaking changes; patch releases stay compatible (U6, decided at G1).

These four commands are the standing quality gate, first verified on the FDB-002 scaffold:

```
cargo build --all-targets
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

CI (ROADMAP FDB-003) enforces the same set. Warnings are errors; formatting drift fails.

## 6. Quality gates (apply to every change)

1. **No panics across the public API boundary.** Public fallible operations return
   `Result<_, Error>`. Caller-fixable conditions map to `{Busy, DimensionMismatch,
   SchemaViolation, TableNotFound}`; operational failures map to `{Io, CorruptSegment}`.
2. **No `unwrap()`/`expect()` outside `#[cfg(test)]` code.** Tests may use them freely.
3. **ADR supremacy.** Any change that contradicts an ADR requires the ADR amendment to land
   first (or in the same change, amendment included). Cite the ADR number in the explanation.
4. **`unsafe` discipline.** Every `unsafe` block carries a `// SAFETY:` comment giving the
   concrete justification: validity of pointers/references, aliasing, and lifetime arguments.
   Minimize unsafe surface; prefer confining it to audited boundaries.
5. **Performance claims require evidence.** Once the benchmark harness exists (ROADMAP
   FDB-021), any performance-affecting change cites a harness run. Before baselines exist
   (FDB-022), label every performance statement "unmeasured".
6. **FFI-friendly public signatures** (ADR 0001): keep public signatures simple and, in
   principle, C-representable; do not expose abstractions that would foreclose a future FFI
   layer. Rust-only v1 — no bindings, just signature hygiene.
7. **Vocabulary compliance.** Section 3 terms are checked in review like any other gate.
8. **Objective exit criteria.** Work is done when its ROADMAP exit criterion is objectively
   met — not when the code looks finished.

## 7. Coordination protocol (multi-agent work)

- **Decompose by dependency**, using the predecessor lists in `ROADMAP.md`. Do not start a
  work item before its dependencies' exit criteria are met.
- **One writer per file/component.** Claim the exclusive ownership scope of your work item.
  Two tasks must never hold write ownership of the same file concurrently; serialize instead.
- **Run independent tasks concurrently** only when their ownership scopes are disjoint. The
  wave structure in ROADMAP.md encodes safe parallelism; when in doubt, serialize.
- **Integrate in dependency order.** After every merge, re-run the full gate suite (Section 5)
  and re-validate the exit criteria of downstream, already-completed items that the merge
  could have affected.
- **Ownership changes are negotiated**, never taken: widening a scope means updating the
  owning work item in ROADMAP.md first.
- **Report progress against exit criteria**, citing the work-item ID. Latency/recall claims
  must cite a benchmark run (task ID + configuration) or be labeled "unmeasured". Surface any
  discovered ADR conflict immediately as a proposed amendment; escalate unmeasurable or
  ambiguous exit criteria rather than quietly reinterpreting them.

Temporary milestones, baselines, and schedules are out of scope for this file — see ROADMAP.md.

## 8. Agent skills

### Issue tracker

Issues for this repo live in **Linear** (workspace `harlanljones`, team `HJ`, project `Ferrite DB`), driven by the `linear` CLI. ROADMAP work-item IDs (FDB-nnn) title their Linear issues one-to-one. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage roles map identity-for-identity onto Linear labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout: root `CONTEXT.md` glossary plus `docs/adr/`. Read them before exploring code; use glossary vocabulary in all outputs. See `docs/agents/domain.md`.
