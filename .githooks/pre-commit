#!/bin/sh
# Enforces harness/memory/commit-is-not-session-end.md: a commit that changes anything
# outside harness/ must also update harness/progress.md and harness/memory/journal.md in
# the same commit, not afterward. See templates/index.md for what installs this.

staged=$(git diff --cached --name-only --diff-filter=ACMR)

# A lesson is the one write into harness/memory/ with no human gate, and once indexed it is
# read every session. So a new one is announced here, never blocked, so it gets read once
# before it becomes always-on context. Runs before the early exit below, because a commit
# that adds only a lesson touches nothing outside harness/.
new_lessons=$(git diff --cached --name-only --diff-filter=A \
  | grep '^harness/memory/[^/]*\.md$' | grep -vE '/(index|journal|graveyard)\.md$')
if [ -n "$new_lessons" ]; then
  echo "pre-commit: new lesson(s) — read before they become always-on context:" >&2
  echo "$new_lessons" | sed 's/^/  /' >&2
fi

echo "$staged" | grep -qv '^harness/' || exit 0

missing=""
echo "$staged" | grep -q '^harness/progress\.md$' || missing="$missing harness/progress.md"
echo "$staged" | grep -q '^harness/memory/journal\.md$' || missing="$missing harness/memory/journal.md"

if [ -n "$missing" ]; then
  echo "pre-commit: this commit changes project files but is missing:$missing" >&2
  echo "  See harness/memory/commit-is-not-session-end.md" >&2
  echo "  Bypass for non-feature changes: git commit --no-verify" >&2
  exit 1
fi
