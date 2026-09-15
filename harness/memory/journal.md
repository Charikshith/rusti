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
- Ran `prototype/rusti-tui.html` through five rounds of browser annotation; every request (whole-line tool colour, /quit, /resume picker, dim reasoning block, per-tool ms, quit-hint gating, /rename) already existed in `src` — confirmed only after `rtk proxy grep`
- Correction received: told the user three features were unbuilt on the strength of a filtered grep that returned nothing. Lesson saved as `harness/memory/rtk-filters-command-output.md`
- Session-start `git status` listed no `src` changes; `git diff --stat HEAD` later showed 14 files / 1015 insertions. Cause unconfirmed — stale snapshot or a concurrent session in the same worktree. Check `git diff --stat HEAD` before trusting the startup snapshot
- `cargo build --release` fails with `Access is denied` on `target/release/rusti.exe` while a rusti is running (Windows exe lock); debug builds and `cargo test` are unaffected
- `rtk npx … --agent-reply "multi\nline"` dies with `npx: batch file arguments are invalid`; single-line arguments only

### 2026-09-09 — /model navigation bug
- `/model` with no arg printed the profile list into the transcript, so there was nothing to navigate — a sibling command (/resume) had a picker but /model never got one
- Root cause was a missing feature surface, not a broken key handler: the up/down match arms only fired while a picker or menu was open
- Reused the existing Pick struct rather than a new picker type; added a kind enum (Session|Model) to route Enter, and a top window index so long lists scroll instead of walking off-screen
- Lesson: when two slash commands do the same shape of thing, check whether one already has the UI the other is missing before writing new key handling

## 2026-09-10 — /model picker + quit farewell
- User saw the OLD plain-text /model list twice because they ran target/release/rusti.exe while the picker was only in the uncommitted working tree — committed code ≠ running binary is the trap here.
- Windows exe lock: running TUI blocks cargo from overwriting rusti.exe; the in-TUI /reload (target/reload + staged copy) is the designed path, but closing the TUI + plain rebuild is simpler when possible.
- Leftover transcript on relaunch: main-screen TUI (no alternate buffer, by design) + no clear-on-quit. Fixed with terminal::Clear(All) + farewell line on Exit::Quit.
- No user corrections this session; the "why do I see the old screen" confusion was binary staleness, not render state.

## 2026-09-10 (2) — quit-screen saga: main-screen design was the root cause
- Three attempts at the quit artifact: ESC[2J alone (gap remained, farewell floated at bottom), +ESC[3J (gap gone but ate the shell history), 2J+home (gap remained between launch line and panel — the panel pins to the terminal bottom by design).
- Root cause was never the clear sequence: a main-screen TUI with a bottom-pinned panel always leaves a hole between the shell's last line and the input field. Alternate screen is the only fix that preserves the primary buffer exactly.
- Switched to EnterAlternateScreen/LeaveAlternateScreen; quit now needs no clearing at all, farewell prints right under the launch line.
- Lesson: when a cosmetic terminal fix needs escalating escape-sequence hacks, question the screen-buffer model instead of tuning the sequences.
## 2026-09-10 (3) - CommandCode "Tool result is missing"
- Reported as a provider problem; it was a rusti bug. Traced by walking the active parent chain in session.json: m3 (2 tool calls) -> m5 only, m4 orphaned.
- Had to look up: session.add with the same parent creates a fan-out, and path() follows one chain - the tree looked whole but the request was not.
- Surprise: the error repeated on every later turn because the broken prefix stayed on the active path; no retry could clear it.
- Would do differently: when a provider rejects on message shape, dump the actual request (path_messages output) before blaming the provider.
## 2026-09-10 (4) - drop the redundant model-switch note
- User asked why `✓ model switched to ...` lingers in the transcript when the status line already shows the model. It was a one-line Event::Text in the Job::Model arm - deleted.
- Reminder: the status line source is app.model (set synchronously in switch_model), not the session; the async Job::Model only swaps the client/session model.
## 2026-09-15 (1) - the image was dropped twice, in two different programs
- Symptom identical in rusti, pi and curl ("I can't see the image"), so the instinct was one cause. There were two: 9router's openaiToCommandCode transform swapped image parts for the text `[image omitted]`, and pi never sent the image at all because its model catalog entry declared no image input.
- What settled it: logging the raw user content the proxy transform received. pi's request carried `(image omitted: model does not support images)` as plain text — proof the client, not the proxy, had dropped it.
- The model's explanation ("this model has no image support") read as self-knowledge and was in fact it paraphrasing the placeholder string it had been handed. Never take a model's account of its own capabilities as evidence about the wire.
- Test-target lesson: the first `_bands.png` run "passed" because the agent inflated the PNG with node and read the pixels. A vision test has to forbid decoding, or it tests the wrong thing.
