#!/usr/bin/env sh
set -eu
out="$(cargo tree -p story-core --all-targets 2>/dev/null)"
for f in ratatui crossterm reqwest; do
  if echo "$out" | grep -q "$f"; then
    echo "FAIL: story-core depends on forbidden crate '$f'" >&2
    echo "$out" >&2
    exit 1
  fi
done
echo "OK: story-core has no UI/HTTP dependencies"
