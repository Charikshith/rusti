# Extensions

Back to the [readme](../readme.md).

Everything here is a file or a command you write; none of it is a plugin API. rusti reads
`~/.rusti/` and, once the folder is trusted ([safety.md § project trust](safety.md#project-trust)),
`./.rusti/` the same way. Where both define a name, yours wins, so a cloned repo cannot
shadow your commands.

```
~/.rusti/  and  ./.rusti/
  prompts/<name>.md        a prompt template, typed as /<name>
  skills/<name>/SKILL.md   a skill (also ~/.agents/skills and ./.agents/skills)
  themes/<name>.json       a colour palette for /themes
config.json "hooks"        commands run on events (./model.json's need trust)
config.json "mcp"          MCP servers (./model.json's need trust)
```

The `/` menu lists the built-ins, then templates, then skills, then MCP prompts, each tagged
with where it came from: `[u]` yours, `[p]` this project's, `[mcp:server]`. A template named
like a built-in is skipped with a warning; `/commit` stays rusti's. A file that does not load
says why in the transcript at startup.

## prompt templates

A markdown file is a command. Optional frontmatter sets the menu's description and argument
hint; without a description the first line is used:

```markdown
---
description: Review a file for bugs
argument-hint: <file> [focus]
---
Review $1 for ${2:-correctness bugs}. Read it fully first, then list each problem with
its line number and a suggested fix. Change nothing.
```

Saved as `~/.rusti/prompts/review.md`, `/review src/main.rs "error handling"` sends that text
with `$1` = `src/main.rs` and `$2` = `error handling`.

- Arguments split like a shell's: quotes group; `\` escapes a quote, a space or another `\`,
  and any other backslash is kept, so `src\tui\app.rs` arrives intact.
- `$1`…`$9` are positional, `$@` and `$ARGUMENTS` are all of them, and `${1:-default}` falls
  back when the argument is missing or empty.
- Arguments a template never uses are added after a blank line rather than dropped.
- The transcript shows `/review src/main.rs …` as you typed it; the model gets the expansion.

## skills

Skills are [Agent Skills](https://agentskills.io): a folder with a `SKILL.md` whose frontmatter
has a `name` and a `description`, found up to three folders deep. The system prompt lists each
skill's name, description and absolute path, and tells the model to read the file when a task
matches, so a skill costs one line of context until it is used.

- `disable-model-invocation: true` keeps a skill off that list.
- `/skill:<name> args` sends the skill's instructions as your message, followed by the arguments.
- A skill with no description is skipped with a warning; the model could never tell when to use it.

## theme files

Theme files use rusti's own colour roles and inherit everything else from a `base`:

```json
{"name": "my-theme", "note": "nord, warmer", "base": "nord",
 "colors": {"user": "#88c0d0", "ok": "38;5;144", "think": "3;38;5;103"},
 "tints": {"fail": "#2a1618", "ask": "#2a2414"}}
```

Roles: `user info ok fail warn add del code fenced think dim sel` plus the syntax colours
`key string number boolean`; tints are the `fail` and `ask` row backgrounds. A value is
`#rrggbb` or raw SGR parameters. A theme joins the built-ins in `/themes` with its note; the
active one is re-read about once a second when its file changes, so you can edit it and watch.
A typo in a role or value refuses the file and says which, instead of half applying it.

## hooks

Hooks are commands run on six events, in config order, each with a timeout (default 30
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

- `block` is the reason shown, or `true`.
- `pre_tool` runs after rusti's own permission prompt, so a hook can refuse a call you allowed
  but can never allow one you refused; a call a hook rewrote goes back through the prompt with
  its new arguments.
- `user_shell` is the `!` hook: rewrite `!cargo test` into `ssh box 'cd /srv && cargo test'`,
  run it yourself and return the output, or refuse it.
- A hook that exits non-zero, times out or answers with something that is not a JSON object
  blocks what it guards; for `post_tool` the output is withheld from the model (a redaction
  that failed must not leak), and for `session_start` it is only reported.
- A rewrite is never silent: the transcript says a hook changed the call or the message.
- Hooks run in the shell every command runs in, and a sub-agent's tool calls pass through them too.

## MCP servers

An MCP server is **any executable** that speaks JSON-RPC 2.0 on stdin/stdout. rusti spawns
`command` with `args` and nothing else; there is no package manager, runtime or ecosystem in
that path. Servers you write yourself are the first-class case:

```json
"mcp": {
  "mine":   { "command": "./target/release/my-mcp-server" },
  "pyserv": { "command": "python", "args": ["tools/server.py"] },
  "docker": { "command": "docker", "args": ["run", "-i", "--rm", "ghcr.io/acme/mcp"] }
}
```

`tests/fake_mcp_server.py` in this repo is a complete working server in ~70 lines, and it is
what the client is tested against. Read it if you are writing one.

Most published third-party servers ship on npm or PyPI, so their documented command is
`npx -y @scope/server` or `uvx some-server`. That is *their* packaging, not a rusti
requirement, but it pulls Node or Python onto your machine, which this project otherwise
avoids. Prefer installing such a server once and pointing `command` at the resulting binary:
`npx -y` re-resolves the package from the registry on every launch, so you pay network latency
at startup and accept whatever the registry serves that day.

```json
"github": {
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-github"],
  "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_..." },
  "enabled": false
}
```

Their tools join the built-in ones as `mcp__<server>__<tool>`. `/mcp` lists the servers with
their live state and switches them on or off: turning one on connects it there and then,
turning one off kills the child process. The choice is written back to `model.json`, so it
survives a restart.

**Deferred tools.** A server with dozens of tools puts every schema into every request.
`"defer": true` on its entry keeps its tools out until the model asks for them: rusti then
declares a `tool_search` tool naming each deferred server and its tool count, ranks the
deferred tools against the model's query (BM25 over names, descriptions and parameters) and
loads the best matches (5 by default, at most 8) from the next request on. A load is recorded
on that tool result in the session, so it holds after `--resume` and on that `/tree` branch
only. Each load changes the tool list once, which costs one prompt-cache miss.

**Transport** is stdio with JSON-RPC 2.0, implemented directly against `std::process` and
serde_json: **no MCP SDK and no added dependency**; rusti's own side stays pure Rust. On
Windows the command is run through `cmd /C`, because `npx` and `uvx` are `.cmd` shims that
`CreateProcess` cannot execute directly.

**Permissions.** MCP tools are always permission-gated and always refused in plan mode. rusti
cannot read a third-party tool's effects off its name, so it treats every one of them as if it
mutates. `[a]lways` works on them like any other tool.

**MCP prompts.** A server that advertises prompts gets one `/` command per prompt,
`/<server>:<prompt>`, listed in the menu with its arguments (`<required> [optional]`) and the
`mcp:<server>` tag. Typed words fill the declared arguments in order and the last one takes
the rest, so `/gh:review fix the login bug` gives a single `focus` argument the whole sentence.
On Enter rusti calls `prompts/get` and sends the text it returns as your message.
