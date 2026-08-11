# Journal

## 2026-04-10 — Session 1

### Built
- Full coding agent in Rust: async core (tokio current-thread), streaming SSE, 5 tools, TUI, config, sessions
- Pi-style TUI: main screen, synchronized output (CSI 2026), differential rendering, no alternate screen
- Session tree browser with branching (id/parentId, select, --tree, --resume)
- Self-test with fake SSE server (offline, no network)
- Model config with profiles, --use/--add/--list, API key persistence

### Fixed
- TUI flickering: root cause was `Clear(All)` every frame + border past terminal height. Fixed with overwrite-in-place + CSI 2026 + height math.
- Dropped ratatui (~200KB saved): replaced with custom ANSI renderer using crossterm escape sequences

### Tested
- Live against localhost:20128 with cmc/xiaomi/mimo-v2.5-pro and cx/gpt-5.4-mini
- All 9 features pass, 0 warnings, 0 errors, 2.6 MB binary

### Deferred
- Context compaction (summarize when context fills)
- reasoning_content (for thinking models)
- Command timeouts, TLS support, unicode-width

### 2026-08-12 — TUI experience session
- Read pi interactive-mode.js (5.4k lines) directly for the feature comparison; source reading beat guessing — key model, tool components, footer all confirmed from code
- Architect decision adopted: only annotate non-obvious turn ends; success is the answer itself, failures need ✗ + reason
- Cancellation via shared AtomicBool + per-chunk checks; plain mode (piped stdin + --tui) never sends a Job::Task — pre-existing hang, left alone (out of scope)
- Esc→interrupt vs Ctrl+C→clear vs Ctrl+D→exit: pi's exact key model; users habitually hit Ctrl+C to cancel — noted as a follow-up option
