# rusti vs Pi: side-by-side feature comparison

A feature-by-feature comparison of rusti with the Pi coding agent, which rusti borrows several
ideas from (the session tree, the self-docs pointer, the shell-command behaviour).

## Versions compared

| Side | Version |
|---|---|
| rusti | 0.1.4 (`Cargo.toml`), feat-001 to feat-078, all `done` in `harness/feature_list.json` |
| Pi | npm package `@earendil-works/pi-coding-agent` 0.99.1 (2026-09-29) |

References in the tables are shortened:
- rusti paths are relative to `src/` unless a top-level file such as `readme.md` or `harness/...` is named.
- Pi paths are relative to the npm package root (its `README.md`, `docs/`, `examples/` and `CHANGELOG.md`).

The Pi side comes from Pi's own documentation and changelog, with one look at its `dist/` code to
confirm the default truncation limits; Pi was not run. Pi's MCP support is recent: 0.99.0 added
MCP, codemode and tool_search as built-in extensions (`CHANGELOG.md` 0.99.0 "Added";
`docs/mcp.md`), so older write-ups saying "Pi has no MCP" are out of date.

---

## Summary

### What rusti has that Pi lacks in core

- **Permission gate built in.**
  - rusti: each write, edit, command, background job, web fetch and MCP call asks Yes, No, or "Yes, and stop asking". An "always" answer is saved per project in `./model.json` `allow` (`ai_core/mod.rs`).
  - Pi: its docs say it "does not ask for approval before every tool call" (`docs/security.md:3`). Approval gates exist only as example extensions (`examples/extensions/permission-gate.ts`, `confirm-destructive.ts`, `protected-paths.ts`).
- **Project-root write guard.** rusti refuses writes outside the working directory unless `--yolo` is given (`ai_core/tools.rs`).
- **Plan mode as a core toggle.** `/plan` refuses every gated tool, even under `--yolo` (`ai_core/mod.rs`). In Pi, plan mode is only an example extension (`examples/extensions/plan-mode/`).
- **Sub-agents, background jobs, todo, ask_user and web_fetch are built-in tools.**
  - rusti ships `delegate`, `run_background`, `job_output`, `job_stop`, `todo`, `ask_user` and `web_fetch` (`ai_core/mod.rs`).
  - In Pi these are examples or packages: `examples/extensions/subagent/`, `todo.ts`, `question.ts`, `interactive-shell.ts`. Pi has no web-fetch tool in core; none is listed in `docs/cli.md` "Tools".
- **One-level undo.** `/undo` puts back the files the last turn changed and rewinds the session (`ai_core/tools.rs`, `tui/mod.rs`). Not in Pi core; `examples/extensions/git-checkpoint.ts` is the closest thing.
- **`/commit` macro.** It asks the model to stage and commit the work (`tui/app.rs`). Git branch and `git status` are also injected into every turn (`ai_core/mod.rs`). Pi only has example extensions (`auto-commit-on-exit.ts`, `dirty-repo-guard.ts`).
- **`/reload` rebuilds rusti from its own source and relaunches it** with the session kept (`tui/mod.rs`). Pi's `/reload` reloads extensions, keybindings and resources, and does not rebuild.
- **Single native binary.**
  - rusti is a Rust binary of about 2.2 MB, with no Node or Python needed (`readme.md`; `Cargo.toml` release profile).
  - Pi needs Node 22.19 or later (`README.md:27`), installed via npm or `pi.dev/install.sh`.

### The biggest Pi features rusti lacks

1. **Providers and auth.**
   - Pi: about 25 built-in providers, subscription OAuth through `/login` (Claude Pro/Max, ChatGPT/Codex, GitHub Copilot), native Anthropic, Google and Bedrock APIs, `models.json` custom providers, and virtual and classifier models (`docs/providers.md`, `docs/models.md`, `docs/virtual-models.md`).
   - rusti: any OpenAI-compatible `/v1/chat/completions` endpoint with a bearer key. No OAuth (`ai_core/llm.rs`).
