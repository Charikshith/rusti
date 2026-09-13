# Open Work

## Suggested Order (2026-09-13 review)

Tier 1 — done (feat-028/029/030/031: prompt tokens, compaction, retry, failure output)
Tier 2 — daily-driver quality: Git Integration, CLI Polish (--help/--version), Bash Mode, Parallel Tool Calls, Context/Token Footer (/tree picker and /undo landed as feat-032/033)
Tier 3 — reach: Plan Mode, Native Anthropic/Gemini, Image Input, Prompt Caching, /export, /session switch, Project Config, Web Fetch, MCP Client

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

### Tool Output Expand Toggle
When: feat-031 shows the tail of *failed* tool output only; a successful `cargo test` or `grep` still hides its result
What: Ctrl+O toggles showing the tail for every tool line (carry `output` on success too, render on demand); the data is already in the session entries
Where: tui/app.rs ToolEnd handler + key handler, ai_core/mod.rs ToolEnd emit

### Editor Autocomplete
When: Typing speed matters; model/file names not discoverable (slash commands done)
What: Fuzzy autocomplete in input for saved model names and file paths
Where: tui/app.rs input handling (fd/fuzzy lib or lazy prefix match)

### Slash-Menu Follow-ups (prototype parity landed 2026-09-09)
When: The menu exists (`/` opens it) but only completes command names
What: Extend the same panel to saved model names and file paths; the queued
commands it lists (/undo /commit /plan /test /export) are the items below
Where: tui/app.rs filter_cmds/menu_items, tui/render.rs panel_rows

### Bash Mode (`!` prefix)
When: User wants to run own shell command and see output inline
What: `!` prefix runs command, streams output into transcript (pi: `!` = bash with context, `!!` = without)
Where: tui/app.rs Enter handler

### In-Place Model Switching
When: `/use` requires typing profile names from memory
What: Ctrl+P cycle next / Ctrl+Shift+P previous / Ctrl+L picker, model change without leaving input
Where: tui/app.rs key handlers + tui/mod.rs Job::Model (already exists)

### Context/Token Footer
When: User wants to see cost/token burn and context %
What: Footer with ↑/↓ token counts, cache R/W, context % of window, git branch, session name
Where: tui/render.rs status line

### Follow-Up Queue
When: User wants to type next message while agent streams
What: Alt+Enter queues message, Alt+Up edits queued; tasks already serialize in job channel
Where: tui/app.rs Enter handler

## Agent Capability (from 2026-09-09 review) — ★ = start here

### Retry Follow-ups
When: feat-030 retries connect/status failures only
What: honor `Retry-After` on 429; retry a `stream error` that arrives before the first token (today it fails the turn)
Where: ai_core/llm.rs chat_stream

### Parallel Tool Calls
When: Model emits several independent tool calls in one turn; they run one by one
What: Execute the batch concurrently (tokio::join_all or spawn_blocking for sync tools); preserve result order by tool_call_id
Where: ai_core/mod.rs run_agent tool loop

## Safety

### Read-Side Sandbox
When: Model reads/lists/greps paths outside the project (../, absolute, ~) — writes are already guarded
What: Extend `tools::guard` to read_file/list_dir/grep/glob behind an `--allow-outside` flag; today reads are deliberately open so the model can look at dependency sources
Where: ai_core/tools.rs guard()

### Project Config File
When: Permission "always" answers live only in process memory (ALLOWED in ai_core/mod.rs) and vanish on exit; default model / max-iters must be re-passed per run
What: `.rusti/config.json` with a persistent tool allowlist, default profile, and max_iters; loaded at startup, written when the user answers `always`
Where: config.rs, ai_core/mod.rs permission gate, main.rs

### Diff in the Tool Line
When: feat-033 `/undo` can revert an edit, but you still can't *see* what edit_file/write_file changed before deciding
What: Render a short unified diff (or ±line counts) under the ✓ line for edit/write/multi_edit, reusing fail_tail's dim rows
Where: ai_core/tools.rs (return the diff in the result string), tui/app.rs ToolEnd

### Multi-Level Undo
When: `/undo` only covers the last turn (UNDO is a single frame)
What: Stack of per-turn frames; `/undo` pops one, `/undo N` pops N
Where: ai_core/tools.rs UNDO

## Developer Workflow

