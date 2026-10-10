# Architecture

Back to the [readme](../readme.md).

## modules

Async core (tokio + reqwest), custom ANSI TUI (no ratatui).

```
main.rs      CLI
├── rpc      --json event lines and the --rpc command loop
├── config   model.json profiles
├── session  tree-shaped session persistence (.rusti/sessions/NAME.json)
├── tree     interactive session tree browser (ANSI TUI / plain list)
├── resources prompt templates and skills
└── ai_core  agent loop, LLM client, tool dispatch, event system
    ├── llm    reqwest SSE streaming client
    ├── tools  read/write/edit/multi_edit/delete/move file, grep/glob/list_dir, run command (timeout),
    │          background jobs, todo, delegate (sub-agent), ask user, web_fetch, batch;
    │          permission gate + project-root guard; parallel-safe calls run concurrently
    ├── hooks  user commands on events
    └── mcp    stdio MCP client; deferred servers + tool_search (BM25)

mod tui     custom ANSI TUI (no ratatui)
```

## file map

| file | responsibility |
|------|----------------|
| main.rs | CLI entry, flags, `--help` text, config resolution, session/task dispatch |
| config.rs | model.json load/save/resolve, interactive profile creation |
| session.rs | session tree: entries with id/parentId, active leaf, path, select, save/load |
| tree.rs | session tree browser: ANSI TUI (interactive) or plain numbered list (piped) |
| rpc.rs | `--json` event lines and the `--rpc` command loop |
| resources.rs | prompt templates and skills: discovery, frontmatter, argument expansion |
| tui/mod.rs, app.rs | TUI app state and event loop on the alternate screen |
| tui/render.rs | differential renderer: only changed lines are redrawn |
| tui/theme.rs | colour palettes for `/themes` |
| ai_core/mod.rs | run_agent loop, event system (SINK), tool dispatch, the embedded docs, self-test |
| ai_core/llm.rs | reqwest SSE streaming client, tool-call argument accumulation |
| ai_core/tools.rs | the built-in tools ([tools.md](tools.md)); project-root guard |
| ai_core/mcp.rs | MCP client: stdio JSON-RPC 2.0, spawns servers, merges their tools in as `mcp__<server>__<tool>` and their prompts in as `/<server>:<prompt>`, `/mcp` toggles them |
| ai_core/hooks.rs | hooks: user commands on six events, JSON in on stdin, a JSON decision out on stdout |

## build profile

`/reload` rebuilds with the release profile, so it is tuned for the wait you actually sit
through: thin LTO at `codegen-units = 4` with `opt-level = "z"` and reqwest's unused
http2/charset features dropped gets a 2.16 MB binary in ~4.2s (from 3.27 MB in 23.5s). Fat
LTO is 1.87 MB but cannot beat 11.7s at any `codegen-units`, because it merges the whole
program into one module and optimizes that serially. The full matrix is in `Cargo.toml`.

## releases

Push a `v*` tag and `.github/workflows/release.yml` builds and attaches the binaries.
