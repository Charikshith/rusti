# rusti

Minimal coding agent in Rust: async core (tokio + reqwest), custom ANSI
TUI (no ratatui), ~2.2 MB release binary.
How it compares with the Pi coding agent: [docs/rusti-vs-pi.md](docs/rusti-vs-pi.md).

## Install

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/Charikshith/rusti/master/install.ps1 | iex
```
```sh
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/Charikshith/rusti/master/install.sh | sh
```

Either puts `rusti` in `~/.rusti/bin` and adds it to your PATH; run it again to update.
To uninstall, delete `~/.rusti` (that also removes your saved models) and the PATH entry.
From source instead: `cargo install --git https://github.com/Charikshith/rusti`.
New releases: push a `v*` tag and `.github/workflows/release.yml` builds and attaches the binaries.

## Use

```sh
cargo build --release            # when working on rusti itself

# Settings: ~/.rusti/config.json holds models, keys, theme, footer and MCP servers for every
# project (RUSTI_HOME moves it). ./model.json holds this project's "allow" list, max_iters and
# context; any other key put there overrides the global one here (keys that run code or steer
# permissions need the folder trusted first, see "project trust"). First launch asks for a model:
./target/release/rusti "add a --version flag to src/main.rs"

# manage saved models
./target/release/rusti --list            # show saved profiles (* = default)
./target/release/rusti --use <name>      # switch default
./target/release/rusti --add             # add another profile interactively

# TUI (custom ANSI renderer, streaming transcript, Esc quits): bare `rusti` in a
# terminal opens it; --tui forces it. A task argument or piped stdin is a one-shot run.
rusti

# session tree (pi-style branching)
./target/release/rusti --tree            # browse + branch from earlier entries
./target/release/rusti --resume          # continue the latest session from its active leaf

# flags/env still override the saved profile
./target/release/rusti --max-iters 100 "task"   # tool rounds per task (default 50, env RUSTI_MAX_ITERS)
./target/release/rusti --context 60000 "task"   # compact history once the prompt exceeds N tokens (default 100000, env RUSTI_CONTEXT; see § compaction)
./target/release/rusti --json "task"            # the run as JSON lines on stdout, for programs (see § JSON and RPC modes)
./target/release/rusti --rpc                    # drive rusti over JSON lines on stdin/stdout
./target/release/rusti --yolo "task"            # no permission prompts, no project-root guard (env RUSTI_YOLO=1)
./target/release/rusti --trust "task"           # trust this folder's model.json / .rusti without asking
./target/release/rusti --self-test       # offline check, fake server
./target/release/rusti --help            # all flags and slash commands
```

In the TUI: `/plan` toggles plan mode (the agent reads and proposes but every write and
command is refused), `/tree` browses and branches the session, `/undo` puts back the files
the last turn changed, `/commit` stages and commits the work, `/export` writes the
transcript out as markdown, `/settings` chooses which status-line segments are shown,
`/mcp` switches MCP servers on and off, `!cargo test` runs a shell command whose output the model
sees on the next turn, and `!!git log` runs one whose output only you see. Your own prompt
templates, skills and MCP prompts join the `/` menu too (see "extensions"). `/new [name]` saves the session and starts an empty one in place, keeping the model, plan mode, MCP servers and background jobs. `/resume` with no argument lists saved sessions and switches to the
one you pick; with a name or number it switches straight to it. The status line carries
plan mode, the session, model, git branch, tokens used, how full the context is and, when
the provider reports it, how much of the last prompt came from its cache (`cache 82%`).
The system prompt holds only what rarely changes; git status and plan mode travel on each
user message instead, so a provider's prompt cache can reuse everything sent before.

`model.json` example:
```json
{"default": "ollama", "models": [{"name": "ollama", "url": "http://localhost:11434/v1/chat/completions", "key": "", "model": "llama3.2"}]}
```