2. **Thinking levels, and model cycling or scoping.**
   - Pi: `/thinking` and `--thinking off|minimal|low|medium|high|xhigh|max` (default `medium`), Shift+Tab, `thinkingBudgets`, and Ctrl+P / `--models` cycling (`docs/slash-commands.md`, `docs/settings.md:13`).
   - rusti: displays reasoning but cannot request a level. The request body is only model, messages, stream and tools (`ai_core/llm.rs`).
3. **An extensibility platform.**
   - Pi has a TypeScript extension API with tools, commands, shortcuts, providers, renderers and more than 20 lifecycle events (`docs/extensions.md`).
   - Pi also has Agent Skills (`docs/skills.md`), prompt templates (`docs/prompt-templates.md`), npm or git packages with `pi install` (`docs/packages.md`), and custom themes with hot reload (`docs/themes.md`).
   - rusti has none of these. Its only extension point is MCP.
4. **Programmatic interfaces.**
   - Pi has `--mode json` event streams (`docs/json.md`), a JSONL RPC protocol with about 30 commands (`docs/rpc-commands.md`), and a JS SDK built around `createAgentSession()` (`docs/sdk.md`).
   - rusti has plain one-shot text output only. `--json` is an open-work item.
5. **Message queueing while the agent runs.**
   - Pi: Enter queues a steering message, Alt+Enter queues a follow-up, and Alt+Up pulls queued messages back into the editor (`docs/usage.md:36-40`, `docs/keybindings.md`).
   - rusti: open-work "Follow-Up Queue".
6. **Session tooling.**
   - Pi has `/fork`, `/clone`, HTML export, `/share` to a gist, `/import`, branch summaries on `/tree`, and deleting sessions from the picker (`docs/sessions.md`, `docs/slash-commands.md`).
   - rusti has the tree, resume, rename, /new and markdown export.
7. **Manual `/compact` and a structured summary.**
   - Pi: summary sections Goal, Progress and Next Steps, cumulative read and modified file lists, and compact-and-retry on overflow (`docs/compaction.md`).
   - rusti: auto-compaction only.
8. **Cost display.** Pi's footer shows cost and `/session` shows per-session cost (`README.md`, `docs/sessions.md`). rusti shows tokens and context % only.
9. **MCP breadth.** Pi supports stdio plus streamable HTTP with OAuth, `pi mcp add|remove|list|login`, tool exposure modes, and lazy `tool_search` and `codemode` (`docs/mcp.md`, `docs/cli.md:141-182`). rusti supports stdio only.

### Same feature, different mechanism

