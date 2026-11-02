#!/bin/bash
# Commit ./screenshots/*.png as docs/screenshots/<platform>/ to the branch
# the workflow runs on (screenshots.yml), retrying when another platform's
# job pushed first.
set -euo pipefail
platform=$1
shopt -s nullglob
files=(screenshots/*.png)
[ ${#files[@]} -gt 0 ] || { echo "No screenshots."; exit 0; }
dest="docs/screenshots/$platform"
mkdir -p "$dest"
cp "${files[@]}" "$dest/"
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add "$dest"
git commit -q -m "Screenshots: $platform" || { echo "Unchanged."; exit 0; }
branch=${GITHUB_REF_NAME:?}
for i in 1 2 3 4 5; do
  git pull -q --rebase origin "$branch" && git push -q origin "HEAD:$branch" && exit 0
  sleep $((i * 4))
done
exit 1
