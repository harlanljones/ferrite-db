# ROADMAP.md — Ferrite DB execution plan

Executable plan, not a wish list: every work item has an objectively checkable exit criterion,
every milestone has a gate, and every gate is re-validated after merges (AGENTS.md §7).
Precedence: ADRs > AGENTS.md > this file > individual judgment.

## 1. Current state (verified at planning time)

- `CONTEXT.md` — canonical domain glossary (binding vocabulary).
- `docs/adr/0001..0007` — ratified decisions.
- `AGENTS.md`, `ROADMAP.md` — created by this planning pass.
- **No** `Cargo.toml`, **no** source files, **no** git repository. Toolchain present:
  `cargo`/`rustc` 1.97.1.

## 2. Objective and scope

Ship v1 of Ferrite DB: an embedded Rust library for low-latency ANN search over dense vectors
(ADR 0001), meeting the SLOs below on the synthetic benchmark contract (ADR 0006), with the
write path, scan-only search, index ladder, Compaction, admission control, error hardening,
and observability all in scope.

### SLOs (targets; no baselines or instruments exist yet)

| Kind | Value | Role |
|------|-------|------|
| p50 search latency (unfiltered) | ≤ 1.2 ms | target |
| p99 search latency (unfiltered) | ≤ 4.5 ms | target |
| Recall@10 unfiltered | ≥ 96.5% | target |
| p50 hard ceiling | 2 ms | CI fail-gate |
| p99 hard ceiling | 8 ms | CI fail-gate |
| Recall@10 floor | 94% | CI fail-gate |
| Filtered recall@10 at selectivity 1.0 / 0.1 / 0.01 / 0.001 | TBD until first baseline | report-only until gate G2 |

Benchmark contract (ADR 0006): synthetic 10M × 512-d f32 corpus with exact-search ground
truth is the gate; SIFT1M is a comparability sanity run only, never the gate.

Provenance: these values adopt the original TDD's §5 table and deliberately supersede its
header (p50 ≤ 1.5 ms, p99 ≤ 5 ms @ 10,000 QPS). The QPS figure was discarded because Ferrite
DB exposes no service boundary (ADR 0001): latency contracts hold per call, at the
caller-concurrency level pinned by the benchmark environment spec (see §4, G2-entry).

### Non-goals (explicit)

Networked service/API server; distributed anything; multi-tenancy guarantees; MVCC; WAL;
custom SIMD index implementations beyond targeted kernels proven necessary by profiling;
Python/C bindings in v1; snapshot reads in v1; string-parsed filters; cross-Table queries
(permanently out of scope, ADR 0005).

## 3. Assumptions

1. Target workload stores derived embeddings, so crash recovery by upstream re-ingest is
   acceptable and no WAL is needed (ADR 0003).
2. Callers doing fire-and-forget inserts handle `Busy`/rejection themselves (accepted design
   assumption behind ADRs 0003 and 0007).
3. One host process hosts many independent Tables; concurrent writers per Table shard at the
   caller (ADR 0005).
4. The public API stays synchronous/blocking on a library-owned Rayon pool (ADR 0004); hosts
   needing async wrap it later.
5. The 10M × 512-d corpus fits on local disk where benchmarks run. Arithmetic to verify at
   FDB-020 start: vectors alone are ~20.5 GiB (10M × 512 × 4 B), multiplied by ground truth,
   index artifacts, allocator-comparison reruns (U1), and any CI copies.
6. LanceDB + Arrow provide IVF-PQ/HNSW of sufficient quality that custom ANN structures are
   unnecessary (ADR 0002).

## 4. Unresolved decisions

| # | Decision | Why it matters | Resolved at |
|---|----------|----------------|-------------|
| U1 | Allocator default (mimalloc vs jemalloc), shipped as compile-time feature flag chosen by benchmark — never API surface | allocation strategy moves tail latency; must be evidence-chosen | G4 / FDB-070 |
| U2 | Filtered-recall gate values for selectivity tiers | needed to convert report-only tiers into enforceable gates | G2 / FDB-022 |
| U3 | LanceDB (+ Arrow) version pinning strategy | upstream churn is risk R1; pinning policy bounds it | G-Lance / FDB-030 |

**Resolved at G1 (held 2026-08-23, with FDB-002):**

- **U4 crate layout**: root virtual workspace, single member `crates/ferrite-db`, ten concern
  modules (`errors`, `table`, `storage`, `write_path`, `concurrency`, `search`,
  `index_substrate`, `compaction`, `admission`, `observability`).
- **U5 `Busy` scope**: search admission only; writers block on the writer lock, never return
  `Busy` (clarified in ADR 0007).
- **U6 MSRV/semver**: MSRV 1.97; until 1.0, minors may break, patches compatible (recorded in
  AGENTS.md §5).
