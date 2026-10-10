# rusti

Minimal coding agent in Rust: async core (tokio + reqwest), custom ANSI
TUI (no ratatui), ~2.2 MB release binary.
<!-- How it compares with the Pi coding agent: [docs/rusti-vs-pi.md](docs/rusti-vs-pi.md). -->

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
From source instead: `cargo install --git https://github.com/Charikshith/rusti`, or
`cargo build --release` in a checkout when working on rusti itself.

## Use

```sh
rusti                                   # the TUI; the first launch asks for a model
rusti "add a --version flag to src/main.rs"   # one-shot: run the task, print the answer
rusti --list | --use <name> | --add     # show, switch or add saved model profiles
rusti --resume                          # continue the latest session
rusti --tree                            # browse the session tree and branch
rusti --yolo "task"                     # devil mode: no prompts, no project-root guard (RUSTI_YOLO=1)
rusti --trust "task"                    # trust this folder's model.json / .rusti without asking
rusti --json "task"                     # the run as JSON lines, for programs; --rpc to drive it
rusti --self-test                       # offline check against a fake server
rusti --help                            # every flag, slash command and key
```

A task argument or piped stdin is a one-shot run; bare `rusti` in a terminal opens the TUI.
`--max-iters N` (default 50) and `--context N` (compaction limit, default 100000 tokens)
override the saved settings, as do their env vars `RUSTI_MAX_ITERS` and `RUSTI_CONTEXT`.

## Essential keys and commands

| in the TUI | does |
|---|---|
| Shift+Tab | cycle the permission mode (below); the status line shows it |
| Enter while working | steer: sent after the current tool calls |
| Ctrl+Q | follow-up: runs as its own task when this one ends |
| Esc | interrupt the running turn |
| Ctrl+T | hide or show the model's thinking |
| `@path` | name a project file (picker opens); images attach |
| `!cmd` / `!!cmd` | run a shell command; output goes to the model / to you only |
| `/model`, `/new`, `/resume`, `/tree` | switch model, new session, switch session, branch |
| `/undo`, `/commit`, `/export` | revert the last turn's files, commit the work, save the transcript |
| `/plan`, `/compact`, `/mcp`, `/themes`, `/settings` | plan mode, compact now, MCP servers, palette, status line |
| `/reload` | rebuild rusti from source and relaunch in place; an installed binary just restarts to reload config |

Your own prompt templates, skills and MCP prompts join the `/` menu too.

## Permission modes

Tools that change files, run commands or reach the network ask first:
`[y]es / [n]o / [a]lways` (always is saved to `model.json`). Shift+Tab cycles:

| mode | what happens |
|---|---|
| manual | the default: every such tool asks |
| plan | reads and searches only; every write and command is refused (also `/plan`) |
| auto | edits inside the project root and non-risky commands run without asking; deletes, `web_fetch`, MCP tools and risky commands (`rm`, `git push`, installs, writes outside the root, …) still ask |
| devil | no prompts and no project-root guard; same as `--yolo` |

Outside devil mode, writes outside the working directory are refused. Piped and one-shot runs
deny every prompt unless `--yolo`. A cloned repo's `model.json` / `.rusti/` keys that run
code or steer permissions are ignored until you trust the folder. Details:
[docs/safety.md](docs/safety.md).

## Config

| file | holds |
|---|---|
| `~/.rusti/config.json` | models, keys, theme, footer, shell, hooks and MCP servers for every project (`RUSTI_HOME` moves `~/.rusti`) |
| `./model.json` | this project's `"allow"` list, `"max_iters"` and `"context"`; any other key overrides the global one here |
| `AGENTS.md` (or `RUSTI.md`, `CLAUDE.md`) | project instructions appended to the system prompt, re-read every turn |
| `~/.rusti/` and `./.rusti/` | `prompts/`, `skills/`, `themes/`, `SYSTEM.md`, `APPEND_SYSTEM.md` |

Flags and env override both config files. A model profile:

```json
{"default": "ollama", "models": [{"name": "ollama", "url": "http://localhost:11434/v1/chat/completions", "key": "", "model": "llama3.2"}]}
```

## More docs

rusti's docs are compiled into the binary and written to `~/.rusti/docs/`, so you can ask
rusti about itself ("how do I change the theme?") and it reads them.

- [docs/configuration.md](docs/configuration.md): config files, prompt-cache markers, shell commands, project instructions and system prompts
- [docs/safety.md](docs/safety.md): permission prompts and modes, the risky-command list, project-root guard, project trust
- [docs/tools.md](docs/tools.md): the built-in tools, read limits, parallel calls and `batch`, sub-agents and background jobs
- [docs/tui.md](docs/tui.md): slash commands, status line, transcript, steering, `@` references, images, `/reload`
- [docs/sessions.md](docs/sessions.md): the session tree, `/new`, `/resume`, `/tree`, `/undo`, compaction
- [docs/extensions.md](docs/extensions.md): prompt templates, skills, themes, hooks, MCP servers
- [docs/json-rpc.md](docs/json-rpc.md): `--json` and `--rpc` line formats
- [docs/architecture.md](docs/architecture.md): modules, file map, build profile, releases
