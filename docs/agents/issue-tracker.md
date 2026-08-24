# Issue tracker: Linear

Issues and specs for this repo live in **Linear**.

## Command and scope

- Command: `/home/harlan/.cache/.bun/bin/linear`
- Workspace: `harlanljones`
- Team: `HJ`
- Project: `Ferrite DB`

Run the command's `--version` and `--help` once at the start of a tracker session. The installed CLI's help is authoritative. If it is unavailable, do not substitute GitHub issues, local markdown, or direct API calls; report the setup gap and continue work that does not need the tracker.

Credentials load from `$HOME/.linear.toml` into `LINEAR_API_KEY`; never print or store the token.

## Work-item mapping

ROADMAP.md is the source of truth. Every FDB-nnn work item has exactly one Linear issue titled `FDB-nnn <name>`. Dependency edges from ROADMAP §10 become native Linear `blocked-by` relations. A ticket is frontier work only when open, unassigned, and free of open blockers — list order and the `ready-for-agent` label do not prove unblocked status.

Labeling policy: `ready-for-agent` is applied when an item's wave becomes near-term and its spec is complete (no pending gate decisions). Milestones M0–M7 live in ROADMAP §7; mirror them here only if cycle/milestone tracking is requested.

## States and labels

Linear workflow states and labels are separate. Canonical triage roles such as `ready-for-agent` are labels; applying one does not move workflow state unless the invoking skill says so.

The triage label mapping lives in `docs/agents/triage-labels.md`.

## Common operations

- Create: `linear issue create --no-interactive --team HJ --project "Ferrite DB" --title "..." --description-file <path>`
- Read: `linear issue view [ID] --json --no-download`
- Query: `linear issue query --team HJ --all-states --all-assignees --json`
- Comment: `linear issue comment add [ID] --body-file <path>`
- Incremental labels: `linear issue update [ID] --add-label "..."` / `--remove-label "..."`
- Claim: `linear issue update [ID] --assignee self`
- Complete: `linear issue update [ID] --state completed`
- Dependencies: `linear issue relation add [BLOCKED] blocked-by [BLOCKER]`

Use Markdown files for multi-line descriptions and comments (temporary files outside the repo; remove only those exact files afterward).

## Pull requests as a triage surface

**PRs as a request surface: no.** This repo has no remote yet.