- **U7 sidecar migration**: `.fseg` version field + reserved bytes; readers reject any other
  version eagerly; migration via Compaction rewrite (ADR 0008).
- **§13-4 string columns**: declared, stored, retrievable — not filterable in v1.
- **§13-1 delete-of-unknown-id**: succeed-and-ignore.
- **§13-3 SearchOptions shape**: `{ top_k: u32 (default 10, max 1_000), probes: Option<u32>,
  ef_search: Option<u32> }`.
- **§13-6 publication channel**: private registry for now; revisit crates.io at 1.0.

Benchmark environment spec — hardware/OS class, caller-concurrency level, top-k, warmup and
query-count methodology, cache-state control, reproducibility variance bound: resolved at a
**G2-entry session held before FDB-020 starts**. Measuring first and pinning the environment
afterwards would make the baselines meaningless and unfalsifiable. §13 items 1–4 are decided
at G1 because they freeze Wave 1/3 signatures; item 5 by FDB-021; item 6 at G1.
*Amendment note (2026-08-24): the session was in fact held late — with FDB-022, immediately
before capture, and pinned the dev machine at declared reduced scale
(`docs/baselines/ENVIRONMENT.md`). The pin-before-measure principle was preserved; only its
timing slipped, with Harlan's approval and contract-scale re-baseline tracked as FDB-023.*

## 5. Metrics

Baselines were established by FDB-022 (2026-08-24) at the **declared reduced scale
100 000 × 512-d** on the pinned dev-baseline environment, because the contract corpus cannot be
captured on available hardware (see `docs/baselines/ENVIRONMENT.md`); re-baselining at the 10M × 512
contract scale is tracked as FDB-023. Values below are medians of 5 reruns with run artifacts and the
recorded reproducibility-variance bound in `docs/baselines/README.md`; owners assigned at FDB-022.

| metric | baseline | target/threshold | measurement method | owner | review cadence |
|---|---|---|---|---|---|
| p50 search latency, unfiltered top-k | **141.5 ms** @ declared scale (variance bound ±15%) [docs/baselines/artifacts/tier-1.0-run-*.json] | target ≤ 1.2 ms; hard ceiling 2 ms (CI fail-gate) — enforceable only against contract-scale re-baseline | FDB-021 harness percentile over its query set against the FDB-020 corpus (declared-scale capture) | performance (FDB-022); matters: primary latency SLO | every merged harness run; re-baselined at each milestone gate |
| p99 search latency, unfiltered top-k | **279.7 ms** @ declared scale (variance bound ±35%) [same artifacts] | target ≤ 4.5 ms; hard ceiling 8 ms (CI fail-gate) — same caveat | same harness as above | performance (FDB-022); matters: tail-latency SLO | same as above |
| Recall@10, unfiltered | **1.0000** (exact scan reproduces ground truth; variance bound: exact equality) [same artifacts] | target ≥ 96.5%; fail-floor 94% (CI fail-gate) | harness vs FDB-020 exact-search ground truth | performance (FDB-022); matters: accuracy SLO | same as above |
| Filtered recall@10 @ selectivity 1.0 | **1.0000** (exact; report-only until G2 sets gates) | TBD pending G2 decision | harness predicate-filtered runs via FDB-021 | performance (FDB-022) | report-only cadence until G2 converts to gates |
| Filtered recall@10 @ selectivity 0.1 | **1.0000** vs filtered oracle (exact; report-only until G2) | TBD pending first baseline (set at G2) | as above | performance (FDB-022) | as above |
| Filtered recall@10 @ selectivity 0.01 | **1.0000** vs filtered oracle (exact; report-only until G2) | TBD pending first baseline (set at G2) | as above | performance (FDB-022) | as above |
| Filtered recall@10 @ selectivity 0.001 | **1.0000** vs filtered oracle (exact; report-only until G2) | TBD pending first baseline (set at G2) | as above | performance (FDB-022) | as above |
| Peak RSS under the benchmark contract | **≈ 761 MB** unfiltered pass / ≈ 481–508 MB filtered @ declared scale (variance bound ±1%) [docs/baselines/artifacts/] | target set at G2-entry with the environment spec — no basis to invent one today | harness captures process RSS over the standard run | performance (FDB-022); matters: first-order purchase criterion for an embedded library | every merged harness run |

Ingest throughput (R4 monitor): median ≈ 1.15 M vectors/s at declared scale (per-tier medians
0.97–1.17 M/s; variance bound ±50%), reported in every artifact under `ingest`.

SIFT1M comparability sanity runs (secondary, never gating) are reported alongside but get no
table row until G2 decides whether one is warranted.

## 6. Phasing rationale

Order matches the ratified architecture and two discipline rules:

1. **Correctness before indices** — write path plus exhaustive-scan search first, so there is
   always an oracle-correct implementation to diff indexed results against.
