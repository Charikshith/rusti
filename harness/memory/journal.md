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

### 2026-09-09 — TUI prototype review loop (Lavish)
- Ran `prototype/rustypi-tui.html` through five rounds of browser annotation; every request (whole-line tool colour, /quit, /resume picker, dim reasoning block, per-tool ms, quit-hint gating, /rename) already existed in `src` — confirmed only after `rtk proxy grep`
- Correction received: told the user three features were unbuilt on the strength of a filtered grep that returned nothing. Lesson saved as `harness/memory/rtk-filters-command-output.md`
- Session-start `git status` listed no `src` changes; `git diff --stat HEAD` later showed 14 files / 1015 insertions. Cause unconfirmed — stale snapshot or a concurrent session in the same worktree. Check `git diff --stat HEAD` before trusting the startup snapshot
- `cargo build --release` fails with `Access is denied` on `target/release/rustypi.exe` while a rustypi is running (Windows exe lock); debug builds and `cargo test` are unaffected
- `rtk npx … --agent-reply "multi\nline"` dies with `npx: batch file arguments are invalid`; single-line arguments only

### 2026-09-09 — /model navigation bug
- `/model` with no arg printed the profile list into the transcript, so there was nothing to navigate — a sibling command (/resume) had a picker but /model never got one
- Root cause was a missing feature surface, not a broken key handler: the up/down match arms only fired while a picker or menu was open
- Reused the existing Pick struct rather than a new picker type; added a kind enum (Session|Model) to route Enter, and a top window index so long lists scroll instead of walking off-screen
- Lesson: when two slash commands do the same shape of thing, check whether one already has the UI the other is missing before writing new key handling
