# Configuration

Back to the [readme](../readme.md).

## config files

| file | scope | holds |
|---|---|---|
| `~/.rusti/config.json` | every project | models, keys, theme, footer, `"light"`, `"hide_thinking"`, `"shell"`, `"hooks"`, MCP servers |
| `./model.json` | this project | `"allow"`, `"max_iters"`, `"context"`; any other key here overrides the global one for this project |

`RUSTI_HOME` moves `~/.rusti`. Flags and env override both files. Keys in `./model.json`
that run code or steer permissions only apply once the folder is trusted
([safety.md § project trust](safety.md#project-trust)). The first launch asks for a model.

A model profile:

```json
{"default": "ollama", "models": [{"name": "ollama", "url": "http://localhost:11434/v1/chat/completions", "key": "", "model": "llama3.2"}]}
```

Manage profiles with `rusti --list` (`*` = default), `rusti --use <name>` (switch default) and
`rusti --add` (add one interactively); `/model` and `/use` switch in the TUI.
`--url U --key K --model M` override the saved profile for one run.

`model.json` doubles as the project config. Answering `[a]lways` at a permission prompt
appends the tool to `"allow"`, so the next run doesn't ask again; `"max_iters"` and
`"context"` set per-project defaults that flags and env still override:

```json
{"allow": ["read_file", "run_command"], "max_iters": 100, "context": 60000}
```

`--max-iters N` / `RUSTI_MAX_ITERS` sets the tool rounds per task (default 50).
`--context N` / `RUSTI_CONTEXT` sets the compaction limit (default 100000 tokens; see
[sessions.md § compaction](sessions.md#compaction)).

## prompt-cache markers

`"cache"` on a profile adds explicit prompt-cache markers, off by default because a server
that does not know the fields may reject the request.

- `"anthropic"` (for Anthropic-format endpoints such as OpenRouter's `anthropic/*`) puts
  `cache_control` on the system prompt, the last tool and the newest message.
- `"openai"` sends a `prompt_cache_key` derived from the session file, so one session's
  requests share a cache across restarts.

The system prompt holds only what rarely changes; git status and plan mode travel on each
user message instead, so a provider's prompt cache can reuse everything sent before. The
status line shows how much of the last prompt came from the cache (`cache 82%`) when the
provider reports it.

## shell commands

The model's `run_command` and `run_background` and your own `!cmd` / `!!cmd` all run in one
shell, chosen once at startup:

1. the `"shell"` path in `~/.rusti/config.json` (a leading `~` is your home folder);
2. on Windows, Git Bash under Program Files or Program Files (x86);
3. `bash` on PATH;
4. `cmd /C` on Windows, `sh -c` elsewhere.

Bash runs as `bash -c <cmd>`. The system prompt tells the model which shell that is.
`"shell_command_prefix"` is put in front of every command, on its own line (joined with `&`
under cmd):

```json
{"shell": "~/scoop/apps/git/current/bin/bash.exe", "shell_command_prefix": "shopt -s expand_aliases"}
```

`!cmd` and `!!cmd` stream their output into the transcript as it arrives, and Esc kills them.
`!cmd`'s output is then added to the conversation for the model; `!!cmd`'s never is.

## project instructions

rusti's own docs (the readme, these `docs/` files and `--help`) are compiled into the binary
and written to `~/.rusti/docs/`; the system prompt carries only their paths and a topic map,
so asking rusti "how do I change the theme?" makes it read them instead of guessing.

If the working directory has an `AGENTS.md` (fallback `RUSTI.md`, then `CLAUDE.md`), its
contents are appended to the system prompt. It is re-read every turn, so edits take effect on
the next message without a restart.

`SYSTEM.md` replaces rusti's built-in base prompt, and `APPEND_SYSTEM.md` is added after it.
Both are looked for in `~/.rusti/` and, once the folder is trusted, in `./.rusti/`:

- the project's `SYSTEM.md` wins over the global one;
- both `APPEND_SYSTEM.md` files apply, global first, then project;
- `--system-prompt X` and `--append-system-prompt X` (repeatable) win over those files; X is
  read as a file when one exists at that path, else used as the text itself.

Only the base text is replaced: the shell line, the docs pointer, the project instructions
above, git context and the plan-mode block always stay. The files are re-read every turn, and
startup prints `ℹ system prompt: …` naming whatever changed it (for example
`ℹ system prompt: .rusti/SYSTEM.md`; an unreadable file is skipped and named there too).
`/reload` keeps the flags.