2. **Evidence before tuning claims** — the benchmark harness and first baseline land before any
   index-ladder work can make performance statements (AGENTS.md quality gate 5). This forces
   Wave 4 ahead of Wave 5 even though LanceDB integration could technically start earlier.
   The FDB-004 spike is exempt by construction: it yields capability facts only, never
   performance claims, so it may run in Wave 1.
3. **Compaction after the ladder** because merged Segments must be re-indexed under the ladder's
   selection rules.
4. **Hardening before instrumentation before tuning**: admission control and panic/error audits
   freeze behavior; observability then instruments stable surfaces; tuning consumes both.

## 7. Milestones and gates

| ID | Name | Work items | Exit gate (objectively checkable) |
|----|------|-----------|------------------------------------|
| M0 | Repository bootstrap | FDB-001 → FDB-002 | git repo tracks exactly the four root docs; empty scaffold passes all four gate commands (build/test/clippy `-D warnings`/fmt); naming proposal tabled for G1 |
| M1 | Correctness core | FDB-003 ∥ FDB-010 ∥ FDB-011 ∥ FDB-012, then FDB-013/FDB-015, then FDB-014 → FDB-016 | end-to-end suite green: create Table → insert → immediate visibility via Delta → filtered/unfiltered scan search correct vs oracle across all three Metrics → Tombstone/update visibility; crash-at-rename test passes; full gate suite green |
| M2 | Evidence baseline | FDB-020 → FDB-021 → FDB-022 | corpus regenerates byte-identical; harness emits machine-readable p50/p99/recall incl. selectivity tiers; baselines recorded in §5 replacing TBDs; hard-ceiling checks wired to non-zero exit; ingest throughput reported (risk R4 monitor) |
| M3 | Index ladder | FDB-030 → FDB-031 → FDB-032 | selector function proven correct at 10k/1M boundaries; override-at-creation honored; background build past ~50k rows observed; calibrated defaults Pareto-dominate naive fixed defaults (recall@10 ≥ AND p50 ≤ AND p99 ≤) in a recorded harness run |
| M4 | Lifecycle | FDB-040 | triggers fire at max(1% rows, 100k) or ≥ 4 Deltas; > 20% Tombstone ratio physically removes vectors; searches stay oracle-consistent during Compaction; `compact()` idempotent |
| M5 | Hardening | FDB-050 then FDB-051 | overload sheds with `Busy`, zero queue structures exist; automated audit shows no unwrap/expect/panic outside tests; simulated crash leaves each Segment committed-or-absent per ADR 0003 |
| M6 | Observability | FDB-060 | all instrumentation behind feature-gated macros (grep-audit proves default build carries none); enabled build emits spans + histograms for one golden workload |
| M7 | Tuning campaign | FDB-070 | campaign report states achieved p50/p99/recall vs targets with run citations; allocator choice ratified at G4; any miss documented with evidence and escalated to G3 — targets never silently relaxed |

## 8. Traceability (requirement → work items)

| Source | Requirement | Work items |
|--------|-------------|------------|
| ADR 0001 | embedded crate; never a service; FFI-friendly signatures; Rust-only v1 | FDB-002 (lib scaffold), FDB-051 (boundary audit) |
| ADR 0002 | consume LanceDB IVF-PQ/HNSW over Arrow; no custom ANN; SIMD only if profile-proven | FDB-004 (feasibility spike), FDB-030, FDB-031, FDB-032; deferred backlog (kernels) |
| ADR 0003 | immutable Segments; atomic-rename Commit; no WAL; re-ingest recovery | FDB-012, FDB-013, FDB-051 |
| ADR 0004 | sync blocking core; library-owned Rayon pool; async facade later | FDB-015, FDB-050; deferred backlog (facade) |
| ADR 0005 | many Tables; single writer/many readers; cross-Table queries unrepresentable | FDB-011, FDB-014, FDB-015 |
| ADR 0006 | synthetic 10M × 512-d gate; SIFT1M sanity only | FDB-020, FDB-021, FDB-022, FDB-070 |
| ADR 0007 | semaphore ~2× cores; shed with `Busy`; never queue | FDB-050 |
| Design decisions | dimension u32 fixed per Table; Metrics {Cosine (normalized), L2, Dot} | FDB-011, FDB-014 |
| Design decisions | Metadata Schema {bool/i64/f64/string}; Predicate Tree {=,!=,<,<=,>,>=,IN + AND/OR/NOT}; pre-filter pushdown; no string filters v1 | FDB-011, FDB-014 |
| Design decisions | append-only inserts; Delta searched exhaustively; auto-chunk 64k–128k; no batch upper bound | FDB-013 |
| Design decisions | Tombstone deletes; > 20% ratio physical rewrite; update = delete+insert | FDB-016, FDB-040 |
| Design decisions | Compaction trigger max(1% rows, 100k) or ≥ 4 Deltas; manual `compact()` | FDB-040 |
| Design decisions | index ladder < 10k exhaustive / < 1M HNSW / above IVF-PQ; override at creation; background build past ~50k | FDB-031 |
| Design decisions | Probes/ef_search auto-calibrated; SearchOptions escape hatch | FDB-032 |
| Design decisions | error taxonomy caller-fixable vs operational; no panics across boundary | FDB-010, FDB-051 |
| Design decisions | optional tracing spans/histograms, zero cost disabled | FDB-060 |
| Design decisions | allocator as compile-time flag, bench-chosen | FDB-070 (G4) |
| SLOs (§2) | latency + recall targets and hard-ceiling fail-gates | FDB-021, FDB-022, FDB-070 |