| Feature | rusti | Pi |
|---|---|---|
| Session storage | One pretty-printed JSON tree per session in `.rusti/sessions/NAME.json` (unnamed ones auto-named; a legacy `./session.json` is still read), stored inside the project (`session.rs`) | Append-only JSONL v3 per session under `~/.pi/agent/sessions/--<path>--/` (`docs/session-format.md`, `docs/sessions.md`) |
| Branching | Pick an entry in `/tree` or `--tree`; the semantics follow Pi's (`readme.md` "session tree") | Same model, plus labels, filters, folding and optional branch summarization (`docs/sessions.md`) |
| Compaction | Automatic past `--context` (default 100k). Summarises all but the last 8 entries into a new branch: system → summary → copied tail (`ai_core/mod.rs`) | Automatic at `contextWindow − reserveTokens` (16384 reserved), keeps about 20k recent tokens, handles a cut that falls mid-turn, and also has manual `/compact` (`docs/compaction.md`) |
| Context files | First non-empty `AGENTS.md`, then `RUSTI.md`, then `CLAUDE.md`, from the cwd only; 20 KB cap; re-read every turn (`ai_core/mod.rs`) | All `AGENTS.md`/`CLAUDE.md` files from `~/.pi/agent`, parent directories and the cwd, concatenated. `SYSTEM.md` replaces the system prompt and `APPEND_SYSTEM.md` appends to it (`README.md`, `docs/security.md:57`) |
| Config | `~/.rusti/config.json` (global) plus `./model.json` (project allow list and overrides) (`config.rs`) | `~/.pi/agent/settings.json` plus `.pi/settings.json`; project resources load only after the project is trusted (`docs/settings.md`, `docs/security.md:27-57`) |
| `!cmd` / `!!cmd` | `!` output streams into the transcript live, then goes to the model; `!!` output never does (`readme.md` "shell commands") | `!` sends output to the model, `!!` does not (`docs/usage.md`) |
| Windows shell | The `shell` setting, then Git Bash, then bash on PATH, then `cmd /C` (`ai_core/tools.rs`) | The `shellPath` setting, then Git Bash, then bash on PATH, plus an optional `powershell` tool (`docs/windows.md:3,33-47`) |
| MCP config | An `"mcp"` block in `~/.rusti/config.json`; toggled live with `/mcp` | `~/.pi/agent/mcp.json` or `.pi/mcp.json` in the usual `mcpServers` shape; managed with the `/mcp` manager and `pi mcp ...` |
| MCP safety | Every MCP tool always asks and is refused in plan mode (`readme.md` "MCP servers") | No gate. Project `mcp.json` loads only after the project is trusted (`docs/mcp.md`) |
| Images | Ctrl+V/Alt+V saves the clipboard image and types its path; `read_file` then attaches it as an image part. Not yet verified against a vision endpoint (`ai_core/tools.rs`) | Paste or drag straight into the editor, `@file` arguments, auto-resize, and inline display in the terminal (`docs/usage.md`, `docs/settings.md`) |
| Themes | 15 compiled-in palettes picked with `/themes`, plus a `"light"` flag (`tui/theme.rs`) | A `system` theme that follows the terminal palette, `dark`/`light`, and custom JSON themes with hot reload (`docs/themes.md`) |

---

## 1. Interface and TUI

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Interactive TUI | **yes**: custom ANSI renderer with no framework, alternate screen, CSI 2026 sync (`tui/mod.rs`, `tui/render.rs`) | **yes**: regular mode (native scrollback) or fullscreen mode (fixed editor, transcript search, mouse), picked with `--tui-mode` (`docs/usage.md:86`) | rusti is hand-rolled on crossterm and always uses the alternate screen, with no scrollback mode |
| Bare command opens UI | **yes**: bare `rusti` on a TTY (feat-075, `main.rs`) | **yes**: `pi` | Same |
| Other front ends | **yes**: plain stream fallback when not a TTY (`tui/plain.rs`) | **yes**: print, json and rpc modes | See area 10 |
| Slash command menu | **yes**: `/` opens a filterable menu (`tui/app.rs`) | **yes**: autocomplete for built-ins, templates, skills and extensions | Pi's list is user-extensible |
| Keybindings | **yes, fixed**: Esc interrupt, Ctrl+C twice to quit, Ctrl+D, Ctrl+O output, Ctrl+T thinking, Ctrl+V/Alt+V image, Shift/Alt+Enter, history, PgUp/PgDn, mouse wheel (`tui/app.rs`) | **yes, configurable**: `~/.pi/agent/keybindings.json`, kill ring, undo, word navigation (`docs/keybindings.md`) | rusti bindings cannot be rebound |
| Editor features | **partial**: multi-line and history. No autocomplete or `$EDITOR` (open-work) | **yes**: `@` fuzzy file refs, Tab path completion, Ctrl+G `$EDITOR` (`docs/usage.md`) | Pi ahead |
| Markdown rendering | **yes**: headings, bold, code, bullets, fenced blocks, aligned tables, own highlighter (`tui/render.rs`) | **yes** (`docs/tui.md`) | Similar |
| Transcript tinting | **yes**: user, failed-tool and permission rows banded; reads folded; narration dimmed (feat-076, `tui/render.rs`) | partial: tool expand (Ctrl+O), thinking collapse (Ctrl+T) | Different styling approach |
| Thinking display | **yes**: italic block; Ctrl+T folds it, keeping a two-row live preview, saved as `hide_thinking` (feat-080, `tui/render.rs`) | **yes**: Ctrl+T collapse, `hideThinkingBlock` | Pi also toggles one block by click |
| Themes | **yes**: 15 built-in palettes, `/themes`, `light` flag (`tui/theme.rs`) | **yes**: system, dark and light themes plus custom JSON with hot reload (`docs/themes.md`) | Pi's are user-authored |
| Image input | **partial**: clipboard paste to a file path, `read_file` attaches it; last hop unverified (feat-061/062) | **yes**: paste or drag, `@img`, inline display (`docs/usage.md`) | Pi direct; rusti goes through a tool call |
| Queue while streaming | **no**: open-work "Follow-Up Queue" | **yes**: steer (Enter), follow-up (Alt+Enter), dequeue (Alt+Up) (`docs/usage.md:36-40`) | Notable gap |
| Footer | **yes**: plan, session, model, branch, tokens, ctx %; toggled with `/settings` (`tui/app.rs`) | **yes**: cwd, session, tokens, cache, cost, ctx, model | Pi shows cost |
| Clipboard copy of reply | **no** (open-work, low priority) | **yes**: `/copy` | |
| Extension UI (widgets, overlays) | **no** | **yes** (`docs/tui.md`) | |
| Unicode-width wrapping | **no**: byte-based (`tui/mod.rs`) | not stated in docs (presumably handled) | rusti's CJK and emoji wrapping is naive |

