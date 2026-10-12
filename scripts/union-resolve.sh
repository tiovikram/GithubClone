#!/usr/bin/env bash
# Resolves merge conflicts by keeping both sides (for additive conflicts in mod lists/routers),
# then drops duplicate `pub mod x;` / `.merge(x::routes())` lines the union may introduce.
# Usage: scripts/union-resolve.sh file...
for f in "$@"; do
  sed -i '' -E '/^(<<<<<<< |=======$|>>>>>>> )/d' "$f"
  awk '{ key=$0; sub(/^[ \t]+/, "", key)
         if (key ~ /^(pub )?mod [a-z_0-9]+;$/ || key ~ /^\.merge\([a-z_0-9:]+\(\)\)$/) { if (seen[key]++) next }
         print }' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
  git add "$f"
done
