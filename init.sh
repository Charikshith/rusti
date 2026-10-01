#!/bin/bash
set -e

# Activates .githooks/pre-commit on every run (added by enrich-harness.mjs).
if [ -f .githooks/pre-commit ] && git rev-parse --git-dir >/dev/null 2>&1; then
  hooks_path="$(git config --get core.hooksPath 2>/dev/null || true)"
  if [ -z "$hooks_path" ]; then
    git config core.hooksPath .githooks
  elif [ "$hooks_path" != ".githooks" ]; then
    echo "note: core.hooksPath is $hooks_path (another hook manager); .githooks/pre-commit is not active"
  fi
fi


echo "=== Harness Initialization ==="

# Environment contract runs before anything else and reports separately from test output:
# a missing tool is not a failing test, and conflating the two sends the next session
# debugging code that was never broken.
#
# Path is harness/environment.md, not environment.md: init.sh stays at the project
# root because it is invoked as ./init.sh, but the contract it reads is harness state.
ENV_CONTRACT="harness/environment.md"
if [ -f "$ENV_CONTRACT" ]; then
  echo "=== Environment contract ==="
  ENV_FAILED=0
  while IFS='|' read -r _ requirement check _; do
    requirement="$(echo "$requirement" | sed 's/^ *//;s/ *$//')"
    check="$(echo "$check" | sed 's/^ *//;s/ *$//;s/^`//;s/`$//')"
    case "$requirement" in ''|Requirement|---*) continue ;; esac
    [ -z "$check" ] && continue
    if eval "$check" >/dev/null 2>&1; then
      echo "  PASS  $requirement"
    else
      echo "  FAIL  $requirement   (check: $check)"
      ENV_FAILED=$((ENV_FAILED + 1))
    fi
  done < "$ENV_CONTRACT"
  if [ "$ENV_FAILED" -gt 0 ]; then
    echo "Environment contract failed ($ENV_FAILED unmet). This is the machine, not the code."
    exit 1
  fi
fi

echo "=== harness merge-safety check ==="
./harness/check-merge-safe.sh

echo "=== cargo test ==="
cargo test

echo "=== Verification Complete ==="
echo ""
echo "Next steps:"
echo "1. Read harness/feature_list.json and harness/features/ to see current feature state"
echo "2. Pick ONE unfinished feature to work on"
echo "3. Implement only that feature"
echo "4. Re-run verification before claiming done"