## 2. Sessions and history

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Storage | **yes**: JSON tree in the project directory (`session.rs`) | **yes**: JSONL v3 per cwd under `~/.pi/agent/sessions` (`docs/session-format.md`) | Location and format differ |
| Continue last | **yes**: `--resume` resumes the named or most recent session's active leaf (`main.rs`) | **yes**: `-c/--continue` | |
| Resume picker | **yes**: `/resume [n\|name]` shows messages, branches, age and last message (`tui/app.rs`) | **yes**: `-r` and `/resume`, with rename, delete, sort and filter keys (`docs/sessions.md`) | Pi can delete and sort from the picker |
| Tree and branching | **yes**: `/tree`, `--tree`, Pi semantics (`tree.rs`, `session.rs`) | **yes**: `/tree` with search, filters, fold, labels and timestamps | Pi richer |
| Branch summary | **no** | **yes**: optional summary when switching branches (`docs/compaction.md`) | |
| Fork or clone to a new file | **no** | **yes**: `/fork`, `/clone`, `--fork` | |
| New session in-app | **yes**: `/new [name]`, auto-named `s-YYYYMMDD-HHMM` when unnamed (`tui/mod.rs`) | **yes**: `/new` | |
| Naming | **yes**: `/rename` moves the file (`tui/mod.rs`) | **yes**: `/name`, `--name` | |
| Export | **yes**: `/export [file.md]` markdown (`session.rs`) | **yes**: `/export` HTML or JSONL, `pi --export`, `/share` (a Radius artifact, or a private gist via `gh`; `docs/usage.md:82`), `/import`, `/bug` | Pi has HTML, share and import |
| Undo | **yes, one level**: file snapshots plus rewind (`ai_core/tools.rs`) | not in core (example `git-checkpoint.ts`) | rusti ahead |
| Session stats | partial: footer tokens | **yes**: `/session` shows path, tokens and cost | |

