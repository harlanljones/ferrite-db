#!/usr/bin/env bash
# FDB-051 — transitive unsafe-surface + dependency inventory.
#
# ADR 0002 confines every Arrow/Lance type behind the index-substrate seam, and
# AGENTS.md §6 gate 4 requires an auditable unsafe surface. This script prints
# the current inventory so the audit can be re-run after FDB-030 lands the
# LanceDB dependency. It is intentionally a read-only report (no state change).
#
# Usage: tools/audit/unsafe_inventory.sh [REPO_ROOT]
set -euo pipefail

ROOT="${1:-.}"

echo "# FDB-051 unsafe-surface and dependency inventory"
echo
echo "## 'unsafe' occurrences in ferrite-db source"
if grep -rn "unsafe" "$ROOT/crates/ferrite-db/src"; then
  :
else
  echo "none"
fi
echo
echo "## lancedb / arrow references in source"
if grep -rn "lancedb\|arrow" "$ROOT/crates/ferrite-db/src"; then
  :
else
  echo "none (expected before FDB-030 substrate integration)"
fi
echo
echo "## lancedb / arrow in Cargo.lock (transitive, becomes relevant at FDB-030)"
if grep -in "lancedb\|arrow" "$ROOT/Cargo.lock"; then
  :
else
  echo "none yet"
fi
echo
echo "## summary"
src_unsafe=$(grep -rc "unsafe" "$ROOT/crates/ferrite-db/src" | awk -F: '{s+=$2} END {print s+0}')
echo "total 'unsafe' hits in src: $src_unsafe"
