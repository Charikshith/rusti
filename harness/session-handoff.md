# Session Handoff

## What Was Done This Session (2026-09-10, part 2)
- **Fixed feat-026 — multi-tool-call message integrity.** A session failed on *every* turn with
  `[CommandCode error: {"type":"server_error","message":"Tool result is missing for tool call call_00_7dueXDf20YUXmQlI48NA3107."}]`,
  while the same model worked through pi.
- **Root cause (rusti, not the provider):** `run_agent` added each tool result as a **sibling**
  under the assistant entry. `path_messages()` walks a single parent chain from the active leaf, so a
  turn that emitted two tool calls (`run_command` + `list_dir`) sent only the *last* result — the
  first tool call was left dangling and the API rejected the request. The broken prefix stayed on the
  active path, so every later turn failed identically.
- **Fix:** chain tool results (`parent = previous tool entry`) so all of them lie on the active path
  in order. Regression guard: `--self-test` now emits **two** tool calls in one turn and asserts
  6 messages with `msgs[3]` and `msgs[4]` both `role: "tool"`.
- Old `session.json` from the failing session is **not recoverable** (its tree genuinely lacks the
  missing entry) — start a fresh session.

## Verification (all green)
- `cargo test` — 13 passed
- `--self-test` — passes (now exercises the two-tool-call turn)

## Artifacts
- `harness/feature_list.json` — feat-026 added (26/26 done)
- `harness/memory/session-tree-path-drops-siblings.md` + index line
- `harness/progress.md`, `harness/memory/journal.md` — session records

## Next Steps (unchanged, see harness/open-work.md)
1. Context compaction — the likely first wall on long tasks
2. Permission prompt end-to-end verification in the TUI
3. `delegate` against a real model
