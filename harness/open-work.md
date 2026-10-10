# Open Work

## Suggested Order (2026-09-13 review)

Tier 1 — done (feat-028/029/030/031: prompt tokens, compaction, retry, failure output)
Tier 2 — done (feat-032/033/034/035/036/037: /tree picker, /undo, --help/--version, git + /commit, ! bash mode, footer). Parallel Tool Calls examined and skipped 2026-09-13, then built in feat-tools-phase4 (F07).
Tier 3 — resequenced 2026-09-13 by friction-removed ÷ effort, not by the order they were filed:
  3a — done (feat-038/039/040/041: project config, plan mode, web fetch, /export; /session dropped as redundant
     with /resume, see feat-042)
  3b: done or retired. Tool Output Expand Toggle -> feat-059, Diff in the Tool Line -> feat-060, Multi-Level Undo
     and Test Loop rejected 2026-09-14 (verdicts and recheck conditions in harness/memory/graveyard.md).
     Read-Side Sandbox is all that is left, and it only matters if rusti runs somewhere untrusted.
  3c (only when the need is real): Prompt Caching (provider must support it), Native Anthropic/Gemini (only off an
     OpenAI-compatible endpoint), Image Input (changes Entry.content to parts), MCP Client (largest, ecosystem reach)

## Priority — Add When Needed

## TUI Experience Gaps (from pi comparison, 2026-04-10 session)

### Editor Autocomplete
When: Typing speed matters; model names not discoverable (slash commands, `@` files and Tab paths done in feat-at-file-refs)
What: Autocomplete for saved model names (argument completion, see below)
Where: tui/app.rs menu_items

### Slash-Menu Follow-ups (prototype parity landed 2026-09-09)
When: The menu exists (`/` opens it) but only completes command names
What: Extend the same panel to saved model names (file paths landed in feat-at-file-refs); the commands
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

### Parallel Tool Calls — follow-ups
When: feat-tools-phase4 runs adjacent parallel-safe calls concurrently (spawn_blocking works on current_thread)
What: codemode (scripted calls, D6 deferred); Esc does not kill commands already running in a parallel group (nor a
lone one); two parallel `run_command`s can still collide on a shared lock such as cargo's target dir (D6 accepts it);
images a parallel group's reads attach arrive in completion order, not call order
Where: ai_core/mod.rs run_agent step 2, ai_core/tools.rs PENDING_IMAGES

### Clippy `read amount is not handled` (pre-existing)
When: noticed 2026-09-14 while verifying feat-058; `cargo clippy` fails on it, so clippy can't be a gate until it goes
What: `s.read(&mut buf).unwrap()` ignores the returned count, so a short read silently yields a truncated buffer
Where: ai_core/mod.rs:566 (the self-test's fake server)

## Safety

### Read-Side Sandbox
When: Model reads/lists/greps paths outside the project (../, absolute, ~) — writes are already guarded
What: Extend `tools::guard` to read_file/list_dir/grep/glob behind an `--allow-outside` flag; today reads are deliberately open so the model can look at dependency sources
Where: ai_core/tools.rs guard()

### Outside-Root Edits Ask Before Being Refused
When: Manual mode, an edit to a path outside the project: the user answers Yes, then the root guard refuses it anyway
What: Check the root guard before the permission prompt, so a call that will be refused is not asked about first
Where: ai_core/mod.rs permitted(), ai_core/tools.rs guard()

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

## Model / API

### Prompt Caching Follow-ups
When: feat-tools-phase4 added opt-in `"cache": "anthropic" | "openai"` markers (F10 C3)
What: a 1h TTL / `prompt_cache_retention` option; cache warming (needs a cost model, flagged in F10)
Where: ai_core/llm.rs mark_cache, config.rs CacheMode

### Image Input — follow-ups
When: F03 (feat-image-input) attaches `@` images on submit, resizes and has the vision flag; left over:
What: (a) done in feat-tools-phase4: PENDING_IMAGES is a list, so two reads in one batch both attach;
(b) a dragged path without a leading `@` is not attached — bracketed paste (A4) could mark pasted paths;
(c) a steer's `@` images are found when it is queued and attached on delivery, not re-checked in between;
(d) plain mode prints `· attached` rows to stdout, as every Event::Text row is, which a pipe then carries;
(e) inline terminal display (kitty/iTerm2/sixel), an images kill switch, and normalising MCP tool images
Where: ai_core/tools.rs PENDING_IMAGES, ai_core/mod.rs emit, tui/mod.rs Job::Task

### Native Anthropic / Gemini APIs
When: Endpoint isn't OpenAI-compatible
What: Provider enum on the profile; per-provider request/SSE mapping behind the existing ChatResult
Where: ai_core/llm.rs, config.rs

### Streaming Tool-Call Args Display
When: A long write_file looks frozen while arguments stream
What: Show the tool name as soon as it's known and a byte counter while raw_arguments accumulate
Where: ai_core/llm.rs handle_event emit, tui/app.rs ToolStart

### Object-Valued Tool Arguments
When: A proxy sends `function.arguments` as a JSON object rather than a string (not seen yet: 9router sends strings)
What: Serialize the object into raw_arguments instead of dropping it; today the call arrives with no arguments
Where: ai_core/llm.rs handle_event

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

### Extension Model Follow-ups
When: feat-extension-model landed templates, skills, theme files, hooks and MCP prompts in the TUI
What: expand `/template` and `/skill:` in one-shot runs too; `${@:N:L}` argument slices; argument completion
for template hints
Where: main.rs task dispatch, resources.rs expand, tui/app.rs menu_items
