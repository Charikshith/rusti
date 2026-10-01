#!/bin/sh
# Proves the harness log convention is merge-safe: two features added in parallel from the
# same base (each with its harness/features/, harness/progress/ and harness/memory/journal/
# entry, committed through .githooks/pre-commit) merge with no conflict. A control run of the
# old convention (both append to harness/progress.md) must conflict, so the check is not vacuous.
# Also checks every harness/features/*.json is named after its own id.
# Run from the project root; init.sh runs it.
set -e

for f in harness/features/*.json; do
  [ -e "$f" ] || continue
  id=$(basename "$f" .json)
  grep -q "\"id\": *\"$id\"" "$f" || { echo "FAIL: $f does not have \"id\": \"$id\""; exit 1; }
done

root=$(pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
export GIT_AUTHOR_NAME=check GIT_AUTHOR_EMAIL=check@local GIT_COMMITTER_NAME=check GIT_COMMITTER_EMAIL=check@local
g() { git -c core.autocrlf=false -c init.defaultBranch=base "$@" >/dev/null 2>&1; }

g init
cp -r "$root/harness" "$root/.githooks" .
mkdir -p src
echo a > src/a.rs
echo b > src/b.rs
g config core.hooksPath .githooks
g add -A
g commit --no-verify -m base

feature() { # $1 = branch, $2 = src file
  g checkout -q -b "$1" base
  echo "$1" >> "src/$2"
  mkdir -p harness/features harness/progress harness/memory/journal
  printf '{\n  "id": "feat-%s",\n  "status": "done"\n}\n' "$1" > "harness/features/feat-$1.json"
  echo "# $1" > "harness/progress/2026-10-02-feat-$1.md"
  echo "## 2026-10-02 - $1" > "harness/memory/journal/2026-10-02-feat-$1.md"
  g add -A
  g commit -m "$1" || { echo "FAIL: pre-commit hook rejected a convention-following commit ($1)"; exit 1; }
}
feature alpha a.rs
feature beta b.rs
g checkout -q base
g merge --no-ff -m alpha alpha
g merge --no-ff -m beta beta || { echo "FAIL: two parallel features conflicted"; exit 1; }

# Control: the old convention, both appending to the shared log, must conflict.
g checkout -q -b old-a alpha~1
echo "## old a" >> harness/progress.md && g commit -am old-a --no-verify
g checkout -q -b old-b alpha~1
echo "## old b" >> harness/progress.md && g commit -am old-b --no-verify
g checkout -q old-a
if g merge -m old old-b; then echo "FAIL: control merge did not conflict, check is vacuous"; exit 1; fi

echo "PASS: parallel harness entries merge cleanly (old shared-append convention conflicts)"
