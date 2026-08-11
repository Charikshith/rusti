# Progress: rustypi

## Current State
**Phase**: MVP Complete — Agent Live-Tested
**Last Verified**: 2026-04-10 (cargo build --release, --self-test, live LLM test)

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