`"cache"` on a profile adds explicit prompt-cache markers, off by default because a server
that does not know the fields may reject the request. `"anthropic"` (for Anthropic-format
endpoints such as OpenRouter's `anthropic/*`) puts `cache_control` on the system prompt, the
last tool and the newest message; `"openai"` sends a `prompt_cache_key` derived from the
session file, so one session's requests share a cache across restarts.

It doubles as the project config. Answering `[a]lways` at a permission prompt appends the
tool to `"allow"`, so the next run doesn't ask again; `"max_iters"` and `"context"` set
per-project defaults that flags and env still override:
```json
{"allow": ["read_file", "run_command"], "max_iters": 100, "context": 60000}
```

`read_file` returns at most 2000 lines or 50 KB per call. A cut read ends with
`[Showing lines 1-2000 of 5231. Use offset=2001 to continue.]`, so the model can page through
the rest; a single line over the cap gets a `sed -n 'Np' FILE | head -c N` hint instead.
`"read_max_bytes"` (in either file) changes the byte cap; `0` removes both caps, at your own
risk: every result is re-sent on each later turn until compaction.

When the model asks for several tools in one turn, every permission prompt comes first, one
at a time in call order. Then reads, searches, `run_command`, `run_background`, `web_fetch`
and MCP calls that sit next to each other run at the same time; a write, `ask_user`,
`delegate` or `todo` runs alone in its place, so a read the model put after a write sees it.
Results reach the model in call order. The `batch` tool (`{"calls": [{"name", "args"}]}`)
does the same for a model that makes one call per turn, and returns one combined result.

## architecture

```
main.rs      CLI
├── rpc      --json event lines and the --rpc command loop
├── config   model.json profiles
├── session  tree-shaped session persistence (.rusti/sessions/NAME.json)
├── tree     interactive session tree browser (ANSI TUI / plain list)
└── ai_core  agent loop, LLM client, tool dispatch, event system
    ├── llm    reqwest SSE streaming client
    └── tools  read/write/edit/multi_edit/delete/move file, grep/glob/list_dir, run command (timeout),
    │          background jobs, todo, delegate (sub-agent), ask user, web_fetch, batch;
    │          permission gate + project-root guard; parallel-safe calls run concurrently
    └── mcp    stdio MCP client; deferred servers + tool_search (BM25)

mod tui     custom ANSI TUI (no ratatui) + plain stream fallback
```

## file map (src/)

| file | lines | responsibility |
|------|------:|----------------|
| main.rs | ~120 | CLI entry, flags, config resolution, session/task dispatch |
| config.rs | ~90 | model.json load/save/resolve, interactive profile creation |
| session.rs | ~130 | session tree: entries with id/parentId, active leaf, path, select, save/load |
| tree.rs | ~160 | session tree browser: ANSI TUI (interactive) or plain numbered list (piped) |
| tui.rs | ~190 | custom ANSI TUI renderer: scrolling transcript, streaming text, ask_user input |
| ai_core/mod.rs | ~160 | run_agent loop, event system (SINK), tool dispatch, self-test |
| ai_core/llm.rs | ~80 | reqwest SSE streaming client, tool-call argument accumulation |
| ai_core/mcp.rs | ~410 | MCP client: stdio JSON-RPC 2.0, spawns servers, merges their tools in as `mcp__<server>__<tool>` and their prompts in as `/<server>:<prompt>`, `/mcp` toggles them |
| ai_core/hooks.rs | ~200 | hooks: user commands on six events, JSON in on stdin, a JSON decision out on stdout |
| resources.rs | ~300 | prompt templates and skills: discovery, frontmatter, argument expansion |
| ai_core/tools.rs | ~470 | 17 tools: read(offset/limit)/write/edit/multi_edit/delete/move, grep/glob/list_dir (ripgrep if installed, std fallback), run_command (timeout), run_background/job_output/job_stop, todo, ask_user, web_fetch (tags stripped); project-root guard |

**Total application code: ~1,015 lines**

## safety

Every tool that changes state (`write_file`, `edit_file`, `multi_edit`, `delete_file`,
`move_file`, `run_command`, `run_background`) or reaches the network (`web_fetch`)
asks before running:

```
ℹ allow run_command cargo test? [y]es / [n]o / [a]lways (saved)
```

`a` allows that tool from now on — it is saved to `model.json`, so later runs don't ask
again. `/plan` refuses all of them outright, whatever the saved answers say, until you
toggle it back off. A denial is returned to the model
as a tool error so it can explain or ask instead of retrying. With piped stdin there
is nobody to answer, so everything is denied — pass `--yolo` for scripted runs.

Writes are also refused outside the working directory (`../x`, absolute paths
elsewhere), even for files that do not exist yet. Reads are deliberately open so the
model can inspect dependency sources. `--yolo` lifts both the prompts and the guard.

### project trust

A cloned repo's `model.json` could otherwise start its own MCP servers, swap the shell
or pre-approve `run_command` before you type anything. So when `./model.json` sets `mcp`,
`shell`, `shell_command_prefix`, `hooks`, `read_allow` or `allow` (or `./.rusti/` has
`prompts`, `skills`, `themes` or `SYSTEM.md`, or there is a `./.agents/skills`), the TUI asks
once at launch:

```
This folder's model.json sets mcp, shell, allow. Trust it? [y]es / [N]o / [a]lways
```

`y` trusts it for this run, `a` also saves it by canonical path in `~/.rusti/trust.json`.
Untrusted, those keys and folders are ignored with one warning line (keys are saved back untouched);
`max_iters`, `context`, `theme` and `footer` still apply. Piped and one-shot runs never
ask and stay untrusted unless `--trust` is given. `--yolo` does not imply `--trust`.

## sub-agents and background jobs

`delegate(task)` runs a fresh agent with the same tools in its own session file
(`.rusti/sessions/sub-<pid>-<ts>.json`) and returns only its final report, keeping
the subtask's reads and edits out of the main context. One level deep; the sub-agent
streams into the same transcript. `run_background` starts a server or watcher and
returns a job id; `job_output` reads what it has printed so far, `job_stop` kills the
whole process tree. Jobs outlive the agent if not stopped. `todo` lets the model keep
a visible checklist for multi-step work.

## shell commands

The model's `run_command` and `run_background` and your own `!cmd` / `!!cmd` all run in one
shell, chosen once at startup: the `"shell"` path in `~/.rusti/config.json` (a leading `~` is
your home folder), else on Windows Git Bash under Program Files or Program Files (x86), else
`bash` on PATH, else `cmd /C` on Windows and `sh -c` elsewhere. Bash runs as `bash -c <cmd>`.
The system prompt tells the model which shell that is. `"shell_command_prefix"` is put in front
of every command, on its own line (joined with `&` under cmd):
```json
{"shell": "~/scoop/apps/git/current/bin/bash.exe", "shell_command_prefix": "shopt -s expand_aliases"}
```

`!cmd` and `!!cmd` stream their output into the transcript as it arrives. `!cmd`'s output is
then added to the conversation for the model; `!!cmd`'s never is.

## project instructions

rusti's own docs (this readme and `--help`) are compiled into the binary and written to
`~/.rusti/docs/`; the system prompt carries only their paths and a topic map, so asking rusti
"how do I change the theme?" makes it read them instead of guessing.

If the working directory has an `AGENTS.md` (fallback `RUSTI.md`, then `CLAUDE.md`),
its contents are appended to the system prompt. The file is re-read every turn, so
edits take effect on the next message without a restart.

`~/.rusti/SYSTEM.md` replaces rusti's built-in base prompt, and `~/.rusti/APPEND_SYSTEM.md`
is added after it. `--system-prompt X` and `--append-system-prompt X` (repeatable) win over
those files; X is read as a file when one exists at that path, else used as the text itself.
Only the base text is replaced: the shell line, the docs pointer, the project instructions
above, git context and the plan-mode block always stay. Both files are re-read every turn,
and startup prints `ℹ system prompt: …` naming whatever changed it (an unreadable file is
skipped and named there too). `/reload` keeps the
flags. A project-level `.rusti/SYSTEM.md` is not read yet: it waits for project trust.

## markdown

Model prose is rendered, not printed raw: ATX headings and `**bold**` come out bold,
```inline code``` in cyan, `-`/`*` bullets as bullet glyphs, and fenced blocks in their own
colour with the fence lines hidden. No parser crate — markers become style runs over
the visible text, so wrapping still measures real columns. Fenced bodies are truncated
rather than reflowed.

## MCP servers

An MCP server is **any executable** that speaks JSON-RPC 2.0 on stdin/stdout. rusti
spawns `command` with `args` and nothing else — there is no package manager, runtime,
or ecosystem in that path. Servers you write yourself are the first-class case:

```json
"mcp": {
  "mine":   { "command": "./target/release/my-mcp-server" },
  "pyserv": { "command": "python", "args": ["tools/server.py"] },
  "docker": { "command": "docker", "args": ["run", "-i", "--rm", "ghcr.io/acme/mcp"] }
}
```

`tests/fake_mcp_server.py` in this repo is a complete working server in ~70 lines, and
it is what the client is tested against. Read it if you are writing one.

Third-party servers are a different matter: most published ones happen to ship on npm
or PyPI, so their documented command is `npx -y @scope/server` or `uvx some-server`.
That is *their* packaging, not a rusti requirement — but it does mean reaching for the
off-the-shelf ecosystem pulls Node or Python onto your machine, which this project
otherwise avoids. Prefer installing such a server once and pointing `command` at the
resulting binary: `npx -y` re-resolves the package from the registry on every launch,
so you pay network latency at startup and accept whatever the registry serves that day.

```json
"github": {
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-github"],
  "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_..." },
  "enabled": false
}
```

`/mcp` lists them with their live state and switches them on or off — turning one on
connects it there and then, turning one off kills the child process. The choice is
written back to `model.json`, so it survives a restart.

A server with dozens of tools puts every schema into every request. `"defer": true` on its
entry keeps its tools out until the model asks for them: rusti then declares a `tool_search`
tool naming each deferred server and its tool count, ranks the deferred tools against the
model's query (BM25 over names, descriptions and parameters) and loads the best matches
(5 by default, at most 8) from the next request on. A load is recorded on that tool result in
the session, so it holds after `--resume` and on that `/tree` branch only. Each load changes
the tool list once, which costs one prompt-cache miss.

Transport is stdio with JSON-RPC 2.0, implemented directly against `std::process`
and serde_json — **no MCP SDK and no added dependency**; rusti's own side stays pure
Rust. On Windows the command is run through `cmd /C`, because `npx` and `uvx` are
`.cmd` shims that `CreateProcess` cannot execute directly.

MCP tools are **always permission-gated** and always refused in plan mode. rusti
cannot read a third-party tool's effects off its name, so it treats every one of
them as if it mutates. `[a]lways` works on them like any other tool.

A server that advertises **prompts** gets one `/` command per prompt, `/<server>:<prompt>`,
listed in the menu with its arguments (`<required> [optional]`) and the `mcp:<server>` tag.
Typed words fill the declared arguments in order and the last one takes the rest, so
`/gh:review fix the login bug` gives a single `focus` argument the whole sentence. On Enter
rusti calls `prompts/get` and sends the text it returns as your message.

## extensions

Everything here is a file or a command you write; none of it is a plugin API. rusti reads
`~/.rusti/` and, once the folder is trusted (see "project trust"), `./.rusti/` the same way.
Where both define a name, yours wins, so a cloned repo cannot shadow your commands.

```
~/.rusti/  and  ./.rusti/
  prompts/<name>.md        a prompt template, typed as /<name>
  skills/<name>/SKILL.md   a skill (also ~/.agents/skills and ./.agents/skills)
  themes/<name>.json       a colour palette for /themes
config.json "hooks"        commands run on events (./model.json's need trust)
```

The `/` menu lists the built-ins, then templates, then skills, then MCP prompts, each
tagged with where it came from: `[u]` yours, `[p]` this project's, `[mcp:server]`. A template
named like a built-in is skipped with a warning; `/commit` stays rusti's. A file that does
not load says why in the transcript at startup.

**Prompt templates.** A markdown file is a command. Optional frontmatter sets the menu's
description and argument hint; without a description the first line is used:

```markdown
---
description: Review a file for bugs
argument-hint: <file> [focus]
---
Review $1 for ${2:-correctness bugs}. Read it fully first, then list each problem with
its line number and a suggested fix. Change nothing.
```

Saved as `~/.rusti/prompts/review.md`, `/review src/main.rs "error handling"` sends that text
with `$1` = `src/main.rs` and `$2` = `error handling`. Arguments split like a shell's
(quotes group; `\` escapes a quote, a space or another `\`, and any other
backslash is kept, so `src\tui\app.rs` arrives intact); `$1`…`$9` are positional, `$@` and
`$ARGUMENTS` are all of them, and `${1:-default}` falls back when the argument is missing or
empty. Arguments a template never uses are added after a blank line rather than dropped.
The transcript shows `/review src/main.rs …` as you typed it; the model gets the expansion.

**Skills** are [Agent Skills](https://agentskills.io): a folder with a `SKILL.md` whose
frontmatter has a `name` and a `description`, found up to three folders deep. The system
prompt lists each skill's name, description and absolute path, and tells the model to read
the file when a task matches, so a skill costs one line of context until it is used.
`disable-model-invocation: true` keeps a skill off that list. `/skill:<name> args` sends the
skill's instructions as your message, followed by the arguments. A skill with no
description is skipped with a warning; the model could never tell when to use it.

**Theme files** use rusti's own colour roles and inherit everything else from a `base`:

```json
{"name": "my-theme", "note": "nord, warmer", "base": "nord",
 "colors": {"user": "#88c0d0", "ok": "38;5;144", "think": "3;38;5;103"},
 "tints": {"fail": "#2a1618", "ask": "#2a2414"}}
```

Roles: `user info ok fail warn add del code fenced think dim sel` plus the syntax colours
`key string number boolean`; tints are the `fail` and `ask` row backgrounds. A value is
`#rrggbb` or raw SGR parameters. A theme joins the built-ins in `/themes` with its note; the
active one is re-read about once a second when its file changes, so you can edit it and
watch. A typo in a role or value refuses the file and says which, instead of half applying it.

**Hooks** are commands run on six events, in config order, each with a timeout (default 30
seconds). The event arrives as one JSON object on stdin with `event` and `cwd`; the command
answers with one JSON object on stdout, and nothing or `{}` means carry on. Each hook sees
what the ones before it rewrote.

| event | stdin also has | the reply may set |
|---|---|---|
| `user_shell` | `command`, `to_model` | `command` (run this instead), `output` + `exit` (handled), `block` |
| `pre_tool` | `tool`, `args` | `args` (run with these), `block` |
| `post_tool` | `tool`, `args`, `ok`, `output` | `output` (redact), `ok` |
| `user_prompt` | `prompt` | `prompt` (send this instead), `block` |
| `session_start` | `session`, `resumed` | `context` (added to the session's first message this run) |
| `pre_compact` | `session`, `messages` | `summary` (used instead of asking the model), `block` (skip it) |

```json
"hooks": {
  "user_shell": [{"command": "python ~/.rusti/hooks/remote.py", "timeout": 10}],
  "pre_tool":   [{"command": "~/.rusti/hooks/no-force-push.sh"}]
}
```

`block` is the reason shown, or `true`. `pre_tool` runs after rusti's own permission prompt,
so a hook can refuse a call you allowed but can never allow one you refused; a call a
hook rewrote goes back through the prompt with its new arguments. `user_shell` is
the `!` hook: rewrite `!cargo test` into `ssh box 'cd /srv && cargo test'`, run it yourself
and return the output, or refuse it. A hook that exits non-zero, times out or answers with
something that is not a JSON object blocks what it guards; for `post_tool` the output is
withheld from the model (a redaction that failed must not leak), and for `session_start` it
is only reported. A rewrite is never silent: the transcript says a hook changed the call or
the message. Hooks run in the shell every command runs in, and a sub-agent's tool calls
pass through them too.

## TUI

The transcript marks what needs your eye with a background: your own message (grey), a failed
tool (red) and a tool waiting for your permission (amber). Other tool rows get only a thin
coloured edge: green when done, the spinner colour while running. A run of file reads folds
into one row, the model's narration between tools is dimmed under a rail, and a tool's time
leaves out the time you spent answering its permission prompt. On a light terminal set
`"light": true` in `~/.rusti/config.json` for pale tints instead of dark ones.

The model's thinking is shown in italic grey. Ctrl+T hides it: each finished block becomes one
`thinking… (N lines · ctrl+t)` row, and the block still streaming keeps its last two rows under
that row, so you can see the model working. The choice is saved as `"hide_thinking": true` in
`~/.rusti/config.json`; press Ctrl+T again to show it.

You can type while the agent works. Enter queues a **steer**: it is sent as soon as the current
batch of tool calls finishes, before the next model request, and everything queued goes as one
message. Ctrl+Q queues a **follow-up**: when the task ends successfully, the first one runs as its
own task, then the next. A steer typed just as the task ends, or after an Esc, becomes the first
follow-up. Queued messages show as dim `↳ steer:` / `↳ next:` rows above the input; Alt+Up
(or Alt+Q) puts them all back in the draft, and Esc does the same when it interrupts. Slash and
`!` commands still wait.

Typing `@` at the start of a word opens a file picker over the project (`rg --files`, so
.gitignore is honoured; without ripgrep a walk that skips `.git`, `target`, `node_modules` and
the like). Each character narrows it, ↑/↓ moves, Tab or Enter inserts `@path`, Esc closes it.
A folder inserts `@dir/` and keeps the picker open on what is inside it; a path with a space
becomes `@"path with space"`. The reference stays plain text: the model is told that `@path`
names a file and reads it. The list is built once per draft and rebuilt after each turn.
With no menu open, Tab completes the path before the cursor (`~` is your home folder): one
match goes in outright, several fill what they share and open the list, folders first.

An `@` reference to an image (`.png`, `.jpg`, `.gif`, `.webp`) is attached to that message as an
image part when you send it, shown as `  · attached shot.png (312 KB)`, so the model sees it on
the same request. Only `@` references attach; a path you merely mention stays text, so type `@`
before dropping a file into the terminal. Ctrl+V / Alt+V saves the clipboard's image under
`.rusti/clips/` and types an `@` reference to it; copied image files come in as one reference
each. The same works for a one-shot task: `rusti "@shot.png what is wrong here?"`. An image over
2000 px or 4.5 MB of base64 (`read_file` ones too) is scaled down first, by PowerShell's
System.Drawing on Windows, `sips` on macOS or ImageMagick elsewhere, and the model is told the
original size so it can map coordinates back. A profile with `"vision": false` refuses images
outright (`this model profile has no vision; shot.png not sent`) instead of sending parts a
text-only endpoint would drop without a word.

rusti uses its own custom terminal renderer built with direct ANSI
escape sequences (no ratatui, no heavy TUI framework). this keeps the
binary small (~2.2 MB) and the rendering fast — full-screen redraw with
word wrap, streaming text, and an input box for ask_user questions.

`/reload` rebuilds with the release profile, so it is tuned for the wait you
actually sit through: thin LTO at `codegen-units = 4` with `opt-level = "z"` and
reqwest's unused http2/charset features dropped gets a 2.16 MB binary in ~4.2s
(from 3.27 MB in 23.5s). Fat LTO is 1.87 MB but cannot beat 11.7s at any
`codegen-units`, because it merges the whole program into one module and
optimizes that serially. See the full matrix in `Cargo.toml`.

for non-interactive environments (piped stdin, SSH, CI), it falls back
automatically to plain stdout/stderr streaming.

slash commands: `/model` (pick a saved profile, or `/model <name>` to switch —
same as `/use <name>`), `/resume` (list and switch sessions), `/tree` (browse
the session tree and branch from an entry), `/reload` (see below). Every picker
filters as you type.

## session tree

sessions are stored as a tree of entries with `id`/`parentId` fields,
mirroring pi's session model. the current position is the active leaf.

each session is one file under `.rusti/sessions/`. `--session NAME` (or `/new NAME`)
picks the name; otherwise a launch or `/new` starts `s-YYYYMMDD-HHMM` (local time, `-2`
and up when that minute is taken), so a fresh start never overwrites an older session.
`--resume` and `--tree` without `--session` open the most recent one; a `./session.json`
from older versions is still listed and resumable. `/new` and `/resume` also reset `/undo`,
so it can only put back files the current session changed.

```sh
./target/release/rusti --tree   # browse the tree interactively
```

selection semantics (same as pi's `/tree`):

| pick | leaf moves to | prompt |
|------|---------------|--------|
| user message | entry's parent | text prefilled (edit + resubmit = new branch) |
| assistant message | that entry | empty (continue from there) |

in interactive terminals: ↑/↓ to navigate, Enter to branch, Esc to cancel.
with piped stdin: plain numbered list, type a number and press Enter.

## compaction

When the prompt passes the context limit, rusti summarizes the older part of the session
and carries on from the summary. The limit is `--context` / `RUSTI_CONTEXT` if given, else
the active profile's own `"context"`, else the config's `"context"`, else 100000 tokens. A
profile's `"context"` makes the limit follow the model, including after `/model`:

```json
{"models": [{"name": "big", "url": "...", "model": "kimi-k2.6", "context": 200000},
            {"name": "small", "url": "...", "model": "llama3.2", "context": 24000}]}
```

- **What is kept:** the newest entries up to min(20000, limit/5) estimated tokens (4 chars a
  token), cut only where no tool result is separated from its call. The rest becomes one
  summary entry; the old entries stay in the session tree.
- **The summary** has fixed sections (Goal, Constraints & Preferences, Progress with Done /
  In Progress / Blocked, Key Decisions, Next Steps, Critical Context) followed by
  `<read-files>` and `<modified-files>` lists taken from the tool calls. A later compaction
  updates that summary and carries the file lists forward instead of starting over.
- **When:** before a request once the last prompt (or, at the start of a turn, the
  estimate) passes the limit; once on a provider's context-overflow error, after which the
  request is sent again; and on `/compact [focus]` in the TUI (`compact` over RPC), where
  `focus` tells the summary what to keep.
- The summary shows as one `⟳ compacted` row; Ctrl+O shows its text.

## JSON and RPC modes

Two modes let another program run rusti without scraping text. stdout carries only JSON
lines (one object per LF-terminated line, every object with `"v": 1` and a `"type"`);
everything for humans goes to stderr. `"v"` changes only when a field changes meaning or
goes away; new fields and new types can appear in v1, so ignore what you do not know.

**`rusti --json [flags] TASK`** runs one task. The first line is `session`, then the run's
events, then a last `task_end` that adds `"answer"` (the final reply). Exit code 0 or 1.
Questions cannot be answered here: a permission prompt is refused unless `--yolo`, and
`ask_user` gets `"no answer given"`; the `ask` line still shows what was asked. Like any
one-shot run, the folder is untrusted unless `--trust`.

```sh
rusti --json "run the tests and fix what fails" | jq -c 'select(.type=="tool_end")'
```

**Events** (both modes):

| type | fields |
|------|--------|
| `session` | `name`, `path`, `model`, `messages` (user messages on the path), `cwd`, `thinking` |
| `text_delta` / `thinking_delta` | `text`: a streamed chunk of the reply / of the model's reasoning |
| `text` | `text`: a whole transcript line (attachments, todo lists, warnings) |
| `tool_start` | `id`, `summary`, e.g. `"read src/main.rs"` |
| `tool_end` | `id`, `summary`, `ok`, `ms`, `output` (the tool's result) |
| `usage` | `tokens`, `prompt`, `estimated`, `gen_ms`, `cached` (null when not reported) |
| `retry` | `attempt`, `of`, `wait_ms`, `error` |
| `ask` | `id`, `question`, `choices`: `[{label, value}]`, empty for a free-text question |
| `steer_delivered` | `text`: queued steer text that just joined the conversation |
| `compaction_start` | `reason` (`threshold`, `overflow`, `manual`), `messages`, `tokens` |
| `compaction_end` | `summary`, `tokens` (estimated, after) |
| `notice`, `git`, `session_name`, `resumed` | status: `text`, `branch`, `name`, `messages` |
| `task_end` | `ok`, `error` (`"interrupted"` after an abort); `--json` adds `answer` |

Tool calls can run in parallel, so a `tool_end` belongs to the `tool_start` with the same `id`.

**`rusti --rpc [flags]`** stays up and reads one command per stdin line. It writes the
`session` line at start, then a `response` for every command and the same events as
`--json`. A command may carry any JSON `"id"`; its response echoes it:
`{"v":1,"type":"response","id":7,"command":"prompt","ok":true,"data":{...}}`, or
`"ok":false` with `"error"`. Match responses by id, not by position: events a command
causes can arrive before or after its response. Closing stdin lets the running task and
the queue finish, refuses any question still open, and exits 0.

| command | fields | data / effect |
|---------|--------|---------------|
| `prompt` | `text`, `mode` | idle: runs it (`accepted: "run"`), and `task_end` marks the end. Busy: `mode` must be `steer` (joins the running task after its current tool calls) or `follow_up` (runs as its own task afterwards, one at a time); `@path` images attach as in the TUI |
| `abort` | | cancels the running task, drops queued steers and follow-ups, answers open questions no; `dropped` |
| `answer` | `ask`, `text` | answers the `ask` line with that `id`; for a permission prompt send one of its choice values (`yes`, `no`, `always`) |
| `new_session` | `name`? | saves this session, starts an empty one (auto name if none); `name` |
| `resume_session` | `name`? | switches to that saved session, or the latest; `name`, `messages` |
| `set_model` | `name` | switches to a saved profile, its vision, `cache` and `context` included; `model`, `context_limit` |
| `set_thinking` | `level`? | sends `reasoning_effort: level` (e.g. `low`, `high`) from the next request; none clears it |
| `set_plan` | `on` | plan mode on or off (writes and commands refused) |
| `compact` | `instructions`? | compacts now, focused on the instructions; `compacted`: false when it all fits |
| `get_state` | | `busy`, `session`, `thinking`, `plan`, `context_limit`, `steers`, `follow_ups`, `asks` (open ask ids) |
| `get_messages` | | `messages`: the active path in chat-completions form, as last saved |

`new_session`, `resume_session`, `set_model`, `set_thinking` and `compact` need the agent
idle and fail with an error while a task runs, and their response marks the end; the session-changing ones are followed by a
fresh `session` line. Permission prompts and `ask_user` become `ask` lines that wait for an
`answer` (`--yolo` skips permission prompts as anywhere else).

```
→ {"id":1,"type":"prompt","text":"add a --version flag"}
← {"v":1,"type":"response","id":1,"command":"prompt","ok":true,"data":{"accepted":"run"}}
← {"v":1,"type":"tool_start","id":1,"summary":"edit src/main.rs"}
← {"v":1,"type":"ask","id":"q1","question":"allow edit_file edit src/main.rs?","choices":[...]}
→ {"id":2,"type":"answer","ask":"q1","text":"yes"}
← {"v":1,"type":"response","id":2,"command":"answer","ok":true,"data":{}}
← {"v":1,"type":"tool_end","id":1,"summary":"edit src/main.rs  +3 -0","ok":true,"ms":2,"output":"..."}
← {"v":1,"type":"task_end","ok":true,"error":null}
```

## /reload

Inside the TUI, `/reload` rebuilds rusti from its own source and relaunches
in place — for developing rusti with rusti. It preserves the session
(`--resume`), the active model (even if switched mid-session with `/use`),
and the visible transcript. Only runs when idle (finish or Esc-interrupt the
current turn first); a build failure is reported inline and leaves the
running session untouched.