## 9. Work items

Format: dependencies · suggested role · exclusive ownership scope · deliverable · validation ·
exit criterion. Layout ratified at G1 (2026-08-23): ownership scopes are concrete paths under
`crates/ferrite-db/src/` — errors→`errors.rs`, Table management→`table.rs`, storage→
`storage.rs`, write path→`write_path.rs`, concurrency→`concurrency.rs`, search+predicates→
`search.rs`, index substrate→`index_substrate.rs`, lifecycle/Compaction→`compaction.rs`,
admission→`admission.rs`, observability→`observability.rs`. CI owns pipeline files; corpus
tools, harness, and the audit/test tree own their future directories.

### Wave 0 — bootstrap (strictly serial)

**FDB-001 — Initialize VCS**
- Depends: none (serial prerequisite for everything)
- Suggested role: release/tooling engineer
- Ownership: repository-root VCS metadata only (`.git/`, `.gitignore`)
- Deliverable: initialized git repo; `.gitignore` excluding `/target`
- Validation: `git status` clean after initial commit
- Exit criterion: initial commit contains exactly `CONTEXT.md`, `docs/`, `AGENTS.md`,
  `ROADMAP.md`; nothing else tracked (`git ls-files` proves it)

**FDB-002 — Cargo workspace scaffold**
- Depends: FDB-001
- Suggested role: lead Rust engineer
- Ownership: `Cargo.toml`(s), crate directory skeleton, rustfmt/clippy config files
- Deliverable: lib-type crate skeleton (FFI-friendly posture, ADR 0001) with zero-dependency
  stub modules named after the §4 concerns; proposed final naming/layout written up for G1;
  demonstrates all four gate commands passing
- Validation: run the four PENDING-FIRST-VERIFICATION commands from AGENTS.md §5
- Exit criterion: all four commands succeed on the empty scaffold; layout proposal recorded;
  AGENTS.md §5 labels flipped from pending to verified

### Wave 1 — foundations (parallel, disjoint scopes)

**FDB-003 — CI skeleton**
- Depends: FDB-002
- Suggested role: DevOps engineer
- Ownership: CI pipeline definition files only
- Deliverable: pipeline running the four gate commands plus a dependency/supply-chain audit
  (cargo-deny or equivalent advisory check) on every push/PR
- Validation: observe a red run on an intentionally broken commit, then green on revert
- Exit criterion: CI executes build/test/clippy(`-D warnings`)/fmt and fails on violation

**FDB-004 — Substrate feasibility spike (strictly time-boxed)**
- Depends: FDB-002
- Suggested role: integration engineer
- Ownership: throwaway spike directory outside the shipped workspace (`spike/lancedb/`),
  quarantined or deleted at exit; touches no production modules
- Deliverable: consume-only driver proving on 512-d data that (a) HNSW and IVF-PQ both build
  and answer queries through LanceDB with per-query knob control, (b) an externally managed
  immutable Segment file coexists with Lance-owned storage; friction-log memo for G-Lance
- Validation: the spike demo compiles and runs; memo reviewed at G-Lance
- Exit criterion: memo records demonstrable yes/no per capability above; NO performance
  numbers produced or cited (evidence still routes exclusively through FDB-021/FDB-022);
  any absent capability stops work and escalates to ADR-amendment review rather than being
  engineered around

**FDB-010 — Error taxonomy**
- Depends: FDB-002
- Suggested role: systems engineer
- Ownership: errors module exclusively
- Deliverable: `Error` type with variants `{Busy, DimensionMismatch, SchemaViolation,
  TableNotFound, Io, CorruptSegment}` split caller-fixable vs operational; Display/Error impls
- Validation: exhaustive variant tests; doc examples compile
- Exit criterion: every variant constructed and classified in tests; zero panics in module

**FDB-011 — Table management + Metadata Schema**
- Depends: FDB-002, FDB-010
- Suggested role: systems engineer
- Ownership: table-management module exclusively
- Deliverable: create/open/drop Table; dimension (`u32`) and Metric fixed at creation;
  Metadata Schema declaration (bool/i64/f64/string columns); query API takes a Table handle
  so cross-Table queries are unrepresentable (ADR 0005); Predicate Tree represented as a
  public enum tree — no closures or trait objects in public signatures (AGENTS.md gate 6)
