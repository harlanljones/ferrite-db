#!/usr/bin/env bash
# FDB-030 exit criterion: zero LanceDB/Arrow/async-runtime imports anywhere
# outside the index-substrate seam (crates/ferrite-db/src/index_substrate.rs).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SEAM="crates/ferrite-db/src/index_substrate.rs"
found=0

while IFS= read -r -d '' file; do
  rel="${file#"$ROOT"/}"
  [ "$rel" = "$SEAM" ] && continue
  # Match Rust import paths and qualified type uses only; prose mentions of
  # "LanceDB" in docs/comments are fine and must not trip this audit.
  if grep -nE '(^use |[^[:alnum:]_])(lancedb|arrow_array|arrow_schema|arrow_cast|tokio|futures)::' "$file"; then
    echo "SUBSTRATE LEAK: $rel references substrate-only types"
    found=1
  fi
done < <(find "$ROOT/crates" -name '*.rs' -not -path '*/target/*' -print0)

# Substrate-only dependency declarations stay confined to the owning manifest.
for manifest in "$ROOT"/crates/*/Cargo.toml; do
  rel="${manifest#"$ROOT"/}"
  [ "$rel" = "crates/ferrite-db/Cargo.toml" ] && continue
  if grep -nE '^(lancedb|tokio|futures|arrow)[[:space:]]*=' "$manifest"; then
    echo "SUBSTRATE LEAK: $rel declares a substrate-only dependency"
    found=1
  fi
done

if [ "$found" -eq 0 ]; then
  echo "AUDIT CLEAN: no substrate imports outside $SEAM"
fi
exit "$found"
