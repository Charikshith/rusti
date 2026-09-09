# Progress: rustypi

## Current State
**Phase**: MVP Complete — Agent Live-Tested
**Last Verified**: 2026-09-09 (cargo build --release, cargo test, --self-test, manual cargo build --target-dir dry run)

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

## Completed (11/11 features)
1. ✅ Project Setup & Baseline — cargo build (2.6 MB release), cargo test, --self-test
2. ✅ Core Agent Loop — run_agent with tool dispatch, max 10 rounds, session persistence
3. ✅ LLM Streaming & SSE — reqwest SSE with \n\n/\r\n\r\n delimiters, per-index argument accumulation
4. ✅ Tool Implementations — 5 tools: read/write/edit file, run command, ask user
5. ✅ Model Config & CLI — model.json with profiles, --list/--use/--add, env vars, interactive add
6. ✅ TUI Mode — pi-style main screen, synchronized output (CSI 2026), differential rendering, plain fallback
7. ✅ Self-Test — fake SSE server, sync tool assertions, session tree checks
8. ✅ Session Persistence & Tree — tree structure (id/parentId), pi-style branching, --tree (diff renderer) + --resume
9. ✅ Live LLM Integration — tested against localhost:20128 with cx/gpt-5.4-mini and mimo-v2.5-pro
10. ✅ TUI Experience — semantic status (⠋/✓/✗/⚠/ℹ, cyan ›), animated spinner, cursor editing + history, scrollback, /use /resume /tree
11. ✅ Interrupt Key Model — Esc interrupts, Ctrl+C clears input, Ctrl+D exits; quit no longer waits for in-flight turn

## Build Status
- `cargo build --release` — 0 warnings, 0 errors
- `cargo test` — passes
- `--self-test` — passes (now also asserts interrupt cancels before any network call)
- Binary: ~2.8 MB (release)
- Dependencies: serde, serde_json, tokio (current-thread), reqwest, futures-util, bytes, crossterm, is-terminal

## Verified
- Agent works against real LLM (localhost:20128, model cmc/xiaomi/mimo-v2.5-pro)
- TUI renders without flicker, differential updates, main-screen (scrollback preserved)
- Session tree branching works (id/parentId, select, save/load)
- Config profiles persist API key in model.json (gitignored)
- Plain stream fallback works for piped stdin
- Self-test runs offline (fake SSE server, no network required)