- Validation: schema/dimension/Metric validation matrix tests
- Exit criterion: duplicate or malformed declarations return `SchemaViolation`; unknown names
  return `TableNotFound`; no code path accepts a second Table in a query

**FDB-012 — Segment storage + Commit + Tombstone bitmaps**
- Depends: FDB-002, FDB-010
- Suggested role: storage engineer
- Ownership: storage module exclusively (sole reader/writer of Segment files)
- Deliverable: immutable Segment writer/reader; atomic-rename publish (ADR 0003, no WAL);
  Segment footer with row counts; Tombstone bitmap block
- Validation: footer round-trip tests; bitmap set/clear/iterate tests; rename-boundary fault
  injection; reader-validation tests over truncated, overflowed, and bit-flipped Segment files
- Exit criterion: injected crash around rename yields Segment fully visible or fully absent,
  never partial; round-trips lossless; corrupt/truncated input is rejected with
  `CorruptSegment` before any mapped dereference, proven by test

### Wave 2 — write path + concurrency (parallel)

**FDB-013 — Insert path, Delta buffering, auto-chunking**
- Depends: FDB-011, FDB-012
- Suggested role: systems engineer
- Ownership: write-path module exclusively
- Deliverable: append-only inserts validated against dimension (`DimensionMismatch`) and
  schema (`SchemaViolation`); Delta buffering; auto-chunk into new Segments within
  ~64k–128k vectors; no batch-size upper bound
- Validation: chunk-boundary tests around 64k/128k; oversized-batch acceptance test;
  insert-then-search-immediately contract test
- Exit criterion: freshly inserted vectors are searchable immediately via exhaustive Delta
  scan; chunk sizes stay inside the ratified range under randomized batch sizes

**FDB-015 — Concurrency: single writer / many readers**
- Depends: FDB-011, FDB-012
- Suggested role: concurrency specialist
- Ownership: concurrency module exclusively
- Deliverable: per-Table writer lock; readers traverse an immutable published snapshot swapped
  atomically at Commit; library-owned Rayon pool (ADR 0004)
- Validation: stress test — ongoing Commits while parallel searches run, checked against an
  oracle snapshot
- Exit criterion: stress suite passes with oracle-equality asserted on every concurrent search
  (zero-mismatch tolerance); writers serialize; searches acquire only the immutable published
  snapshot

### Wave 3 — search correctness (ordered within wave)

**FDB-014 — Scan-only search + Predicate Tree**
- Depends: FDB-013, FDB-015
- Suggested role: search engineer
- Ownership: search + predicate modules exclusively
- Deliverable: exhaustive Segment/Delta scan; typed predicates {=,!=,<,<=,>,>=,IN} composed
  via AND/OR/NOT; Metric semantics (Cosine normalized, L2, Dot); top-k assembly
- Validation: property tests against a naive oracle across predicate combinations and all
  three Metrics, including selectivity extremes
- Exit criterion: zero mismatches vs oracle on generated suites; ordering semantics provably
  correct per Metric; string columns retrievable but not filterable (v1 rule)

**FDB-016 — Delete/update semantics**
- Depends: FDB-014
- Suggested role: write-path engineer
- Ownership: tombstone/visibility logic in the write path (negotiated with FDB-013 owner)
- Deliverable: delete records a Tombstone honored by scans; update = delete + insert
- Validation: oracle tests for post-delete/post-update visibility across old and new Segments
- Exit criterion: deleted vectors invisible everywhere; updates never surface stale copies;
  unknown-id delete semantics implemented per §13 resolution

### Wave 4 — evidence baseline (serial chain)

**FDB-020 — Synthetic corpus generator + query-set specification**
- Depends: FDB-016 (needs the public write path to load data); G2-entry environment spec
  (§4) must be pinned first
- Suggested role: benchmarks engineer
- Ownership: corpus tooling directory exclusively
- Deliverable: reproducible generator producing the 10M × 512-d f32 corpus; exact-search
  ground truth AND the harness query-set specification (query count, distribution,
  selectivity-tier construction, top-k) as versioned fixtures consumed unchanged by FDB-021
  (ADR 0006); disk-capacity check per assumption 5
- Validation: regenerate with identical parameters and compare checksums
- Exit criterion: byte-identical regeneration proven once; format documented

**FDB-021 — Benchmark harness**
- Depends: FDB-020
- Suggested role: performance engineer
- Ownership: harness/bench directory exclusively
- Deliverable: measures p50/p99 latency and recall@10 under the G2-entry environment spec,
  recording hardware, caller-concurrency level, top-k, warmup/query counts, and cache-state
  control (pre-warm methodology so cold-start mmap faults cannot masquerade as steady-state
  p99); supports filtered runs at selectivity 1.0/0.1/0.01/0.001; emits machine-readable
  results including peak RSS and ingest throughput; wires hard ceilings to non-zero exit;
  secondary SIFT1M comparability mode
