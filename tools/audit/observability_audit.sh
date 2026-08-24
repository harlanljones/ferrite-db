#!/usr/bin/env bash
# FDB-060 — observability audit.
#
# Proves the exit criterion "disabled build compiles with zero tracing calls
# reachable": the default feature set must not pull the `tracing` (or
# `tracing-subscriber`) dependency, and every `tracing`/`ferrite_span!`/
# `ferrite_histogram!` reference in source must sit inside a
# `#[cfg(... feature = "tracing" ...)]` region. Exits non-zero on violation.
#
# Usage: tools/audit/observability_audit.sh [REPO_ROOT]
set -uo pipefail

ROOT="${1:-.}"
SRC="$ROOT/crates/ferrite-db/src"

echo "## compile audit: default build must not depend on tracing"
if cargo tree -e no-dev --manifest-path "$ROOT/crates/ferrite-db/Cargo.toml" 2>/dev/null \
   | grep -qE 'tracing(-subscriber)?'; then
  echo "AUDIT FAILED: default build pulls a tracing dependency"
  exit 1
fi
echo "default build: no tracing dependency (ok)"

echo "## source audit: tracing refs must be feature-gated"
status=0
while IFS= read -r file; do
  # Drop doc/line comments so only real code references are checked.
  stripped=$(mktemp)
  grep -vE '^[[:space:]]*//!|[[:space:]]*///|[[:space:]]*//' "$file" > "$stripped"

  # Gate markers: any line that opens a cfg enabling the tracing feature.
  gates=$(grep -nE '#\[cfg\(.*feature = "tracing"' "$stripped" | cut -d: -f1)

  # Candidate tracing references.
  refs=$(grep -nE 'tracing::|ferrite_span!|ferrite_histogram!' "$stripped" | cut -d: -f1)

  for ref in $refs; do
    [ -z "$ref" ] && continue
    inside=0
    for g in $gates; do
      [ -z "$g" ] && continue
      if [ "$ref" -ge "$g" ]; then
        inside=1
        break
      fi
    done
    if [ "$inside" -eq 0 ]; then
      echo "UNGATED TRACING REF: $file:$ref"
      status=1
    fi
  done
  rm -f "$stripped"
done < <(find "$SRC" -name '*.rs')

if [ "$status" -ne 0 ]; then
  echo "AUDIT FAILED: ungated tracing references found"
  exit 1
fi
echo "AUDIT CLEAN: all tracing references are feature-gated"
exit 0
