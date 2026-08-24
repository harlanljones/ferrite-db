#!/usr/bin/env bash
# FDB-051 — panic-prone call audit.
#
# Enforces AGENTS.md §6 gate 2: no `unwrap()`/`expect()`/`panic!`/etc. outside
# `#[cfg(test)]` modules may cross into production code. Exits non-zero (fails
# CI) if any such call is found in production paths, so the audit is
# machine-checkable rather than a human review step.
#
# Usage: tools/audit/panic_audit.sh [SRC_ROOT]
set -euo pipefail

SRC_ROOT="${1:-crates/ferrite-db/src}"
# Passed through ENVIRON so awk does not re-process the backslash escapes.
export PAT='\.unwrap\(\)|\.expect\(|\.unwrap_err\(|panic!|unreachable!|unimplemented!|todo!'

found=0
while IFS= read -r file; do
  # awk strips line comments, records the line where #[cfg(test)] starts, and
  # flags panic-prone calls that appear strictly before that marker.
  if awk -v file="$file" '
    BEGIN { pat = ENVIRON["PAT"] }
    /#\[cfg\([^]]*test[^]]*\)\]/ { test_start = NR }
    {
      sub(/\/\/.*/, "")
      if (test_start == 0 || NR < test_start) {
        if ($0 ~ pat) { print "PRODUCTION PANIC-PRONE: " file ":" NR ": " $0; found=1 }
      }
    }
    END { exit found }
  ' "$file"; then
    :
  else
    found=1
  fi
done < <(find "$SRC_ROOT" -name '*.rs')

if [ "$found" -ne 0 ]; then
  echo "AUDIT FAILED: production code contains panic-prone calls"
  exit 1
fi
echo "AUDIT CLEAN: no unwrap/expect/panic outside #[cfg(test)] modules"
exit 0