- Validation: dry-run against the oracle implementation itself
- Exit criterion: one full machine-readable report produced end-to-end; ceiling violations
  demonstrably fail the process

**FDB-022 — First baseline capture**
- Depends: FDB-021
- Suggested role: performance engineer
- Ownership: §5 metric table values + baseline artifacts location
- Deliverable: recorded baselines with environment description matching the G2-entry spec and
  run artifacts; ingest throughput reported; a numeric reproducibility-variance bound recorded
  alongside the baselines it qualifies. Known documented limitation: admission control
  (FDB-050) is not yet in place, so baselines measure unthrottled dispatch. Resolution note
  (2026-08-24): FDB-050 landed before capture; at the pinned caller-concurrency of 1 admission
  never sheds, so the baselines remain effectively unthrottled — limitation now moot.
- Validation: reruns reproduce within the recorded variance bound on the pinned environment
- Exit criterion: §5 TBDs replaced with measured values + artifact pointers + the variance
  bound; G2 close-out convened
- Outcome deviation (approved by Harlan 2026-08-24): captured at declared reduced scale
  100 000 × 512-d on the dev machine (contract scale exceeds available RAM); G2-entry
  environment pinned to that machine in `docs/baselines/ENVIRONMENT.md`; artifacts in
  `docs/baselines/artifacts/`; variance bound per metric in `docs/baselines/README.md`

**FDB-023 — Contract-scale re-baseline**
- Depends: target benchmark hardware availability (isolated, frequency-pinned machine able to
  hold the 10M × 512 corpus plus harness working set)