## 3. Tools for the model

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Default set | 17 tools, always on (`ai_core/mod.rs`) | `read`, `bash`, `edit`, `write` on; `grep`, `find`, `ls` opt-in; `powershell` on Windows (`docs/cli.md:128-139`) | rusti ships a wider default set |
| Read | **yes**: `read_file` with offset/limit; images ≤ 4 MB | **yes**: `read` for text and images | |
| Write | **yes**: `write_file`, guarded, snapshotted, returns a diff stat | **yes**: `write` | |
| Edit | **yes**: `edit_file` and atomic `multi_edit`, exact unique match | **yes**: `edit`, exact replacements | rusti has a separate multi-edit tool |
| Delete and move | **yes**: `delete_file`, `move_file` | **no** dedicated tool (done via bash) | |
| Search | **yes**: `grep`, `glob`, `list_dir` (ripgrep if installed) | partial: `grep`, `find`, `ls`, off by default | |
| Shell | **yes**: `run_command` in the resolved shell (see area 4), 120 s timeout (`ai_core/tools.rs`) | **yes**: `bash`; `powershell` on Windows | |
| Background jobs | **yes**: `run_background`, `job_output`, `job_stop` (`ai_core/tools.rs`) | **no** in core (example `interactive-shell.ts`, tmux) | rusti ahead |
| Sub-agent | **yes**: `delegate`, one level (`ai_core/mod.rs`) | **no** in core (example `subagent/`) | rusti ahead |
| Todo list | **yes**: `todo` | **no** in core (example `todo.ts`) | |
| Ask the user | **yes**: `ask_user` | **no** in core (examples `question.ts`, `questionnaire.ts`) | |
| Web fetch | **yes**: `web_fetch`, 20 s, 5 MB, labelled untrusted (`ai_core/tools.rs`) | not in core | rusti ahead |
| Web search | **no** (open-work) | not in core | Neither |
| Output limits | 20,000 bytes per result (`ai_core/tools.rs`) | 2000 lines or 50 KB (`dist/core/tools/truncate.js:10-11`) | Pi allows more per call |
| Tool allowlisting | **no** | **yes**: `--tools`, `-xt`, `-nbt`, `-nt`, `defaultTools` with `+`/`-` | |
| Parallel or scripted calls | **no**: deliberately skipped (open-work "Parallel Tool Calls") | **yes**: `codemode` runs JavaScript in QuickJS that calls tools in parallel; opt-in (`docs/cli.md:148-178`) | |
| Tool search | **no** | **yes**: `tool_search`, opt-in (`docs/cli.md:180-182`) | |
| Override built-ins | **no** | **yes**: an extension registers the same tool name (`docs/extensions.md`) | |

## 4. User shell commands

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| `!cmd` sends output to the model | **yes**: runs when idle; output is added to the conversation as a user entry (`tui/app.rs`, `tui/mod.rs`) | **yes** (`docs/usage.md`) | Same idea |
| `!!cmd` keeps output from the model | **yes**: runs like `!cmd`, but its output never enters the session (feat-078) | **yes** | Parity |
| Live streamed output | **yes**: `!` and `!!` output streams into the transcript line by line while the command runs (feat-078) | not stated in docs | |
| Shell resolution | **yes**: resolved once at startup: the `"shell"` path in `~/.rusti/config.json`, else Git Bash under Program Files on Windows, else `bash` on PATH, else `cmd /C` on Windows and `sh -c` elsewhere; one shell for `run_command`, `run_background`, `!` and `!!` (feat-078, `ai_core/tools.rs`) | **yes**: `shellPath`, then Git Bash, then bash on PATH (`docs/windows.md`) | Same order as Pi |
| Command prefix and aliases | **yes**: `"shell_command_prefix"`, put on its own line before every command (`&` under cmd) (feat-078) | **yes**: `shellCommandPrefix`; alias recipe in `docs/shell-aliases.md` | Parity |
| Hook on `!` | **no** | **yes**: `user_bash` extension event | |
| MCP server launch shell | `cmd /C` on Windows, so `npx`/`uvx` shims work (`ai_core/mcp.rs`) | n/a (spawns the command directly) | |

