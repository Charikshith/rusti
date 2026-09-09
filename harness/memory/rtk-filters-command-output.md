# RTK rewrites shell commands and summarizes their output

A hook in this environment rewrites bare commands to `rtk <cmd>` (a token-reducing
proxy). `grep` comes back as a summary — literally `37 matches in 1 files` instead of
the matching lines — and a pattern that *does* match can come back looking empty.

**Why:** In the 2026-09-09 session I ran `grep -rn '"/quit"' src/`, got nothing back, and
told the user three features "don't exist in src yet". All three were implemented in
`src/tui/app.rs` and `src/ai_core/llm.rs` at the time. The absence was an artifact of the
filter, not the codebase, and the user acted on the wrong claim.

**How to apply:** Never assert a symbol, command, or feature is missing on the strength of
a filtered `grep`. Run `rtk proxy <cmd>` for the raw output, or use the Grep tool, before
any claim of absence — and prefer `harness/feature_list.json` as the first check for
"is this built?", since it is the declared source of truth.

Also affects long-running commands: `rtk npx …` with a multi-line argument fails with
`npx: batch file arguments are invalid`; keep such arguments on one line.
