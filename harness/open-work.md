# Open Work

## Suggested Order (2026-09-13 review)

Tier 1 — done (feat-028/029/030/031: prompt tokens, compaction, retry, failure output)
Tier 2 — done (feat-032/033/034/035/036/037: /tree picker, /undo, --help/--version, git + /commit, ! bash mode, footer). Parallel Tool Calls examined and deliberately skipped — see its entry below.
Tier 3 — resequenced 2026-09-13 by friction-removed ÷ effort, not by the order they were filed:
  3a — done (feat-038/039/040/041: project config, plan mode, web fetch, /export; /session dropped as redundant
     with /resume, see feat-042)
  3b: done or retired. Tool Output Expand Toggle -> feat-059, Diff in the Tool Line -> feat-060, Multi-Level Undo
     and Test Loop rejected 2026-09-14 (verdicts and recheck conditions in harness/memory/graveyard.md).
     Read-Side Sandbox is all that is left, and it only matters if rusti runs somewhere untrusted.
  3c (only when the need is real): Prompt Caching (provider must support it), Native Anthropic/Gemini (only off an
     OpenAI-compatible endpoint), Image Input (changes Entry.content to parts), MCP Client (largest, ecosystem reach)

## Priority — Add When Needed

### Compaction Follow-ups
When: feat-029 landed with a global --context limit and a summary that streams into the transcript
What: per-profile `context` field in model.json so the limit follows the model; `/compact` slash command for manual trigger; collapse the streamed summary into one dim block; retry-on-HTTP-400-context-error as a reactive trigger
Where: config.rs profile, tui/app.rs handle_command, ai_core/mod.rs compact()

### unicode-width Word Wrap
When: CJK or emoji output garbles the TUI
What: Use unicode-width crate for character display width
Where: tui/mod.rs word_wrap()

## TUI Experience Gaps (from pi comparison, 2026-04-10 session)

### Editor Autocomplete
When: Typing speed matters; model/file names not discoverable (slash commands done)
What: Fuzzy autocomplete in input for saved model names and file paths
Where: tui/app.rs input handling (fd/fuzzy lib or lazy prefix match)

### Slash-Menu Follow-ups (prototype parity landed 2026-09-09)
When: The menu exists (`/` opens it) but only completes command names
What: Extend the same panel to saved model names and file paths; the commands
it still lists as `· soon` (/plan /test /export /session) are the items below
Where: tui/app.rs filter_cmds/menu_items, tui/render.rs panel_rows

### Bash Mode Follow-ups
When: feat-036 landed `!cmd` (always with context — the output becomes a user entry)
What: pi's `!!` variant that runs without adding to the conversation; stream output live instead of after exit
Where: tui/app.rs Enter handler, tui/mod.rs Job::Bash

### In-Place Model Switching
When: `/use` requires typing profile names from memory
What: Ctrl+P cycle next / Ctrl+Shift+P previous / Ctrl+L picker, model change without leaving input
Where: tui/app.rs key handlers + tui/mod.rs Job::Model (already exists)

### Cost in the Footer
When: feat-037 shows session tokens and context %, but not money
What: price-per-token on the model profile, multiplied into a session cost figure; needs prompt/completion split kept separately
Where: config.rs profile, tui/app.rs footer_right

## Agent Capability (from 2026-09-09 review) — ★ = start here

### Retry Follow-ups
When: feat-030 retries connect/status failures only
What: honor `Retry-After` on 429; retry a `stream error` that arrives before the first token (today it fails the turn)
Where: ai_core/llm.rs chat_stream

### Parallel Tool Calls — examined 2026-09-13, deliberately not done
When: Model emits several independent tool calls in one turn; they run one by one
Why not: the agent runs on `new_current_thread`, and every tool but `delegate` is blocking (fs, Command::output),
so `join_all` would not overlap anything. Real concurrency needs a multi-thread runtime + spawn_blocking, which puts
the shared `Mutex` statics (UNDO, ALLOWED, JOBS) and the interactive permission prompt under contention, and the TUI's
single `tool_line` would need per-call tracking. The win is milliseconds on fs calls; two long commands are already
served by run_background/job_output.
Revisit if: profiling shows tool time actually dominates a turn.

