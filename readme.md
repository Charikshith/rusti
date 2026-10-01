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
./target/release/rusti --resume          # continue from active leaf

# flags/env still override the saved profile
./target/release/rusti --max-iters 100 "task"   # tool rounds per task (default 50, env RUSTI_MAX_ITERS)
./target/release/rusti --context 60000 "task"   # compact history once the prompt exceeds N tokens (default 100000, env RUSTI_CONTEXT)
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
sees on the next turn, and `!!git log` runs one whose output only you see. `/resume` with no argument lists saved sessions and switches to the
one you pick; with a name or number it switches straight to it. The status line carries
plan mode, the session, model, git branch, tokens used, how full the context is and, when
the provider reports it, how much of the last prompt came from its cache (`cache 82%`).
The system prompt holds only what rarely changes; git status and plan mode travel on each
user message instead, so a provider's prompt cache can reuse everything sent before.

`model.json` example:
```json
{"default": "ollama", "models": [{"name": "ollama", "url": "http://localhost:11434/v1/chat/completions", "key": "", "model": "llama3.2"}]}
```

It doubles as the project config. Answering `[a]lways` at a permission prompt appends the
tool to `"allow"`, so the next run doesn't ask again; `"max_iters"` and `"context"` set
per-project defaults that flags and env still override:
```json
{"allow": ["read_file", "run_command"], "max_iters": 100, "context": 60000}
```

## architecture

```
main.rs      CLI
├── config   model.json profiles
├── session  tree-shaped session persistence (session.json)
├── tree     interactive session tree browser (ANSI TUI / plain list)
└── ai_core  agent loop, LLM client, tool dispatch, event system
    ├── llm    reqwest SSE streaming client
    └── tools  read/write/edit/multi_edit/delete/move file, grep/glob/list_dir, run command (timeout),
    │          background jobs, todo, delegate (sub-agent), ask user, web_fetch;
    │          permission gate + project-root guard

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
| ai_core/mcp.rs | ~410 | MCP client: stdio JSON-RPC 2.0, spawns servers, merges their tools in as `mcp__<server>__<tool>`, `/mcp` toggles them |
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
`prompts`, `skills`, `themes` or `SYSTEM.md`), the TUI asks once at launch:

```
This folder's model.json sets mcp, shell, allow. Trust it? [y]es / [N]o / [a]lways
```

`y` trusts it for this run, `a` also saves it by canonical path in `~/.rusti/trust.json`.
Untrusted, those keys are ignored with one warning line (and saved back untouched);
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

Transport is stdio with JSON-RPC 2.0, implemented directly against `std::process`
and serde_json — **no MCP SDK and no added dependency**; rusti's own side stays pure
Rust. On Windows the command is run through `cmd /C`, because `npx` and `uvx` are
`.cmd` shims that `CreateProcess` cannot execute directly.

MCP tools are **always permission-gated** and always refused in plan mode. rusti
cannot read a third-party tool's effects off its name, so it treats every one of
them as if it mutates. `[a]lways` works on them like any other tool.

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

## /reload

Inside the TUI, `/reload` rebuilds rusti from its own source and relaunches
in place — for developing rusti with rusti. It preserves the session
(`--resume`), the active model (even if switched mid-session with `/use`),
and the visible transcript. Only runs when idle (finish or Esc-interrupt the
current turn first); a build failure is reported inline and leaves the
running session untouched.