### Git Integration
When: Agent has no idea what's already changed; commits are manual
What: `git status --short` + branch injected at turn start (cheap), `/commit` that drafts a message from the session path, branch name in footer
Where: ai_core/mod.rs run_agent (context), tui/app.rs command, tui/render.rs status

### Plan Mode
When: User wants to approve steps before files are touched
What: `/plan` toggles a mode where write/edit/run are disabled (tool error "plan mode"); model proposes steps, user approves, mode drops
Where: ai_core/mod.rs dispatch gate, tui/app.rs command + status marker

### Test Loop
When: "Run tests until green" needs babysitting
What: `/test <cmd>` runs cmd, feeds failure output back as the next user turn, repeats until exit 0 or N attempts
Where: tui/app.rs command → Job::Task loop, or a run_agent wrapper

### CLI Polish: --version, --help, JSON output
When: Scripting/CI use of the one-shot CLI; discoverability of flags
What: `--version` from CARGO_PKG_VERSION, `--help` listing flags/slash commands, `--json` streaming NDJSON events for the plain (piped) path
Where: main.rs, tui/plain.rs

### session.json in .gitignore
When: Every run shows session.json as modified (it's runtime state like model.json)
What: Add it to .gitignore; `git rm --cached session.json`
Where: .gitignore

## Sessions

### /session list|switch
When: `--session NAME`, `/rename` and the `/resume` picker exist; switching
still needs a restart or a trip through /resume (menu lists it as `· soon`)
What: `/session list` (names + ages, same rows as the picker) and
`/session switch NAME`, reusing session::list() and Job::ResumePath
Where: tui/app.rs handle_command

### Export Transcript
When: Sharing a session or filing an issue from it
What: `/export [file.md]` writes the active path as markdown (user/assistant/tool blocks)
Where: tui/app.rs command, session.rs path()

## Model / API

### Cost & Token Tracking
When: Per-turn stats land (`· tok · tps · s`) but nothing accumulates across a
session, and there's no cost or context-% figure
What: Accumulate Event::Usage into the session, add price-per-token to the
profile, show session totals + context % in the footer
Where: session.rs, config.rs profile, tui/render.rs (Event::Usage exists)

### Prompt Caching Headers
When: Long system prompt + history re-sent every turn on providers that support caching
What: Send provider cache-control markers on the system/early messages when the profile opts in
Where: ai_core/llm.rs request body, config.rs profile field

### Image Input
When: Pasting a screenshot of an error / UI
What: `/image <path>` or Alt+V attaches a base64 image part to the next user message (OpenAI content-parts format)
Where: session.rs Entry content (String → parts), ai_core/llm.rs, tui/app.rs

### Native Anthropic / Gemini APIs
When: Endpoint isn't OpenAI-compatible
What: Provider enum on the profile; per-provider request/SSE mapping behind the existing ChatResult
Where: ai_core/llm.rs, config.rs

### Streaming Tool-Call Args Display
When: A long write_file looks frozen while arguments stream
What: Show the tool name as soon as it's known and a byte counter while raw_arguments accumulate
Where: ai_core/llm.rs handle_event emit, tui/app.rs ToolStart

## Tools / Ecosystem

### Web Fetch Tool
When: Model needs to read docs, a changelog, or an error page it was given a URL for
What: `web_fetch(url)` via the existing reqwest client, HTML stripped to text, capped at MAX_RESULT; skip if staying small matters more
Where: ai_core/tools.rs, dispatch in ai_core/mod.rs

### MCP Client
When: Users want tools rusti doesn't ship (databases, browsers, issue trackers)
What: Connect to stdio MCP servers listed in the project config, merge their tool schemas into the tool list, proxy calls; heavy — defer until Tier 1/2 land
Where: new ai_core/mcp.rs, config.rs, ai_core/mod.rs tool registry

## TUI Experience Low Priority (defer indefinitely)
- External editor (Ctrl+G), clipboard copy (Ctrl+X), paste image (Alt+V)
- Thinking-block expand/collapse (Ctrl+T) — only if models emit reasoning
- Startup help header, changelog, retry/compaction indicators, taskbar progress

### Boxed Full-Screen Pickers
When: The /resume picker renders as a panel above the input (menu-styled rows),
not the boxed full-screen overlay the prototype drew
What: If the panel proves cramped for long session lists, lift tree.rs's box
renderer into a shared overlay used by /resume and an in-TUI /tree
Where: tui/render.rs panel_rows, src/tree.rs