### Clippy `read amount is not handled` (pre-existing)
When: noticed 2026-09-14 while verifying feat-058; `cargo clippy` fails on it, so clippy can't be a gate until it goes
What: `s.read(&mut buf).unwrap()` ignores the returned count, so a short read silently yields a truncated buffer
Where: ai_core/mod.rs:566 (the self-test's fake server)

## Safety

### Read-Side Sandbox
When: Model reads/lists/greps paths outside the project (../, absolute, ~) — writes are already guarded
What: Extend `tools::guard` to read_file/list_dir/grep/glob behind an `--allow-outside` flag; today reads are deliberately open so the model can look at dependency sources
Where: ai_core/tools.rs guard()

### Project Config Follow-ups
When: feat-038 put `allow` / `max_iters` / `context` in model.json (not the separate `.rusti/config.json` first
sketched — one file and one loader beat two, and the profile default already lived there)
What: a way to see and forget saved permissions without hand-editing JSON (`/allow` listing them, `--forget NAME`);
per-profile `context` so the limit follows the model rather than the project
Where: config.rs, tui/app.rs handle_command

## Developer Workflow

### Git Follow-ups
When: feat-035 injects branch + `git status --short` and adds /commit
What: `git diff` of the current turn's files in the prompt when the status is small; `/commit --amend`; skip the git
calls entirely when `.git` is absent (today two processes spawn per turn either way)
Where: ai_core/mod.rs git_context()

### Plan Mode Follow-ups
When: feat-039 landed `/plan` as a toggle in the TUI only
What: a `--plan` flag so one-shot CLI runs can plan too; auto-drop the mode when the user replies "go"/"do it"
(needs an approval state machine — only worth it if the toggle proves annoying)
Where: main.rs, tui/app.rs handle_command

### --json NDJSON Output
When: Scripting/CI use of the one-shot CLI (--help/--version landed as feat-034)
What: `--json` streams the Event enum as NDJSON on the plain (piped) path instead of prose
Where: main.rs, tui/plain.rs

## Model / API

### Prompt Caching Headers
When: Long system prompt + history re-sent every turn on providers that support caching
What: Send provider cache-control markers on the system/early messages when the profile opts in
Where: ai_core/llm.rs request body, config.rs profile field

### Image Input — follow-ups
When: feat-061 (read_file attaches an image) and feat-062 (ctrl+v / alt+v paste one) are in; what is missing
is a provider to prove the last hop, since the :20128 proxy strips image parts
What: a vision-capable endpoint to verify against; optionally attach straight to the user entry instead of
via a read_file round-trip, which needs the [Image #N] placeholder + side table Claude Code uses
Where: ai_core/tools.rs clipboard_image, tui/app.rs Enter handler

### Native Anthropic / Gemini APIs
When: Endpoint isn't OpenAI-compatible
What: Provider enum on the profile; per-provider request/SSE mapping behind the existing ChatResult
Where: ai_core/llm.rs, config.rs

### Streaming Tool-Call Args Display
When: A long write_file looks frozen while arguments stream
What: Show the tool name as soon as it's known and a byte counter while raw_arguments accumulate
Where: ai_core/llm.rs handle_event emit, tui/app.rs ToolStart

## Tools / Ecosystem

### Web Fetch Follow-ups
When: feat-040 fetches and strips a page; it has no idea about redirects-to-elsewhere, robots, or search
What: block private/loopback addresses behind a flag if rusti is ever run somewhere untrusted (today the agent can
already `run_command curl`, so it changes nothing); a `web_search` tool needs an API key and is a bigger call
Where: ai_core/tools.rs web_fetch

### MCP Client
When: Users want tools rusti doesn't ship (databases, browsers, issue trackers)
What: Connect to stdio MCP servers listed in the project config, merge their tool schemas into the tool list, proxy calls; heavy — defer until Tier 1/2 land
Where: new ai_core/mcp.rs, config.rs, ai_core/mod.rs tool registry

## TUI Experience Low Priority (defer indefinitely)
- External editor (Ctrl+G), clipboard copy (Ctrl+X), paste image (Alt+V)
- Startup help header, changelog, retry/compaction indicators, taskbar progress

### Boxed Full-Screen Pickers
When: The /resume picker renders as a panel above the input (menu-styled rows),
not the boxed full-screen overlay the prototype drew
What: If the panel proves cramped for long session lists, lift tree.rs's box
renderer into a shared overlay used by /resume and an in-TUI /tree
Where: tui/render.rs panel_rows, src/tree.rs