## 5. Models and providers

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| API shape | partial: OpenAI-compatible chat completions only (`ai_core/llm.rs`) | **yes**: OpenAI completions and responses, Anthropic messages, Google, Bedrock and more (`docs/models.md`, `docs/providers.md`) | Largest gap |
| Built-in providers | none by name; any URL goes in a profile | **yes**: about 25 providers (`docs/providers.md`) | |
| Auth | API key sent as a bearer token (`ai_core/llm.rs`) | keys, `auth.json` with `$ENV`/`!cmd` values, OAuth, cloud credentials, `pi auth check` (`docs/providers.md`, `docs/cli.md:289-311`) | |
| Subscription login | **no** | **yes**: `/login` for Claude Pro/Max, ChatGPT/Codex, Copilot | |
| Profiles and switching | **yes**: `--add`, `--list`, `--use`, `/model`, `/use` (`config.rs`, `tui/app.rs`) | **yes**: `/model`, Ctrl+L, `--model provider/id[:thinking]` | |
| Cycling and scoped models | **no** (open-work "In-Place Model Switching") | **yes**: Ctrl+P, `--models`, `/scoped-models` | |
| Thinking level | **no**: display only | **yes**: seven levels (`off` through `max`), `/thinking`, budgets, per-model defaults (`docs/settings.md:13`) | |
| Local models | **yes**: any OpenAI-compatible server (Ollama example in `readme.md`) | **yes**: Ollama, LM Studio, vLLM, llama.cpp router (`docs/llama-cpp.md`) | |
| Custom provider code | **no** | **yes**: `pi.registerProvider()`, virtual models (`docs/custom-provider.md`, `docs/virtual-models.md`) | |
| Retry | **yes**: 3 tries with 1, 2 and 4 s backoff on 408, 429 and 5xx; no Retry-After (`ai_core/llm.rs`) | **yes**: configurable `retry.*` | |
| Prompt caching | **no** (open-work) | **yes**: `PI_CACHE_RETENTION`; footer shows cache usage | |

## 6. Context and compaction

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Auto compaction | **yes**: past `--context` (default 100k); keeps the last 8 entries (`ai_core/mod.rs`) | **yes**: reserve- and keep-token thresholds, split-turn handling, retry on overflow (`docs/compaction.md`) | Pi's rules are richer |
| Manual `/compact` | **no** (open-work) | **yes**: `/compact [instructions]` | |
| Structured summary | partial: a free-form prompt (`ai_core/mod.rs`) | **yes**: Goal, Constraints, Progress and similar sections, plus file lists | |
| Compaction hook | **no** | **yes**: `session_before_compact` | |
| Context files | **yes**: the first of AGENTS.md, RUSTI.md, CLAUDE.md in the cwd, re-read each turn | **yes**: all levels concatenated; `-nc` disables them | Pi walks parent directories |
| System prompt override | **yes**: `~/.rusti/SYSTEM.md`, `APPEND_SYSTEM.md`, `--system-prompt`, `--append-system-prompt`; replaces only the base text (readme.md § project instructions) | **yes**: `SYSTEM.md`, `APPEND_SYSTEM.md`, `--system-prompt`, `--append-system-prompt` | rusti reads no project `.rusti/SYSTEM.md` yet (waits for project trust) |
| Git state in prompt | **yes**: branch and `git status --short` every turn (`ai_core/mod.rs`) | not in core | rusti ahead |
| Self-docs pointer | **yes**: the binary's docs are written to `~/.rusti/docs` (`ai_core/mod.rs`, feat-074) | **yes**: rusti ported this from Pi's `<docs>` pointer | Same |
| Tokens and context % | **yes**: server usage, or a chars/4 estimate marked `~` | **yes** | |
| Cost | **no** (open-work) | **yes** | |

