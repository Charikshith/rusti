# Progress: rustypi

## Current State
**Phase**: MVP Complete — Agent Live-Tested
**Last Verified**: 2026-09-09 (cargo build --release, cargo test, --self-test, manual cargo build --target-dir dry run)

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
