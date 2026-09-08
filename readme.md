# rustypi

Minimal coding agent in Rust: async core (tokio + reqwest), custom ANSI
TUI (no ratatui), ~2.6 MB release binary.

```sh
cargo build --release

# model.json (in cwd) persists model details. First launch asks interactively:
./target/release/rustypi "add a --version flag to src/main.rs"

# manage saved models
./target/release/rustypi --list            # show saved profiles (* = default)
./target/release/rustypi --use <name>      # switch default
./target/release/rustypi --add             # add another profile interactively

# TUI mode (custom ANSI renderer, streaming transcript, Esc quits)
# auto-falls back to plain stream when stdin is piped
./target/release/rustypi --tui "your task"

# session tree (pi-style branching)
./target/release/rustypi --tree            # browse + branch from earlier entries
./target/release/rustypi --resume          # continue from active leaf

# flags/env still override the saved profile
./target/release/rustypi --self-test       # offline check, fake server
```

`model.json` example:
```json
{"default": "ollama", "models": [{"name": "ollama", "url": "http://localhost:11434/v1/chat/completions", "key": "", "model": "llama3.2"}]}
```

## architecture

```
main.rs      CLI
├── config   model.json profiles
├── session  tree-shaped session persistence (session.json)
├── tree     interactive session tree browser (ANSI TUI / plain list)
└── ai_core  agent loop, LLM client, tool dispatch, event system
    ├── llm    reqwest SSE streaming client
    └── tools  read/write/edit file, run command, ask user

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
| ai_core/tools.rs | ~85 | 5 tools: read/write/edit file, run command, ask user |

**Total application code: ~1,015 lines**

## TUI

rustypi uses its own custom terminal renderer built with direct ANSI
escape sequences (no ratatui, no heavy TUI framework). this keeps the
binary small (~2.6 MB) and the rendering fast — full-screen redraw with
word wrap, streaming text, and an input box for ask_user questions.

for non-interactive environments (piped stdin, SSH, CI), it falls back
automatically to plain stdout/stderr streaming.

slash commands: `/model` (list saved profiles, or `/model <name>` to switch —
same as `/use <name>`), `/resume` (reload session.json), `/tree` (dump the
session path), `/reload` (see below).

## session tree

sessions are stored as a tree of entries with `id`/`parentId` fields,
mirroring pi's session model. the current position is the active leaf.

```sh
./target/release/rustypi --tree   # browse the tree interactively
```

selection semantics (same as pi's `/tree`):

| pick | leaf moves to | prompt |
|------|---------------|--------|
| user message | entry's parent | text prefilled (edit + resubmit = new branch) |
| assistant message | that entry | empty (continue from there) |

in interactive terminals: ↑/↓ to navigate, Enter to branch, Esc to cancel.
with piped stdin: plain numbered list, type a number and press Enter.

## /reload

Inside the TUI, `/reload` rebuilds rustypi from its own source and relaunches
in place — for developing rustypi with rustypi. It preserves the session
(`--resume`), the active model (even if switched mid-session with `/use`),
and the visible transcript. Only runs when idle (finish or Esc-interrupt the
current turn first); a build failure is reported inline and leaves the
running session untouched.