## 7. Permissions and safety

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Approval prompts | **yes**: gated tools and every MCP tool ask Yes, No or "stop asking" (`ai_core/mod.rs`) | **no**: "does not ask for approval before every tool call" (`docs/security.md:3`) | Opposite defaults |
| Saved approvals | **yes**: per-project `allow` in `./model.json`; no UI to forget them (open-work) | n/a | |
| Plan mode | **yes**: `/plan` in the TUI; no `--plan` flag | **no** in core (example `plan-mode/`) | |
| Write guard | **yes**: project-root guard (`ai_core/tools.rs`) | **no** in core (example `protected-paths.ts`) | |
| Read sandbox | **no**: open by design (open-work) | **no** | Same |
| Bypass | `--yolo` / `RUSTI_YOLO=1` | n/a; already ungated | |
| Non-interactive | piped runs deny every gated tool unless `--yolo` is given (`readme.md` "safety") | runs ungated | |
| Project trust | **yes**: a `./model.json` that sets `mcp`, `shell`, `shell_command_prefix`, `hooks`, `read_allow` or `allow`, or a `./.rusti/` with prompts, skills, themes or `SYSTEM.md`, is asked about once (Yes / No / Always, saved in `~/.rusti/trust.json`); piped and one-shot runs need `--trust` (`config.rs`) | **partial**: gates loading of project settings, extensions, skills, prompts, themes, `SYSTEM.md` and `mcp.json`, but not tool actions or context files. Decided by `/trust`, `-a/--approve` or `defaultProjectTrust` (`docs/security.md:27-57`) | Both gate startup config, not tool actions |
| OS or container sandbox | none | documented recipes only (`docs/containerization.md`; example `sandbox/`) | |
| Dangerous-command detection | none | example `confirm-destructive.ts` only | Neither in core |
| Untrusted labelling | partial: `web_fetch` output is labelled (`ai_core/tools.rs`) | not stated in docs | |

## 8. Configuration

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Global file | `~/.rusti/config.json`, moved with `RUSTI_HOME` (`config.rs`) | `~/.pi/agent/settings.json`, moved with `PI_CODING_AGENT_DIR` | |
| Project file | `./model.json`: `allow`, `max_iters`, `context`, and any override; code-running and permission keys only once trusted (`config.rs`) | `.pi/settings.json`, trust-gated | rusti's project file sits in the repo root |
| Settings UI | partial: `/settings` (footer segments), `/themes`, `/mcp` | **yes**: `/settings`, `pi config` | |
| Malformed-file safety | **yes**: reported, and saving is refused (`config.rs`) | not stated in docs | |
| Env vars | `LLM_URL`, `LLM_KEY`, `LLM_MODEL`, `RUSTI_SESSION`, `RUSTI_MAX_ITERS`, `RUSTI_CONTEXT`, `RUSTI_YOLO`, `RUSTI_HOME` | about 15 `PI_*` variables plus provider keys (`docs/environment-variables.md`) | |
| CLI flags | `--tui --session --resume --tree --url --key --model --use --list --add --max-iters --context --system-prompt --append-system-prompt --yolo --trust --self-test --help --version` (`main.rs`) | about 40 flags, plus `install`, `remove`, `update`, `list`, `config`, `mcp` and `auth` subcommands (`docs/cli.md`) | |
| Iteration cap | **yes**: `--max-iters` (default 50) | not stated as a setting | |

