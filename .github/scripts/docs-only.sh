#!/usr/bin/env bash
# Reads changed paths, one per line, and prints true when every one is
# documentation that no build, test, proof or example reads; otherwise
# false. No paths prints false, so an unknown change set runs every check.
set -euo pipefail

docs=false
while IFS= read -r path; do
  [[ -z $path ]] && continue
  case $path in
    # quark-macros' docs_reference_matches_the_tables test reads this one.
    docs/guide/writing-views.md) echo false; exit 0 ;;
  esac
  # Top-level and per-crate notes and the guide. vendor/ and tools/ are
  # left out: taffy's README is its crate docs (doctests) and syntax-pack
  # builds its samples.
  if [[ $path =~ ^[^/]+\.md$ || $path =~ ^crates/[^/]+/[^/]+\.md$ || $path =~ ^docs/.+\.md$ ]]; then
    docs=true
  else
    echo false
    exit 0
  fi
done
echo "$docs"
