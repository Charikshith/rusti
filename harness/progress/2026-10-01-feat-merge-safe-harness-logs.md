## Session 2026-10-01: feat-merge-safe-harness-logs — harness logs no longer conflict across parallel PRs
- harness/feature_list.json, harness/progress.md and harness/memory/journal.md are closed to new entries
  (headers say so; old entries untouched). New work goes in harness/features/feat-<slug>.json,
  harness/progress/YYYY-MM-DD-<feature-id>.md and harness/memory/journal/YYYY-MM-DD-<feature-id>.md.
- .githooks/pre-commit accepts a harness/progress/ and harness/memory/journal/ entry; AGENTS.md startup,
  working rules, end-of-session and curation text point at the new paths.
- Verification: harness/check-merge-safe.sh (now part of ./init.sh) merges two parallel features cleanly and
  shows the old convention conflicting; it fails against the previous hook. ./init.sh passed.
