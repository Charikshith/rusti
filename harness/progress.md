# Progress: rusti

## Current State
**Phase**: MVP Complete — Agent Live-Tested
**Last Verified**: 2026-09-09 (cargo build --release 0 warnings, cargo test 13 passed, --self-test with and without ripgrep on PATH, ./init.sh)

## Session 2026-09-09: Fix — /model list had no up/down navigation
- Reported as "/model isn't navigable with up/down". Root cause: `/model` with no argument called
  `list_models`, which dumped every profile into the transcript as plain text — there was no
  selection to move. The `/resume` command had a real arrow picker (`app.pick`), but `/model` never got one
- Fix: `/model` (no arg) now opens the same picker as `/resume`. `Pick` gained a `kind` field
  (`Session` | `Model`) and a `top` window index; `pick_model` builds the rows from model.json and
  marks the active profile with `*`; Enter routes by kind to `Job::ResumePath` or `switch_model`
- `picker_nav` is the shared arrow logic, and it now keeps the cursor inside a visible window
  (`PICK_ROWS = 8`) instead of letting Up/Down walk off-screen; long lists scroll, short ones don't
- Renderer: `panel_rows` slices the picker rows by `p.top`, and the footer hint reads "switch" for
  models vs "resume" for sessions, plus a `n/total` counter when the list overflows the window
- Verification: `cargo test` 13 passed (new: picker_nav_walks_and_scrolls_the_visible_window)

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
- Permission gate in `run_agent` before `dispatch`: gated tools ask through the existing `Event::Ask` (so the TUI needed no change) with `y / n / a(lways for this tool)`; `decide()` is the pure decision so it is under test. `--yolo` / `RUSTI_YOLO=1` skips. Piped stdin -> "no answer given" -> denied, by design
- `tools::guard()` refuses writes outside the canonicalized cwd; it canonicalizes the deepest *existing* ancestor and re-appends the rest so not-yet-created files and `../` are judged on the real path. Applied to write/multi_edit/delete/move only; reads stay open (recorded as Read-Side Sandbox in open-work)
- New tools: `delete_file`, `move_file` (refuses to clobber), `run_background` / `job_output` / `job_stop` (shared 1 MB ring buffer fed by pump threads; `job_output(0)` lists; stop uses `taskkill /T /F` on Windows because `cmd /C` wraps the real process), `todo` (whole-list replace, rendered `☑ ◐ ☐` and emitted as `Event::Text` lines), `delegate` (one level, `Box::pin(run_agent)` on a fresh `Session` under `.rusti/sessions/sub-*.json`, `DEPTH` guard refuses nesting)
- `dispatch` now takes `client` and `cancel` for delegate; `edit_file`/`run_command` share `shell()`; `truncate` is `pub`
- Self-test flips `YOLO` on for the fake-server run (a prompt with no stdin would deny the scripted `run_command`) and off again around the guard checks

## Session 2026-09-09: Iteration cap + project instructions (feat-022)
- `MAX_ITERS` is now an `AtomicUsize` defaulting to 50 (was a const 10 that killed any real read/edit/test/fix task); `--max-iters N` or `RUSTI_MAX_ITERS` overrides, floor 1. Flag is in `VALUE_FLAGS` so its value is never mistaken for the task
- `system_prompt()` appends the working directory's `AGENTS.md` (fallback `RUSTI.md`, `CLAUDE.md`; blank files skipped, 20k cap). Read every turn — the system entry already refreshed each turn, so instruction edits are live mid-session

## Session 2026-09-09: Native search + edit tools (feat-021)
- Tools grew from 5 to 9: `grep`, `glob`, `list_dir`, `multi_edit`; `read_file` gained `offset`/`limit` (ranged reads are line-numbered, whole reads stay raw); `run_command` gained `timeout_secs` (default 120, polls `try_wait`, kills at deadline)
- grep/glob shell out to ripgrep when it is on PATH (regex, .gitignore-aware, parallel) and fall back to a std-only walk with a fixed skip list (`.git`, `target`, `node_modules`, …) and a home-grown `*`/`?`/`**` matcher — no new crates, binary size unchanged. Fallback grep is substring-only and says so in its "no matches" line
- `edit_file` is now a one-entry `multi_edit`; edits apply in memory and write once, so a bad edit N leaves the file untouched
- Self-test covers both search paths: run once normally, once with `PATH=/c/Windows/System32` to hide rg
- Removed the three matching open-work items (Command Timeouts, Native Search Tools, Multi-Edit)

