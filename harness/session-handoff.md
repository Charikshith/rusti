# Session Handoff

## What Was Done This Session
- **Merged model config** into `config.rs` — single `--use NAME`/`--add`/`--list` command, interactive add when model.json is empty, EOF-safe prompts
- **CLI polish** — added `--self-test` flag, `.gitignore` for model.json, improved error messages with `--use`, fixed ArgMatches borrow
- **Built self-test** — fake SSE server (2 requests: tool-call then final answer), sync tool assertions, session tree id/parentId/select checks
- **Implemented session persistence** — `session.rs` with Entry/Session tree, incremental save after each round, `--resume` flag
- **Built session tree browser** — `tree.rs` with pi-style main-screen diff renderer + plain numbered list fallback, `--tree` flag
- **Merged into main.rs** — added --tree/--resume flags, reordered so tree browse precedes model resolution
- **Dropped ratatui** — replaced with custom ANSI TUI (direct escape sequences via crossterm), binary 2.8→2.6 MB
- **Split TUI module** — `tui/{mod,app,render,plain}.rs`
- **Fixed TUI flickering** — root cause: `Clear(All)` on every frame + bottom border past terminal height; fixed with CSI 2026 synchronized output, differential rendering, cursor home + overwrite-in-place
- **Pi-style TUI** — main screen (no alternate screen), transcript stays in scrollback, diff rendering with RenderState, atomic frames
- **Live tested** against localhost:20128 with mimo-v2.5-pro and cx/gpt-5.4-mini

## Next Steps
1. Context compaction — summarize older messages when context window fills
2. `reasoning_content` field support for thinking models (QwQ, DeepSeek R1)
3. Command timeouts — kill long-running `run_command` after N seconds
4. TLS support — `https` endpoint support for LLM client
5. unicode-width for CJK/emoji word wrap

## Key Files
- `src/main.rs` — CLI flags, config resolution, session/task dispatch
- `src/config.rs` — model.json load/save/resolve/ask_profile
- `src/session.rs` — Entry/Session tree, save/load, select
- `src/tree.rs` — session tree browser (main-screen diff + plain list)
- `src/tui/mod.rs` — ANSI helpers, word_wrap, entry point
- `src/tui/app.rs` — App struct, main-screen event loop
- `src/tui/render.rs` — differential renderer with RenderState, CSI 2026
- `src/tui/plain.rs` — non-interactive fallback
- `src/ai_core/mod.rs` — Event enum, SINK, run_agent, tool dispatch, self_test
- `src/ai_core/llm.rs` — Client, chat_stream (reqwest SSE)
- `src/ai_core/tools.rs` — 5 tools: read/write/edit file, run command, ask user

## Gotchas
- `config.rs` is two files with same name — `src/config.rs` (the real one) and `config.rs` at root (orphan, can delete)
- `llm.rs` and `tools.rs` at root are orphans from the module split — safe to delete
- `model.json` has API key — gitignored, don't commit
- Session agent role is "ai" (not "assistant") — LLM maps it at send time
- `SINK` is OnceLock — can only be set once per process; self-test uses it too

## Deferred
- `add_profile()` merge: `--url`/`--key`/`--model` still resolve the default directly; `add_profile()` exists but is only used by `--add`
- TLS: reqwest default-tls not enabled — only `http://` endpoints work
