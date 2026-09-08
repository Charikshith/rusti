# Open Work

## Priority — Add When Needed

### Context Compaction
When: Long conversations hit token limits
What: Summarize older messages, keep recent context
Where: ai_core/mod.rs run_agent loop

### reasoning_content Support
When: User switches to thinking models (QwQ, DeepSeek R1)
What: Parse `reasoning_content` field from SSE delta alongside `content`
Where: ai_core/llm.rs chat_stream

### Command Timeouts
When: run_command hangs (infinite loop, blocked I/O)
What: Kill process after N seconds (default 30s)
Where: ai_core/tools.rs run_command

### TLS Support
When: User needs https:// LLM endpoint
What: Enable reqwest `default-tls` or `rustls-tls` feature
Where: Cargo.toml, ai_core/llm.rs

### unicode-width Word Wrap
When: CJK or emoji output garbles the TUI
What: Use unicode-width crate for character display width
Where: tui/mod.rs word_wrap()

## TUI Experience Gaps (from pi comparison, 2026-04-10 session)

### Markdown Rendering in Assistant Messages
When: Code answers become walls of unformatted text (pi renders bold/code/lists)
What: Render markdown in assistant text: code blocks, bold, lists, inline images
Where: tui/render.rs colorize_row / new renderer pass

### Tool Output Visibility
When: User sees `✓ cargo test` but not WHY it failed
What: Show expandable tool stdout/stderr in tool lines (Ctrl+O expands all); result already stored in session entries, this is a rendering change only
Where: tui/render.rs + tui/app.rs ToolEnd handler

### Editor Autocomplete
When: Typing speed matters; slash commands/names not discoverable
What: Fuzzy autocomplete in input: slash commands, saved model names, file paths
Where: tui/app.rs input handling (fd/fuzzy lib or lazy prefix match)

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

### ★ Native Search Tools: list_dir / grep / glob
When: Model shells out via run_command to find anything — slow, cmd-vs-sh platform-dependent, error-prone
What: Add `list_dir(path)`, `grep(pattern, path, glob?)`, `glob(pattern)` tools; walk with std::fs, regex crate only if plain substring proves insufficient; cap results like MAX_RESULT
Where: ai_core/tools.rs + tool_schemas()/dispatch() in ai_core/mod.rs

### ★ MAX_ITERS Too Low
When: A real task (read 3 files, edit 2, run tests, fix, rerun) dies with "hit max iterations without a final answer"
What: Raise default (50+) and make it a flag/env (`--max-iters`); pair with a cost/turn guard rather than a hard small cap
Where: ai_core/mod.rs MAX_ITERS, main.rs flags

### ★ Project Instructions File
When: Agent doesn't know the project's conventions (build cmd, style, don't-touch dirs) and has to rediscover them each session
What: Read `AGENTS.md` (fallback `RUSTYPI.md`/`CLAUDE.md`) from cwd, append to the system prompt; entries[0] already refreshes every turn so edits are live
Where: ai_core/mod.rs SYSTEM_PROMPT → fn system_prompt()

### Retry with Backoff
When: Transient errors (connection reset, 429, 5xx) fail the whole turn
What: Retry chat_stream up to N times with exponential backoff; surface `⚠ retrying…` in the TUI; never retry after partial content was streamed
Where: ai_core/llm.rs chat_stream, ai_core/mod.rs run_agent

### Parallel Tool Calls
When: Model emits several independent tool calls in one turn; they run one by one
What: Execute the batch concurrently (tokio::join_all or spawn_blocking for sync tools); preserve result order by tool_call_id
Where: ai_core/mod.rs run_agent tool loop

### Multi-Edit / Patch Tool
When: Several replacements in one file cost several round-trips
What: `edit_file` accepting a list of {old_text,new_text}, all-or-nothing; or a unified-diff `apply_patch` tool
Where: ai_core/tools.rs edit_file

### Sub-Agents / Task Delegation
When: Main context balloons from a self-contained subtask (e.g. "investigate why tests fail")
What: `delegate(task)` tool that runs a fresh run_agent with its own Session, returns only the final summary
Where: ai_core/mod.rs (new tool + dispatch)

## Safety (currently none)

### ★ Permission Prompt for run_command / write_file
When: Model can run any command or overwrite any file with zero confirmation
What: allow / deny / always-allow-this-session prompt routed through Event::Ask; `--yolo` flag skips; deny returns a tool error the model can react to
Where: ai_core/mod.rs dispatch(), tui/app.rs Ask handler, main.rs flag

### Diff Before Edit + Undo
When: An edit lands wrong and there's no way to see or revert it
What: Show the unified diff of edit_file/write_file in the tool line (ties into Tool Output Visibility); keep the previous content to support `/undo` of the last file change
Where: ai_core/tools.rs (capture before-image), tui/render.rs, tui/app.rs command

### Working-Directory Sandbox
When: Model reads/writes paths outside the project (../, absolute, ~)
What: Canonicalize paths and refuse anything outside cwd unless `--allow-outside`; applies to read/write/edit/list/grep
Where: ai_core/tools.rs (shared path guard)

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

### Named / Multiple Sessions
When: One session.json per cwd — switching tasks clobbers history
What: `--session NAME` → `.rustypi/sessions/NAME.json`; `/session list|switch`
Where: session.rs PATH, main.rs, tui/app.rs command

### /undo Last Turn
When: A bad turn poisons the context
What: Move active leaf to the parent of the last user entry (tree already supports it), remove its rendered lines
Where: session.rs select(), tui/app.rs command

### Export Transcript
When: Sharing a session or filing an issue from it
What: `/export [file.md]` writes the active path as markdown (user/assistant/tool blocks)
Where: tui/app.rs command, session.rs path()

## Model / API

### Cost & Token Tracking
When: No idea how much a session costs or how close to the context limit it is
What: Parse `usage` from the final SSE chunk (most servers send it), accumulate per session, show in footer (ties into Context/Token Footer)
Where: ai_core/llm.rs handle_event, session.rs, tui/render.rs

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

## TUI Experience Low Priority (defer indefinitely)
- External editor (Ctrl+G), clipboard copy (Ctrl+X), paste image (Alt+V)
- Thinking-block expand/collapse (Ctrl+T) — only if models emit reasoning
- Session tree browser/fork/named sessions — /tree text dump works
- Startup help header, changelog, retry/compaction indicators, taskbar progress

## Cleanup (Low Priority)
- Delete orphan files at root: `config.rs`, `llm.rs`, `tools.rs` (superseded by src/ modules)
- Remove unused `add_profile()` in config.rs or wire it into --add flow