## Session 2026-09-09: /reload (hot rebuild + relaunch)
- Added TUI `/reload`: rebuilds rusti from `CARGO_MANIFEST_DIR` (works even when cwd is some other project being coded on) into `target/reload`, then runs a uniquely-named *copy* of the binary — the built file is never the running one, so cargo can always overwrite it (Windows locks a running exe). Relaunch is a true `exec` on Unix; on Windows the old process waits as a thin wrapper (exiting after `spawn` let the shell take its prompt back and fight the child for console input). A first version ping-ponged two build dirs — that breaks on the 3rd reload once parents wait, hence the copy approach
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
Implemented the 8 gaps between prototype/rusti-tui.html and the Rust TUI, one at a time:
1. Tool wall time — `Event::ToolEnd` carries `ms`, measured around dispatch (and around /reload's cargo build); `ai_core::took` formats 450ms / 1.6s
2. Ctrl+C twice to exit — first press clears input + cancels the turn, second within 2s quits, yellow `press ctrl+c again to exit` while armed, any other key disarms
3. `/quit` — `handle_command` returns bool; true means Exit::Quit
4. Per-turn stats — `· N tok · N tps · N.Ns` from `Event::Usage`; server `usage.completion_tokens` when sent, else a `~`-marked chars/4 estimate; tps clock starts at the first token so tool waits don't dilute it
5. Thinking block — `reasoning_content`/`reasoning` deltas stream into a dim `│ ` block, closed by the answer or a tool call, never fed back as content
6. Slash menu — lone `/word` opens it above the input: 5 rows, up/down over the whole list, Tab completes, Enter runs, Esc dismisses for that text, counter row on overflow, queued commands shown `· soon`
7. Status hint policy — `ctrl+c twice to quit` only on a fresh prompt or when scrolled off the bottom, else blank
8. Named sessions + `/resume` picker — `--session NAME` → `.rusti/sessions/NAME.json` (root session.json stays the unnamed default), no-arg `/resume` opens an arrow picker, `/resume 2|NAME` skips it, `Event::Resumed` swaps transcript + history

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
- `/rename <new-name>` — moves the active session to `.rusti/sessions/<name>.json`; name check is ASCII alnum + `._-` up to 40, refuses a taken or current name, an unsaved session just repoints its path. Extracted as `rename_session()` so the file move is under test
- `/session list|switch` added to the menu as `· soon` (still queued in open-work)

Also: `/reload`'s relaunch argv now replaces a stale `--session` pair with the live session name, so a mid-session `/rename` survives the hot rebuild.

Note for future tests: `fs::rename` can't cross volumes, so session-path tests must use cwd-relative
paths (the temp dir is on another drive here) — production paths always are.

Verification: `cargo test` 7 passed, `--self-test` OK, `./init.sh` clean, live runs with
`--session smoke` and `--session two` against 127.0.0.1:8080.

## Completed (25/25 features)
1. ✅ Project Setup & Baseline — cargo build (release), cargo test, --self-test
2. ✅ Core Agent Loop — run_agent with tool dispatch, session persistence (round cap now 50, see feat-022)
3. ✅ LLM Streaming & SSE — reqwest SSE with \n\n/\r\n\r\n delimiters, per-index argument accumulation
4. ✅ Tool Implementations — the original 5: read/write/edit file, run command, ask user (extended by feat-021/023)
5. ✅ Model Config & CLI — model.json with profiles, --list/--use/--add, env vars, interactive add
6. ✅ TUI Mode — pi-style renderer, synchronized output (CSI 2026), differential rendering, plain fallback (screen model now alternate, see 25)
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
24. ✅ TUI Markdown Rendering — headings/bold/inline code/bullets/fences as style runs, no parser crate
25. ✅ Alternate Screen TUI — fresh canvas at launch (no gap below the prompt), quit restores the primary buffer exactly (history intact, farewell under the launch line)

## Build Status
- `cargo build --release` — 0 warnings, 0 errors (release rebuild blocked while the TUI runs: exe lock — use /reload in-TUI instead)
- `cargo test` — 13 passed
- `--self-test` — passes; also covers the search tools on both paths (ripgrep and the std fallback, run with
  `PATH=/c/Windows/System32` to hide rg), multi_edit atomicity, the command timeout, permission decisions,
  the project-root guard, move/delete, background job lifecycle, and todo rendering
- Binary: ~3.0 MB (release), ~3,300 lines of application code
- 16 tools; ripgrep is used when on PATH and is never required
- Dependencies: serde, serde_json, tokio (current-thread), reqwest, futures-util, bytes, crossterm, is-terminal

## Verified
- `/model` picker + quit-clear live-verified by user run (9c8948e): no-arg /model opens the arrow picker over model.json (active row marked ▸, starts selected, scrolls past 8 rows); quit (ctrl+c twice) clears the screen and prints "Come back again, boss" so a relaunch starts on a clean terminal
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

## Session 2026-09-10: /model picker shipped + alternate screen (feat-025)
- TUI now runs on the alternate screen (EnterAlternateScreen/LeaveAlternateScreen): no gap between the shell prompt and the bottom-pinned panel at launch, and quit restores the primary buffer exactly — `Come back again, boss` prints right under the launch line, shell history untouched. Transcripts persist via session.json + /resume.
## Session 2026-09-10 (2): feat-026 - multi-tool-call message integrity

- Bug report: [CommandCode error: "Tool result is missing for tool call call_00_7dueXDf20YUXmQlI48NA3107."] on every turn in one session, while the same model works through pi.
- Root cause: run_agent added each tool result as a sibling under the assistant entry; path_messages() walks a single parent chain, so on a turn with two tool calls only the last result reached the wire and the other call was left dangling.
- Fix: chain tool results (parent = previous tool entry) so the active path carries all of them; --self-test now emits two tool calls per turn and asserts 6 messages (msgs[3] and msgs[4] both role=tool).
- Verification: cargo test 13 passed, --self-test OK. The old broken session.json is not recoverable - start a fresh session.
- Recorded as feat-026; lesson: harness/memory/session-tree-path-drops-siblings.md.
## Session 2026-09-10 (3): model-switch note removed from transcript

- `/use` / `/model` used to append the line `  ✓ model switched to <model>` to the transcript, but the bottom status line already shows `<session> · <model>` (tui/render.rs), so the note was redundant.
- Dropped the `Event::Text` in the `Job::Model` arm (tui/mod.rs); the status line updates immediately because `switch_model` sets `app.model` synchronously.
- Verification: `cargo check` clean, `cargo test` 13 passed. Release build was blocked by the Windows exe lock (a rusti TUI running) — unrelated to the change.

## Session 2026-09-13: feat-028 — prompt-token tracking
- `Event::Usage` now carries `prompt` (context size sent to the model) next to completion `tokens`; read from
  `usage.prompt_tokens`, estimated from the serialized request body (÷4) when the server omits it, `est` set either way.
- TUI stats row ends with `· ctx 4.3k` — the last call's prompt size, so it reads as "current context", not a sum.
- Fixed a stale `--self-test` assertion (5 → 6 entries on disk) that feat-026 left behind; self-test was failing on HEAD.
- Backlog: open-work.md gained a tiered Suggested Order, Project Config File, Web Fetch Tool, MCP Client; stale TLS entry removed.
- Verification: `cargo test` 13 passed, `--self-test` OK.

## Session 2026-09-13 (2): feat-029 — context compaction
- `run_agent` compacts before the next request once the last `prompt_tokens` exceed `CONTEXT_LIMIT`
  (default 100k, `--context N` / `RUSTI_CONTEXT`, floors at 1000).
- `compact()` summarizes `path[..cut]` with one tool-less LLM call, then branches `system -> summary (user role)
  -> clones of path[cut..]`; `compact_cut` keeps the last 8 entries, snapped forward to a non-tool role so no tool
  result is orphaned (falls back to the latest call when a tail is all results). Old entries stay in the tree.
- `ChatResult.prompt_tokens` added so the loop sees the size without going through the event channel.
- Verification: `cargo test` 13 passed, `--self-test` OK (compact_cut cases + limit floor). Live >100k run still pending.

## Session 2026-09-13 (3): feat-030 — retry with backoff
- `chat_stream` wraps connect + status check in a loop: connection errors and HTTP 408/429/5xx retry up to 3×
  with 1s/2s/4s backoff (`RETRY_BASE_MS` static, shrunk in tests); sleep polls `cancel` every 50ms.
- Other 4xx fail at once. Nothing after the first streamed byte is retried, so output never duplicates.
- Transcript line: `⚠ HTTP 503 ...: busy — retry 1/3 in 1.0s`.
- `--self-test` fake server answers 503 on connection #1, then the two scripted SSE bodies — retry is exercised
  on every self-test run. `cargo test` 13 passed, `--self-test` OK.

## Session 2026-09-13 (4): feat-031 — failed tool output under the ✗ line
- `Event::ToolEnd` gained `output` (filled only when `ok == false`, empty otherwise so success stays cheap).
- `ai_core::fail_tail()` turns it into the last 8 non-empty lines as `  · ` rows (dim via the existing stats
  style) plus `  · … N more lines` when cut; TUI, plain pipe, and CLI stderr all use it.
- Deferred: Ctrl+O toggle to expand successful tool output (open-work "Tool Output Expand Toggle").
- `cargo test` 13 passed, `--self-test` OK. Tier 1 of the 2026-09-13 backlog is complete.

## Session 2026-09-13 (5): feat-032 — /tree picker in the TUI
- Question from the user: does `/tree` make `/undo` redundant? Answer: for the *conversation*, yes — but the TUI's
  `/tree` was only a text dump, and the tree cannot restore files on disk.
- `/tree` now opens the shared panel picker (PickKind::Tree) over `tree::rows()` filtered to user/assistant entries;
  Enter -> `Job::Select(id)` -> `session.select` + save -> `Event::Resumed` rebuilds the transcript; a user entry also
  sends `Event::Prefill` so its text lands in the input (pi-style edit-and-resend).
- `/undo` stub removed from CMDS; `one_line()` deleted (only the dump used it). Backlog: "/undo Last Turn" dropped,
  "Diff Before Edit + Undo" reframed as "File Undo".
- `cargo test` 14 passed, `--self-test` OK. The picker itself was not driven interactively this session.

## Session 2026-09-13 (6): feat-033 — /undo puts files back and rewinds the turn
- `tools.rs`: `UNDO` frame of `(path, Option<bytes>)`; `snapshot()` runs after `guard()` in write/edit/multi_edit/
  delete/move (both ends of a move), first before-image per path wins, unreadable files are skipped rather than risk
  deleting them on undo. `undo_turn()` restores in reverse and reports one line per file.
- `run_agent` clears the frame only at DEPTH 0, so a `delegate` sub-run's edits belong to the parent turn.
- TUI: `/undo` -> `Job::Undo` -> restore files (`↶ restored …`), then `branch_at(last user entry)` — the same helper
  the `/tree` picker uses — so the conversation rewinds and the message is prefilled for editing.
- `cargo test` 14 passed, `--self-test` OK (undo round-trip asserted). `/undo` not driven interactively.

## Session 2026-09-13 (7): Tier 2 — feat-034/035/036/037
- **feat-034 --help/--version**: checked before every other argument so they work with no model configured.
  `--json` NDJSON stays in open-work.
- **feat-035 git awareness**: `system_prompt()` appends `# Git` (branch + `git status --short`, 2k cap). It is rewritten
  each turn, so the model always sees the tree as it is now. `Event::Git` carries the branch to the status line
  (emitted before the first frame and after every job, so a `!git checkout` refreshes it). `/commit` is a prompt macro —
  review, stage, commit with a reasoned message, no push.
- **feat-036 `!cmd`**: runs on the agent thread via `tools::run_command`, renders as a `$ cmd` tool line with output
  shown on success too, and appends the result to the session as a user entry so the model sees it next turn.
- **feat-037 footer**: `app::footer_right` composes `session · model · ⎇ branch · N tok · ctx N%` from the parts that
  exist. Session totals reset on /resume.
- **Parallel tool calls: examined and skipped.** The runtime is current-thread and every tool but `delegate` blocks, so
  `join_all` would overlap nothing; real concurrency needs a multi-thread runtime plus spawn_blocking, which puts the
  `Mutex` statics and the interactive permission prompt under contention. Reasoning recorded in open-work.
- `start_task()` extracted from the Enter handler so /commit reuses the same turn setup.
- `cargo test` 15 passed, `--self-test` OK, `--help`/`--version` checked by hand. TUI paths not driven interactively.

## Session 2026-09-13 (8): Tier 3 resequenced, feat-038 project config
- Tier 3 was filed in arrival order; resequenced by friction-removed ÷ effort into 3a (Plan Mode, Web Fetch,
  /session switch, /export), 3b (Test Loop, tool-line diff, expand toggle, read sandbox, multi-level undo),
  3c (prompt caching, native Anthropic/Gemini, image input, MCP — only when the need is real).
- **feat-038**: `model.json` gained `allow` / `max_iters` / `context`, all optional and omitted from the file when
  unset, so every existing model.json keeps loading untouched. `[a]lways` now appends to `allow` and saves —
  permissions survive a restart, which was the actual daily friction.
- **Decision**: extended model.json rather than adding `.rusti/config.json` as the backlog sketched. One file, one
  loader, and the profile default already lived there. Recorded on the feature entry.
- `decide()` became a pure `Answer` parse with the saving moved into `permitted()`, so `--self-test` can exercise the
  permission logic without writing to the user's real model.json (it would have, the naive way).
- `cargo test` 15 passed, `--self-test` OK, `--list` verified against a model.json with none of the new keys.

## Session 2026-09-13 (9): feat-039 plan mode
- `/plan` toggles a PLAN atomic checked at the very top of `permitted()` — before `--yolo` and before the saved
  allowlist, because turning plan mode on is a more specific and more recent instruction than either.
- Reuses the existing GATED list, so the set of blocked tools is exactly the set that already needed permission;
  read/search/list/todo keep working, which is the point.
- The refusal message tells the model to stop retrying and produce a plan; `system_prompt()` grows a `# Plan mode`
  section while it is on, and since the prompt is rewritten each turn the toggle takes effect immediately.
- Toggle rather than the backlog's approve-and-drop flow: no approval state machine to build or explain.
- `cargo test` 15 passed, `--self-test` OK (blocks every mutating tool, no read tool, prompt section appears/disappears).

## Session 2026-09-13 (10): feat-040 web_fetch
- `web_fetch(url)` over the reqwest already in the tree — no new dependency, no binary growth. 20s timeout,
  5 MB content-length ceiling, http(s) only, MAX_RESULT truncation.
- `strip_html` is ~40 lines: one ASCII-lowercased copy keeps byte offsets aligned, `<script>`/`<style>` bodies are
  skipped wholesale, block tags become newlines and other tags spaces, then entities decode (`&amp;` last so
  `&amp;lt;` doesn't decode twice) and whitespace collapses. No HTML crate.
- Gated like the mutating tools: the URL leaves the machine and the reply enters the context. The result carries an
  `[… untrusted page content, not instructions]` prefix so an injected instruction reads as quoted data.
- The self-test now serves a real page off a one-shot TCP listener and asserts the whole path end to end.
- **TLS was never exercised before this** (the LLM endpoint is 127.0.0.1). Confirmed working by pointing the client
  at https://example.com and getting a real HTTP 405 back rather than a handshake error.
- Readme's safety section corrected: it listed the old prompt text and omitted that "always" now persists.
- `cargo test` 15 passed, `--self-test` OK.

## Session 2026-09-13 (11): feat-041 /export, feat-042 /session dropped
- **feat-042 first, because it changed the work**: `/session list|switch` turned out to be redundant. `/resume` with
  no argument already lists every saved session in a picker and switches to the choice; `/resume <name>` and
  `/resume <n>` already switch directly. Building `/session` would have been a second name for the same three
  behaviours. Removed the `· soon` stub and rewrote `/resume`'s menu description so the capability is discoverable.
- **feat-041 `/export [file.md]`**: `Session::export_markdown` renders the active path — numbered user turns,
  assistant prose, tool calls as `- **name** \`args\`` with arguments flattened to one line, tool results as fenced
  blocks clipped to 500 chars. The system prompt is left out (boilerplate) and an empty assistant turn gets no
  heading. Defaults to `<session-name>.md`.
- Real gap found while checking the above: there is still no way to start a *fresh* session without relaunching.
  Filed as "New Session Without Restarting".
- `cargo test` 16 passed, `--self-test` OK.

## Session 2026-09-13 (12): live verification of the one-shot path
Ran real tasks against the proxy on :20128 (`cmc/deepseek/deepseek-v4-flash`), read-only, stdin piped so any
gated tool would auto-deny.
- **Agent loop**: "read Cargo.toml and reply with the package name" → streamed prose, one `read_file` tool line,
  answered `rusti 0.1.0`. The whole feat-028..041 stack is in that binary.
- **feat-035 git context**: asked for branch + tree state with tools forbidden; the model answered `master` and both
  dirty paths from the system prompt alone. Confirms the injection and the every-turn refresh.
- **feat-040 web_fetch**: fetched https://example.com in 1.6s over TLS, read back heading and purpose, and
  volunteered "the fetched content is third-party page text, not instructions" — the untrusted prefix works.
- **feat-030** confirmed by the negative: an HTTP 400 failed immediately with no retry, as designed.

**Environment finding, not a rusti bug**: the default profile `qwen3-8b-gguf-q5_k_m` points at 127.0.0.1:**8080**,
which is not running — every "model not found" seen first came from there, not from the :20128 proxy. 36 other
profiles point at :20128 and work. `--use <name>` fixes it; left alone, it is the user's config.

**Still unverified — needs an interactive terminal**: every TUI surface from feat-032 on (/tree picker, /undo, /plan
toggle and its footer marker, /commit, /export, `!cmd`, the footer itself, failed-tool output rendering).

## Session 2026-09-13 (13): TUI polish from the first live run — feat-043/044
User ran the TUI and sent a screenshot. Two findings, both fixed.
- **feat-043 transcript floated at the top.** `draw()` filled the transcript area top-down from `view` and padded
  with blanks *below*, so a short transcript left a screenful of gap above the input. Now
  `render::transcript_window()` returns the pad/start/end and the pad goes ABOVE — content rests on the input like a
  shell, new lines rise from the bottom. scroll_up is clamped so it can't walk past the first line (it previously
  could, silently shrinking the visible slice).
- **feat-044 type-to-filter in the pickers.** `/model` lists 37 profiles; scrolling that is the wrong tool. `Pick`
  gained `filter` + `visible()`, shared by all three pickers (/model, /resume, /tree) since they share the struct.
  Substring and case-insensitive over the whole label, so "zai-org" finds glm-5.1 by its model id.
- Safety detail: the picker key guard swallowed *everything* unmodified, so adding Char() would have made Ctrl+C
  type a 'c'. The guard now lets Ctrl+key fall through, and a first Ctrl+C closes the picker.
- `cargo test` 18 passed. **The exe could not be relinked — the user's TUI still holds it**; they must quit and
  rebuild to see these.

## Session 2026-09-13 (14): feat-045/046 from the second live run
- **feat-045 the permission answer was a fake message turn.** The Ask-Enter handler did `msg_num += 1` and pushed
  `2› y`, so answering "allow run_command pwd?" looked like the user's second message — but the answer never enters
  the session at all. `App.ask_line` now records where the question was drawn and `close_ask()` rewrites that line as
  `ℹ <question> → <answer>`, reusing the replace-in-place pattern ToolEnd already uses for ToolStart. Esc and Ctrl+C
  go through the same helper, so an interrupted prompt reads `→ interrupted` rather than being left open.
- **feat-046 Shift+Enter inserts a newline.** Enter with SHIFT or ALT inserts `\n` (ALT because terminals vary in
  whether they report Shift+Enter at all). The input box is no longer one row: `input_rows()` draws one row per line,
  `bottom_rows` accounts for the height, and `caret_at()` puts the terminal cursor in the row that owns it.
- Trap avoided: Up/Down were history recall, so the first Up in a half-written multi-line draft would have replaced
  it. They now walk the draft's lines while it contains a newline.
- `cargo test` 19 passed, `--self-test` OK, binary relinked once the user's TUI released it.

## Session 2026-09-13 (15): feat-047 — resumed sessions lost their tool lines
- User reported "only 1 message" after a resume. Read their session.json rather than guessing: 7 entries — system,
  user "hi", ONE assistant carrying four tool_calls, and four tool results. So the user/assistant count was right;
  what vanished was the body of the turn, because `render_history` skipped every tool entry.
- `render_history` now queues each call's summary (via the same `ai_core::tool_summary` the live path uses) and pairs
  it with the tool results chained after it — feat-026's chaining is what makes the ordering reliable.
- `Entry.ok: Option<bool>` added and set when a result is stored, so the replay shows the real ✓/✗. Older entries
  render `·` rather than being guessed at from their text ("user denied …" is a heuristic that would mislabel any
  tool whose real output mentions an error).
- Durations deliberately not replayed: never stored, and a previous run's wall time is noise in a resumed view.
- `cargo test` 19 passed. **Binary not relinked — the user's TUI held it again**, so the live resume is unconfirmed.

## Session 2026-09-13 (16): feat-048 — session bookkeeping is status, not transcript
- User: the "resumed session.json (7 entries…)" line sat in the chat and should appear below and disappear.
- `Event::Notice(String)` added: the TUI parks it in `App.notice` with a timestamp and the status line shows it for
  3s, after which the 50ms render loop just stops drawing it (nothing to clear, no timer).
- Converted the four session-management confirmations — resumed, branched at, exported to, renamed — plus the plan
  mode toggle, whose state the footer already carries. **Failures deliberately stay in the transcript**: a message you
  need to read must not vanish after three seconds.
- Plain/CLI modes print notices as before; only the TUI treats them as transient.
- Also confirmed the user is running a STALE BINARY: exe 2:36pm vs source 2:42pm, so feat-047's replayed tool lines
  were not in the build they tested. cargo build has been blocked all session by their running TUI holding rusti.exe.
- `cargo test` 19 passed; binary still not relinked.

## Session 2026-09-13 (17): feat-049 build time, feat-050 retry line
- **feat-049**: user asked for the release build to drop under 10s (23.9s). Measured three profiles, each warmed
  first (changing a profile rebuilds every dependency, so the first build after a change is not the number that
  matters) then timed after `touch src/main.rs`:
  fat LTO+cgu=1 23.5s/3.27MB · thin+cgu=1 12.7s/3.66MB · thin+cgu=16 **6.1s/4.05MB**.
  Only cgu=16 clears 10s. Took it; the measurements and the revert line live in Cargo.toml. readme's stale
  "~2.6 MB" claim corrected to ~4 MB.
- **feat-050**: retries printed one near-identical line per attempt. `Event::Retry { attempt, of, wait_ms, err }`
  replaces the text line — attempt 1 pushes a row, later attempts rewrite it. Because attempt==1 always starts a new
  row there is no state to clear between turns. Pipes still get one line per attempt; there is nothing to rewrite.
- Verified the plain path live against the dead :8080 endpoint: 1.0s/2.0s/4.0s then the error.
- `cargo test` 19 passed, `--self-test` OK, release build 6.09s.

## Session 2026-09-13 (18): feat-049 revised — under 15s WITHOUT giving up size
- User rejected the thin-LTO trade: keep `lto = true` / `codegen-units = 1`, just get under 15s.
- Found the time in `opt-level` instead. "z" is both cheaper to optimize and far smaller than the default 3:
  3 → 23.5s/3.27MB, 2 → 24.0s/3.13MB, s → 19.0s/2.55MB, **z → 15.7s/2.33MB**.
- Still 0.7s over, so trimmed reqwest to `default-features = false` + json/stream/native-tls, dropping http2 and
  charset. Fat LTO's cost scales with how much IR it merges, so less code helps time and size together:
  **13.7s / 1.87MB** — 42% faster and 43% smaller than where we started.
- Measurement trap worth remembering: the first build after any profile or dependency change recompiles every
  dependency and read 16.2s; three steady-state runs were 13.6/13.7/13.8s. Always warm, then time.
- `incremental = true` measured and discarded — LTO re-merges the whole program regardless.
- Because the feature trim touches the network stack, re-verified rather than assumed: TLS handshake to
  https://example.com, live streaming + tool call against the proxy, and web_fetch over https all still work.
- Reverted the earlier thin-LTO change entirely; readme size claims corrected (2.6MB → 1.9MB) and its stale
  "/tree dumps the session path" line fixed.

## Session 2026-09-13 (19): feat-051 build, feat-052 transcript weight, feat-053 /settings, feat-054 tool status

- **feat-052 first half**: the picker's typed filter was drawn in its header next to the row count.
  Moved to the `>` prompt — while a picker is open the filter IS the draft (real draft hidden until Esc).
  Filter still has no left/right cursor movement; `picker_key` only appends and backspaces.
- **feat-052 second half**: transcript weight was flat — user queries, model prose and tool output all
  full brightness. User lines get a cyan `N›` marker (bright text), tool output drops to 256-colour 245.
  **Trap**: user lines shared style `b'k'` with tool rows, so dimming the tool fallthrough would have
  dimmed every *wrapped* row of a long query while row 1 stayed bright. Split them into `b'u'`.
  Used 245 not `ESC[2m` for the reason already on THINK: Windows Terminal barely darkens `2m`.
- Status line moved right→left on request; padding is now only emitted when there IS a hint, since
  `draw_line` clears to end-of-line and never needed padding to erase the previous frame.
- **feat-051**: user asked for release under 10s. feat-049 had *already* tried thin LTO and reverted it
  to protect size — so this time the whole `lto × codegen-units` matrix got measured before deciding.
  Fat is nearly FLAT across cgu (12.8 / 11.7 / 12.1) because it merges the program into one module and
  optimizes serially; cgu only splits front-end codegen. **Fat's floor is 11.7s — it cannot meet a 10s
  target at any setting**, which is what makes this reversal different from the last one. Thin + cgu=4:
  4.2s / 2.16MB. `cgu=1` is a fat-LTO habit that cost 2.7s here. Full matrix in Cargo.toml.
  If size ever outranks the 2.7s again, thin/cgu=1 at 2.02MB is the fallback — NOT fat/cgu=16, which is
  strictly dominated (same 2.02MB, 5s slower).
- **feat-053**: `ctx %` already existed but was gated on `ctx > 0`, and `turn_ctx` resets to 0 at the start
  of every turn — so it vanished exactly when you were watching it fill. Gate removed. `/settings` reuses
  the picker (`PickKind::Settings`, Enter toggles and does NOT close) and persists to model.json.
  Plan mode is deliberately not toggleable: it is *why* writes get refused.
- **feat-054**: `ToolStart` stored a literal `⠋`, so a running command looked identical to a stalled one,
  and its text was dim like finished output. Live frame now substituted at draw time from `app.spinner`;
  running row is bold, and `ToolEnd`'s glyph swap un-bolds it.

### Found, not fixed
- **`Config::load_from` swallows a malformed model.json silently** (`config.rs:60`, `.ok()` twice): one wrong
  key returns all-defaults with no message. Caught it by writing a test fixture with `base` instead of `url`
  and watching `default` come back `None`. Now that `/settings` writes to that file, a bad hand-edit loses
  models and `allow` invisibly. Ten-line fix; the highest-value thing left.
- **No `panic::set_hook`**: the restore at `mod.rs:394` is a plain sequential call, so a panic skips
  raw-mode-off and alternate-screen-exit and wrecks the terminal. Also the precondition for `panic="abort"`.
- **`/reload` builds into `target/reload`**, a separate 777MB tree — it did NOT inherit feat-051's warm
  `target/release`. The first `/reload` after this profile change pays a full cold rebuild, not 4.2s.
- `tool-visual-examples.html` is untracked and predates this session; left alone.