## 9. Extensibility

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| MCP client | **yes, stdio only**: JSON-RPC 2.0, protocol 2024-11-05, no SDK, tools only (no resources or prompts), text content only, `/mcp` toggle, tools named `mcp__server__tool` (`ai_core/mcp.rs`, feat-055) | **yes**, new in 0.99.0: stdio and streamable HTTP, OAuth, resources, `/mcp` manager, `pi mcp add/remove/list/login`, `mcp.log`. By default MCP tools are reached through codemode rather than declared to the model; `direct`, `deferred` and `hidden` exposure are also available (`docs/mcp.md`) | Pi adds HTTP, OAuth, resources, CLI management and lazy exposure |
| Extensions and plugins | none | **yes**: TypeScript API for tools, commands, shortcuts, flags, providers, renderers and events (`docs/extensions.md`) | Largest structural gap |
| Skills | none | **yes**: Agent Skills `SKILL.md`, `/skill:name` (`docs/skills.md`) | |
| Prompt templates | none; `/commit` is hard-coded | **yes**: markdown templates with arguments (`docs/prompt-templates.md`) | |
| Packages | none | **yes**: `pi install npm:/git:` (`docs/packages.md`) | |
| Hooks | none | **yes**, through extension events | |
| Custom themes | **no** (compiled in) | **yes** | |
| Self-rebuild | **yes**: `/reload` runs cargo and relaunches (`tui/mod.rs`) | n/a; `/reload` reloads resources | Different meaning |

## 10. Output modes and SDK/RPC

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| One-shot | **yes**: `rusti "task"`; answer on stdout, status on stderr (`main.rs`) | **yes**: `-p`, or automatic when stdin/stdout is not a TTY; piped stdin is prepended to the prompt (`docs/cli-integration.md`) | Piped stdin keeps rusti in one-shot mode but is not read as the task; with no argument the task defaults to "say hello" (`main.rs`) |
| JSON events | **no** (open-work `--json`) | **yes**: `--mode json` (`docs/json.md`) | |
| RPC | none | **yes**: `--mode rpc`, about 30 commands, extension-UI sub-protocol (`docs/rpc-commands.md`, `docs/rpc-extension-ui.md`) | |
| SDK or library | **no**: no `[lib]` target; only an internal `Event` sink (`ai_core/mod.rs`) | **yes**: `createAgentSession()` and related APIs (`docs/sdk.md`) | |
| Offline self-test | **yes**: `--self-test` against a fake SSE server | not a user command | |

## 11. Install and platforms

| Feature | rusti | Pi | Difference |
|---|---|---|---|
| Install | `install.ps1` / `install.sh` pull a GitHub Release binary into `~/.rusti/bin`; `cargo install --git` also works (`readme.md`) | `npm install -g @earendil-works/pi-coding-agent`, or `curl -fsSL https://pi.dev/install.sh \| sh` on macOS/Linux; `pi update` (`README.md:27-32`) | Native binary vs. Node package |
| Runtime dependency | none (about 2.2 MB binary) | Node 22.19 or later (`README.md:27`) | |
| Release targets | windows-x86_64, linux-x86_64, macos arm64 and x86_64 (`.github/workflows/release.yml`); no Linux arm64 | anywhere Node runs, including Termux on Android (`docs/termux.md`) | |
| Windows | **yes**: Git Bash when installed, else `cmd`; clipboard via PowerShell | **yes**: Git Bash by default, optional PowerShell tool, WSL (`docs/windows.md`) | |
| Terminal and tmux guides | none | **yes**: `docs/terminal-setup.md`, `docs/tmux.md` | |
| Offline or telemetry controls | none needed (no telemetry) | `--offline`, `PI_TELEMETRY=0`, install telemetry that is opt-out | |

---

## Notable gaps (observations, not a roadmap)

- **Biggest gaps in rusti:**
  - provider breadth, native APIs and OAuth
  - thinking-level control
  - any extension, skill or template mechanism
  - JSON, RPC and SDK output
  - the steer and follow-up queue
  - manual `/compact` and cost display
- **Where rusti is ahead of Pi's core:**
  - safety by default: gate, plan mode, write guard, undo
  - a batteries-included tool set: sub-agent, background jobs, web_fetch, todo, ask_user
  - a zero-dependency native binary
- **Closed since the first comparison:** Git Bash shell resolution, `!!cmd` and live `!` output
  shipped in feat-078, matching Pi's user-shell behaviour.
