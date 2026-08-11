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

## TUI Experience Low Priority (defer indefinitely)
- External editor (Ctrl+G), clipboard copy (Ctrl+X), paste image (Alt+V)
- Thinking-block expand/collapse (Ctrl+T) — only if models emit reasoning
- Session tree browser/fork/named sessions — /tree text dump works
- Startup help header, changelog, retry/compaction indicators, taskbar progress

## Cleanup (Low Priority)
- Delete orphan files at root: `config.rs`, `llm.rs`, `tools.rs` (superseded by src/ modules)
- Remove unused `add_profile()` in config.rs or wire it into --add flow
