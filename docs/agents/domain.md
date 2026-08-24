# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root (single context; no `CONTEXT-MAP.md`).
- **`docs/adr/`**: read ADRs that touch the area you're about to work in. ADRs 0001–0007 are binding; violating one requires amending it first (AGENTS.md §2).

## File structure

```
/
├── CONTEXT.md
├── docs/
│   ├── adr/
│   │   ├── 0001-embedded-crate-not-service.md
│   │   └── …
│   └── agents/
└── crates/
    └── ferrite-db/
```

## Use the glossary's vocabulary

When your output names a domain concept (an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md` and enforced by AGENTS.md §3 — Table, Segment, Delta, Tombstone, Commit, Compaction, Metric, Predicate Tree, Probe, Metadata Schema. Don't drift to the banned synonyms listed there.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (shed with `Busy`, never queue), but worth reopening because…_
