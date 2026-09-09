# Progress: rustypi

## Current State
**Phase**: MVP Complete — Agent Live-Tested
**Last Verified**: 2026-09-09 (cargo build --release 0 warnings, cargo test 7 passed, --self-test with and without ripgrep on PATH, ./init.sh)

## Session 2026-09-09: Markdown rendering in model prose (feat-024)
- Reported as "why is the text not getting rendered correctly" — it was: the model answers in markdown and
  the TUI printed it verbatim, so `###`, `**` and backticks all showed as literal characters
- `line_style` grew two more kinds: `b'k'` for rows that keep the per-row glyph colouring (tool markers and
  user queries) and `b'm'` for model prose, which takes the markdown pass. Reasoning and stats unchanged
- The ordering problem: stripping markers changes a line's length, so styling before wrapping makes the wrap
  count asterisks it is about to delete, and styling after wrapping breaks spans at row boundaries. Solution
  is `md_line` returning *visible chars + style runs as char ranges*, `wrap_ranges` wrapping those chars into
  ranges, and `md_row` emitting a range with the runs that cover it. Wrapping therefore measures real columns
- `md_row` recomputes the active style per char, so `code` nested inside a bold heading restores the bold
  rather than resetting to plain
- Fenced blocks: the ``` lines are not drawn, the body is coloured and truncated rather than reflowed —
  reflowing code is worse than clipping it. Markdown rows skip `truncate_str` in `draw`, since they are
  already wrapped and cutting them would slice an escape sequence in half
- Also fixed here: the user-query branch built "1›hello", skipping the caret *and* the space after it, and
  it now renders plain white with no accent colours
- Corrected my own docs: 16 tools, not 17 (counted the schemas)

## Session 2026-09-09: Fix — status markers rendered without colour
- Reported as "the ✓ isn't green". Root cause was not in `colorize_row` but in `word_wrap`: it splits on
  `' '`, and a leading indent yields empty tokens that hit the `line.is_empty()` arm and get dropped. Every
  row therefore reached `colorize_row` with its `"  "` gone, matched no branch, and fell through to the
  default arm — which re-adds two spaces, so the rows *looked* right and only the colour was missing
- One bug, seven symptoms: ✓ green, ✗ red, ⠋ and ⚠ yellow, ℹ blue, and the dim `│` reasoning and `·` stats
  rows were all plain. Only user lines survived, because `N› ` has no leading space
- Fix: `word_wrap` splits the indent off, wraps the body against `width - indent`, and re-applies the indent
  to the first row. Charging the indent against the width matters — otherwise `truncate_str` clips the tail
- Two tests: one on `word_wrap` for the indent, one in `render.rs` running a row through the real
  `word_wrap` → `colorize_row` pipeline, since the bug was the two halves disagreeing rather than either
  being wrong alone

## Session 2026-09-09: Safety + workflow tools (feat-023)
- Permission gate in `run_agent` before `dispatch`: gated tools ask through the existing `Event::Ask` (so the TUI needed no change) with `y / n / a(lways for this tool)`; `decide()` is the pure decision so it is under test. `--yolo` / `RUSTYPI_YOLO=1` skips. Piped stdin -> "no answer given" -> denied, by design
- `tools::guard()` refuses writes outside the canonicalized cwd; it canonicalizes the deepest *existing* ancestor and re-appends the rest so not-yet-created files and `../` are judged on the real path. Applied to write/multi_edit/delete/move only; reads stay open (recorded as Read-Side Sandbox in open-work)
- New tools: `delete_file`, `move_file` (refuses to clobber), `run_background` / `job_output` / `job_stop` (shared 1 MB ring buffer fed by pump threads; `job_output(0)` lists; stop uses `taskkill /T /F` on Windows because `cmd /C` wraps the real process), `todo` (whole-list replace, rendered `☑ ◐ ☐` and emitted as `Event::Text` lines), `delegate` (one level, `Box::pin(run_agent)` on a fresh `Session` under `.rustypi/sessions/sub-*.json`, `DEPTH` guard refuses nesting)
- `dispatch` now takes `client` and `cancel` for delegate; `edit_file`/`run_command` share `shell()`; `truncate` is `pub`
- Self-test flips `YOLO` on for the fake-server run (a prompt with no stdin would deny the scripted `run_command`) and off again around the guard checks

## Session 2026-09-09: Iteration cap + project instructions (feat-022)
- `MAX_ITERS` is now an `AtomicUsize` defaulting to 50 (was a const 10 that killed any real read/edit/test/fix task); `--max-iters N` or `RUSTYPI_MAX_ITERS` overrides, floor 1. Flag is in `VALUE_FLAGS` so its value is never mistaken for the task
- `system_prompt()` appends the working directory's `AGENTS.md` (fallback `RUSTYPI.md`, `CLAUDE.md`; blank files skipped, 20k cap). Read every turn — the system entry already refreshed each turn, so instruction edits are live mid-session

## Session 2026-09-09: Native search + edit tools (feat-021)
- Tools grew from 5 to 9: `grep`, `glob`, `list_dir`, `multi_edit`; `read_file` gained `offset`/`limit` (ranged reads are line-numbered, whole reads stay raw); `run_command` gained `timeout_secs` (default 120, polls `try_wait`, kills at deadline)
- grep/glob shell out to ripgrep when it is on PATH (regex, .gitignore-aware, parallel) and fall back to a std-only walk with a fixed skip list (`.git`, `target`, `node_modules`, …) and a home-grown `*`/`?`/`**` matcher — no new crates, binary size unchanged. Fallback grep is substring-only and says so in its "no matches" line
- `edit_file` is now a one-entry `multi_edit`; edits apply in memory and write once, so a bad edit N leaves the file untouched
- Self-test covers both search paths: run once normally, once with `PATH=/c/Windows/System32` to hide rg
- Removed the three matching open-work items (Command Timeouts, Native Search Tools, Multi-Edit)

## Session 2026-09-09: /reload (hot rebuild + relaunch)
- Added TUI `/reload`: rebuilds rustypi from `CARGO_MANIFEST_DIR` (works even when cwd is some other project being coded on) into `target/reload`, then runs a uniquely-named *copy* of the binary — the built file is never the running one, so cargo can always overwrite it (Windows locks a running exe). Relaunch is a true `exec` on Unix; on Windows the old process waits as a thin wrapper (exiting after `spawn` let the shell take its prompt back and fight the child for console input). A first version ping-ponged two build dirs — that breaks on the 3rd reload once parents wait, hence the copy approach
- Relaunch preserves: session (saved first, `--resume` only if that save succeeded — a turn that failed before its first save otherwise pointed `--resume` at a missing file), the live model/url/key (explicit flags, survives a mid-session `/use` even if model.json disagrees), and the visible transcript (now reconstructed from `session.json` on TUI startup — previously a resumed session showed a blank screen). `--tree` is stripped from the relaunch argv
- Added `/model`: lists saved profiles with the active one marked, or `/model <name>` switches (shares `switch_model` with `/use`)
- Build failures are non-destructive: reported inline (tail of stderr), current session/process untouched; gated on `app.done` so it can't race an in-flight turn
- Fixed two pre-existing bugs found while building this: `job_tx` outlived the agent-thread join causing a hang on quit; `--tui --resume` blocked on a stdin prompt before the TUI ever started
- `SYSTEM_PROMPT` now refreshes on a resumed session's system entry every turn (was frozen at session creation) — so editing it + `/reload` actually takes effect

## Session 2026-04-10: TUI Experience + Interrupt
- Removed `✓ Task completed` on every successful turn (architect decision: success is self-evident; only failures get a marker)
- Built all 5 TUI experience items: semantic status symbols/colors, animated spinner, input editing + history, scrollback, slash commands (/use /resume /tree)
- Built pi-style interrupt key model: Esc interrupts (cooperative AtomicBool), Ctrl+C clears input, Ctrl+D exits when empty
- Compared against pi interactive mode source; remaining gaps recorded in harness/open-work.md

## Session 2026-09-09: HTML prototype parity (feat-013 … feat-018)
Implemented the 8 gaps between prototype/rustypi-tui.html and the Rust TUI, one at a time:
1. Tool wall time — `Event::ToolEnd` carries `ms`, measured around dispatch (and around /reload's cargo build); `ai_core::took` formats 450ms / 1.6s
2. Ctrl+C twice to exit — first press clears input + cancels the turn, second within 2s quits, yellow `press ctrl+c again to exit` while armed, any other key disarms
3. `/quit` — `handle_command` returns bool; true means Exit::Quit
4. Per-turn stats — `· N tok · N tps · N.Ns` from `Event::Usage`; server `usage.completion_tokens` when sent, else a `~`-marked chars/4 estimate; tps clock starts at the first token so tool waits don't dilute it
5. Thinking block — `reasoning_content`/`reasoning` deltas stream into a dim `│ ` block, closed by the answer or a tool call, never fed back as content
6. Slash menu — lone `/word` opens it above the input: 5 rows, up/down over the whole list, Tab completes, Enter runs, Esc dismisses for that text, counter row on overflow, queued commands shown `· soon`
7. Status hint policy — `ctrl+c twice to quit` only on a fresh prompt or when scrolled off the bottom, else blank
8. Named sessions + `/resume` picker — `--session NAME` → `.rustypi/sessions/NAME.json` (root session.json stays the unnamed default), no-arg `/resume` opens an arrow picker, `/resume 2|NAME` skips it, `Event::Resumed` swaps transcript + history

Also fixed (root cause, found by the smoke run): `task_arg` took any bare argument as the task,
so `--session smoke "..."` ran "smoke" as the task — flag values are now skipped for
`--url/--key/--model/--session/--use`. Removed the newly-unused `Session::load()`.

Deliberate simplification: the picker renders as a panel above the input in the slash-menu
style, not the prototype's boxed full-screen overlay (recorded in harness/open-work.md).

Not implemented from the prototype: nothing. Its `/undo /commit /plan /test /export` rows are
listed as `· soon` and print a pointer to harness/open-work.md, which is what the prototype does.

Verification: `cargo test` 6 passed (new: stats row, slash filter, session info, task_arg),
`--self-test` OK (fake stream now includes a reasoning delta), live smoke against
127.0.0.1:8080 with `--session smoke`.

## Session 2026-09-09b: prototype refresh (feat-019, feat-020)
The prototype gained three things after the parity pass; all three landed:
- Active session name in the chrome — the status line's right slot is now `<session> · <model>`, fed by `Event::SessionName` (seeded from the session file stem, updated on resume and rename)
- `/rename <new-name>` — moves the active session to `.rustypi/sessions/<name>.json`; name check is ASCII alnum + `._-` up to 40, refuses a taken or current name, an unsaved session just repoints its path. Extracted as `rename_session()` so the file move is under test
- `/session list|switch` added to the menu as `· soon` (still queued in open-work)

Also: `/reload`'s relaunch argv now replaces a stale `--session` pair with the live session name, so a mid-session `/rename` survives the hot rebuild.

Note for future tests: `fs::rename` can't cross volumes, so session-path tests must use cwd-relative
paths (the temp dir is on another drive here) — production paths always are.

Verification: `cargo test` 7 passed, `--self-test` OK, `./init.sh` clean, live runs with
`--session smoke` and `--session two` against 127.0.0.1:8080.

## Completed (23/23 features)
1. ✅ Project Setup & Baseline — cargo build (release), cargo test, --self-test
2. ✅ Core Agent Loop — run_agent with tool dispatch, session persistence (round cap now 50, see feat-022)
3. ✅ LLM Streaming & SSE — reqwest SSE with \n\n/\r\n\r\n delimiters, per-index argument accumulation
4. ✅ Tool Implementations — the original 5: read/write/edit file, run command, ask user (extended by feat-021/023)
5. ✅ Model Config & CLI — model.json with profiles, --list/--use/--add, env vars, interactive add
6. ✅ TUI Mode — pi-style main screen, synchronized output (CSI 2026), differential rendering, plain fallback
7. ✅ Self-Test — fake SSE server, sync tool assertions, session tree checks
8. ✅ Session Persistence & Tree — tree structure (id/parentId), pi-style branching, --tree (diff renderer) + --resume
9. ✅ Live LLM Integration — tested against localhost:20128 with cx/gpt-5.4-mini and mimo-v2.5-pro
10. ✅ TUI Experience — semantic status (⠻/✓/✗/⚠/ℹ, cyan ›), animated spinner, cursor editing + history, scrollback, /use /resume /tree
11. ✅ Interrupt Key Model — Esc interrupts, Ctrl+C clears input, Ctrl+D exits; quit no longer waits for in-flight turn
12. ✅ /reload — hot rebuild from CARGO_MANIFEST_DIR + relaunch preserving session, model and transcript
13. ✅ Tool wall time — Event::ToolEnd carries ms, rendered 450ms / 1.6s
14. ✅ Ctrl+C twice to exit — first press clears input and cancels the turn, second within 2s quits
15. ✅ Per-turn stats — tokens / tps / wall from Event::Usage, server usage when sent else a marked estimate
16. ✅ Thinking block — reasoning_content deltas stream into a dim block, never fed back as content
17. ✅ Slash-command menu — filter, Tab complete, Enter run, overflow counter
18. ✅ Named sessions + /resume picker — --session NAME, arrow picker, Event::Resumed swaps transcript
19. ✅ Active session name in status line — <session> · <model>
20. ✅ /rename — moves the active session file, refuses taken or invalid names
21. ✅ Native Search & Edit Tools — grep/glob/list_dir (ripgrep when installed, std fallback), multi_edit, ranged reads, command timeout
22. ✅ Iteration Cap & Project Instructions — 50 rounds with --max-iters, AGENTS.md appended to the system prompt every turn
23. ✅ Agent Safety & Workflow Tools — permission gate (--yolo), project-root write guard, delete/move, background jobs, todo, delegate sub-agent

## Build Status
- `cargo build --release` — 0 warnings, 0 errors
- `cargo test` — 7 passed
- `--self-test` — passes; also covers the search tools on both paths (ripgrep and the std fallback, run with
  `PATH=/c/Windows/System32` to hide rg), multi_edit atomicity, the command timeout, permission decisions,
  the project-root guard, move/delete, background job lifecycle, and todo rendering
- Binary: ~3.0 MB (release), ~3,300 lines of application code
- 16 tools; ripgrep is used when on PATH and is never required
- Dependencies: serde, serde_json, tokio (current-thread), reqwest, futures-util, bytes, crossterm, is-terminal

## Verified
- Agent works against real LLM (localhost:20128, model cmc/xiaomi/mimo-v2.5-pro)
- TUI renders without flicker, differential updates, main-screen (scrollback preserved)
- Session tree branching works (id/parentId, select, save/load)
- Config profiles persist API key in model.json (gitignored)
- Plain stream fallback works for piped stdin
- Self-test runs offline (fake SSE server, no network required)

## Not Yet Verified Live
- The permission prompt end to end in the TUI (the ask path it reuses is exercised, the gate itself is not)
- `delegate` against a real model
- Any long task against the raised 50-round cap — context compaction is the likely first wall