- Suggested role: performance engineer
- Ownership: §5 metric table baseline values (replacing FDB-022's declared-scale values)
- Deliverable: re-capture every §5 baseline at the full 10M × 512 contract corpus on the target
  machine using the FDB-021 harness unchanged; tighten or re-record the reproducibility-variance
  bound under pinned-frequency isolation; refresh `docs/baselines/` artifacts
- Validation: reruns on the target machine reproduce within the tightened bound
- Exit criterion: §5 baseline cells carry contract-scale values with artifacts; hard-ceiling
  gates (p50 ≤ 2 ms / p99 ≤ 8 ms / recall ≥ 94%) evaluated against contract-scale numbers for
  G2 acceptance

### Wave 5 — index ladder (serial chain)

**FDB-030 — LanceDB substrate integration**
- Depends: FDB-022
- Suggested role: integration engineer
- Ownership: index-substrate module exclusively (only module allowed to import LanceDB/Arrow,
  ADR 0002)
- Deliverable: HNSW + IVF-PQ consumption behind the internal seam; version pins per G-Lance
  outcome
- Validation: round-trip build/query/index-removal tests through the seam
- Exit criterion: both index families usable via the seam with zero lancedb imports elsewhere
  (enforced by lint/grep audit)

**FDB-031 — Index ladder auto-select**
- Depends: FDB-030
- Suggested role: search engineer
- Ownership: ladder-selection logic in the index-substrate seam
- Deliverable: pure selector `row count -> {exhaustive, HNSW, IVF-PQ}` honoring < 10k /
  < 1M thresholds; creation-time override; background build triggered past ~50k rows
- Validation: exhaustive unit tests of the selector at boundary counts; wiring smoke test
- Exit criterion: selector provably matches ratified thresholds at ±1 row of each boundary;
  override respected; background build observable past ~50k

**FDB-032 — Probe/ef_search calibration + SearchOptions**
- Depends: FDB-031
- Suggested role: performance engineer
- Ownership: calibration + options plumbing in search/index seam
- Deliverable: deterministic auto-calibration of Probe count and ef_search from sampled data;
  `SearchOptions` escape hatch overriding them
- Validation: recorded harness comparisons of calibrated defaults vs naive fixed defaults
- Exit criterion: calibrated defaults Pareto-dominate naive fixed defaults in a cited run —
  recall@10 ≥ naive AND p50 ≤ naive AND p99 ≤ naive on the identical fixture; overrides
  verifiably take effect

### Wave 6 — lifecycle

**FDB-040 — Compaction**
- Depends: FDB-013, FDB-016, FDB-031 (merged Segments re-index under the ladder)
- Suggested role: storage/lifecycle engineer
- Ownership: lifecycle/Compaction module exclusively
- Deliverable: background per-Table job triggering at accumulated-change
  max(1% of Table rows, 100k) or ≥ 4 Deltas; absorbs Deltas; physically drops Tombstoned
  vectors when their ratio exceeds 20%; manual `compact()`; §13 interpretation confirmed at
  design time
- Validation: crafted-table trigger tests; post-merge oracle equivalence; concurrent-search
  consistency during merge
- Exit criterion: all M4 gate bullets demonstrated in tests; merged results identical to
  oracle on shared fixtures

### Wave 7 — hardening (ordered within wave)

**FDB-050 — Admission control**
- Depends: FDB-014 (stable search entry point)
- Suggested role: concurrency specialist
- Ownership: admission-control module exclusively
- Deliverable: search-admission semaphore sized ~2× logical cores; overflow returns `Busy`
  immediately; no queue exists (ADR 0007)
- Validation: saturation test asserting immediate `Busy` under overload
- Exit criterion: zero queueing structures present; capacity constant documented; admission is
  a single non-blocking try-acquire (structural proof by code inspection); saturation test
  asserts immediate `Busy` return

**FDB-051 — Error/crash hardening sweep**
- Depends: FDB-050
- Suggested role: reviewer/auditor (cross-cutting; proposes fixes back to owners)
- Ownership: audit scripts + integration-test tree only (no production-file ownership)
- Deliverable: automated unwrap/expect/panic audit outside `#[cfg(test)]`; public-API fuzz/
  property campaign extended with byte-mutation fuzzing over Segment corpora targeting the
  storage reader; transitive `unsafe`-surface inventory of Arrow/Lance dependencies;
  crash-recovery tests proving ADR 0003 semantics
- Validation: audit reports + fault-injection suite + mutation-fuzz runs
- Exit criterion: audits clean; neither API nor mutated-corpus fuzzing finds panics; each
  Segment provably committed-or-absent under injected crashes

### Wave 8 — observability

**FDB-060 — Optional tracing feature**
- Depends: FDB-051
- Suggested role: tooling engineer
- Ownership: observability module + feature-flag definition
- Deliverable: feature-gated spans + histograms emitted via gated macros; default build
  carries none
- Validation: grep/compile audit proving no unconditional instrumentation; enabled-build
  golden workload capture
- Exit criterion: disabled build compiles with zero tracing calls reachable; enabled build
  emits expected spans/histograms for the golden workload

### Wave 9 — tuning

**FDB-070 — SLO tuning campaign + allocator decision**
- Depends: FDB-022 (baselines), FDB-032 (knobs), FDB-060 (instruments)
- Suggested role: performance engineer
- Ownership: tuning configs + campaign reports; allocator feature flags
- Deliverable: systematic sweep over ladder/calibration knobs toward §2 targets; allocator
  comparison (mimalloc vs jemalloc) as compile-time features
- Validation: cited harness runs per configuration
- Exit criterion: targets met, or misses documented with evidence and escalated to G3;
  allocator ratified at G4; hard-ceiling gates active in CI

### Deferred backlog (unscheduled — no IDs assigned)

Async facade (ADR 0004 "later"); networked wrapper crate (ADR 0001 "later"); FFI/C ABI +
language bindings; string-parsed filters, regex/array/geo predicates; snapshots/time-travel;
custom SIMD kernels beyond profile-proven need (ADR 0002 consequence); MVCC/multi-tenancy/
distribution (ADR 0005 exclusion).

## 10. Dependency graph and critical path

Edges (predecessor → successor): 001→002→{003, 004, 010}, 010→011, 010→012,
011+012→{013, 015}, 013+015→014, 014→016, 016→020→021→022→030→031→032, 004→030,
013+016+031→040, 014→050, 050→051, 051→060, 022+032+060→070.

**Critical path:** two terminal chains converge on FDB-070; both traverse FDB-001 → FDB-002 →
FDB-010 → FDB-011 → FDB-012 → FDB-013 → FDB-014, then split:
(A) FDB-016 → FDB-020 → FDB-021 → FDB-022 → FDB-030 → FDB-031 → FDB-032 → FDB-070, and
(B) FDB-050 → FDB-051 → FDB-060 → FDB-070. Which is longer is unknowable until velocity data
exists (M2 onward); schedule both as load-bearing. FDB-040 (Compaction) has no successors and
is a leaf — its delays never delay the terminal chain. No dates are attached anywhere:
velocity is unknown until M2 instruments the work.

## 11. Parallel waves and integration gates

| Wave | Items (parallel unless noted) | Non-overlapping file ownership | Integration gate |
|------|-------------------------------|--------------------------------|------------------|
| 0 | 001 → 002 (serial) | VCS metadata, then manifests/skeleton | four gate commands green; G1 input ready |
| 1 | 003 ∥ 004 ∥ 010 ∥ 011 ∥ 012 | CI files ∥ spike dir ∥ errors ∥ table mgmt ∥ storage | modules compile independently; spike memo recorded; full suite green |
| 2 | 013 ∥ 015 | write path ∥ concurrency | commit-under-read stress green |
| 3 | 014 → 016 (ordered) | search/predicates ∥ (then) tombstone logic | M1 exit suite green |
| 4 | 020 → 021 → 022 (serial) | corpus tools ∥ (then) harness ∥ (then) baselines | M2 gate; G2 convened |
| 5 | 030 → 031 → 032 (serial) | index seam (single owner chain) | seam-isolation audit clean; M3 gate |
| 6 | 040 alone | lifecycle | M4 gate |
| 7 | 050 → 051 (ordered) | admission ∥ (then) audit/test tree | M5 gate |
| 8 | 060 alone | observability | M6 gate |
| 9 | 070 alone | tuning configs/reports | M7 gate |

Standing rule: after every merge re-run the four gate commands; once FDB-022 exists, also
re-run the harness for any change touching write/search/index modules before merging.

## 12. Risks and decision gates

| Risk | Trigger (observable) | Mitigation | Owning item |
|------|----------------------|------------|-------------|
| R1 LanceDB API churn | pinned upgrade breaks compile/tests mid-wave; security fix forces bump | single import seam (FDB-030 lint-audited); pin policy at G-Lance; upgrade drill each milestone gate | FDB-030 |
| R2 mmap-vs-Lance-format mismatch | Ferrite-owned sidecar structures (footer/Tombstone bitmaps) or zero-copy assumptions diverge from `.lance` format evolution | keep sidecars as separate Ferrite-controlled files keyed by Segment id, never inside `.lance` payloads; format conformance test on every dependency bump | FDB-012, FDB-030 |
| R3 recall unreachable at latency budget | frontier sweep finds no Probe/ef_search setting meeting recall@10 ≥ 96.5% at p99 ≤ 4.5 ms | G3 escalation: recalibrate defaults, revisit ladder thresholds or corpus realism via ADR amendment; targets change only through documented gates, never silently | FDB-070 |
| R4 single-writer bottleneck found late | FDB-022 ingest throughput makes corpus loading impractically slow, or Commit queue depth starves writers in stress tests | detect early (ingest rate is an explicit FDB-022 output); reader-lock-free snapshot swap from day one (FDB-015); if confirmed, propose caller-side sharded writes per ADR 0005 via amendment | FDB-015, FDB-022 |
| R5 Delta brute-force cost dominates p99 between Compactions | histograms correlate p99 regressions with Delta count/size | chunk bound (~64k–128k) caps Delta size; trigger thresholds reviewed at M4 with data; amendment path if measurement demands | FDB-040, FDB-070 |

**Decision gates**

| Gate | When | Decides |
|------|------|---------|
| G1 | with FDB-002 | ✓ held 2026-08-23: layout ratified (U4); scopes remapped to paths; Tombstone type → Storage (already honored by FDB-012); U5–U7 and §13 items 1/3/4/6 decided (outcomes in §4) |
| G2-entry | before FDB-020 starts | benchmark environment spec: hardware/OS class, caller-concurrency level, top-k, warmup/query-count methodology, cache-state control, reproducibility variance bound; peak-RSS target row. Held late (with FDB-022, 2026-08-24): environment pinned to the dev machine at declared reduced scale — see `docs/baselines/ENVIRONMENT.md` |
| G2 | after FDB-022 | accept baselines into §5 against the G2-entry spec; set filtered-recall gate values (U2); confirm ceilings enforceable in CI. Acceptance of hard-ceiling gates against contract scale additionally awaits FDB-023 (10M × 512 re-baseline) |
| G-Lance | after the FDB-004 memo, with FDB-030 | LanceDB/Arrow version pinning strategy (U3); disposition of any failed spike capability |
| G3 | after FDB-032 first results, again in FDB-070 | recall-vs-latency frontier review; ADR-amendment escalations |
| G4 | during FDB-070 | allocator default by harness evidence only (U1) |

## 13. Open questions (escalated, not silently resolved)

1. Delete-of-an-unknown-id: ~~succeed-and-ignore or dedicated error?~~ **Resolved at G1**:
   succeed-and-ignore (the taxonomy deliberately has no NotFound variant).
2. Exact Compaction trigger quantity: "max(1% of Table rows, 100k)" is read here as
   accumulated changed rows since last Compaction; confirm interpretation at FDB-040 design.
3. SearchOptions shape: ~~unspecified~~ **Resolved at G1**: `{ top_k: u32 (default 10,
   max 1_000), probes: Option<u32>, ef_search: Option<u32> }` — FDB-032 builds to this.
4. String columns: ~~declare/store/retrieve-but-unfilterable or excluded?~~ **Resolved at
   G1**: declared, stored, retrievable — not filterable in v1 (the assumed reading held).
5. SIFT1M acquisition/vendor policy for the sanity run (ADR 0006 mandates it, sourcing
   undefined) — resolve by FDB-021.
6. Publication channel: ~~crates.io vs private registry~~ **Resolved at G1**: private
   registry for now; revisit crates.io at 1.0.
