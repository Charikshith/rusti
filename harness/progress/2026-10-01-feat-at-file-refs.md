## Session 2026-10-01: feat-at-file-refs — @ file picker and Tab path completion (F01)
- tui/app.rs: Item enum for menu rows, at_token / score_files / at_insert / path_token / path_matches /
  common_prefix / replace_span, background file index, Tab and @ key arms; render.rs draws path rows
  and an "indexing files…" row; tools.rs project_files(); one system-prompt line; help, readme § TUI,
  docs topic map, rusti-vs-pi row and open-work entries updated.
- Verification: 5 new tests (see harness/features/feat-at-file-refs.json); ./init.sh passed.
- Not built: F03 image attach through @; argument completion for /model etc. (open-work).
