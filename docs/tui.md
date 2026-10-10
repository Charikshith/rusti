# TUI

Back to the [readme](../readme.md). `rusti --help` lists every slash command and key.

Bare `rusti` in a terminal opens the TUI; `--tui` forces it. A task argument or piped stdin is
a one-shot run. For non-interactive environments (piped stdin, SSH, CI) it falls back
automatically to plain stdout/stderr streaming.

rusti draws its own terminal UI with direct ANSI escape sequences (no ratatui, no heavy TUI
framework). That keeps the binary small (~2.2 MB) and the rendering fast: only changed lines are redrawn,
with word wrap, streaming text, and an input box for `ask_user` questions.

## slash commands

- `/model` picks a saved profile, or `/model <name>` switches (same as `/use <name>`).
- `/new [name]`, `/resume`, `/rename`, `/tree` manage sessions ([sessions.md](sessions.md)).
- `/plan` toggles plan mode: the agent reads and proposes but every write and command is
  refused; leaving it returns to manual ([safety.md](safety.md)).
- `/undo` puts back the files the last turn changed; `/commit` stages and commits the work;
  `/export` writes the transcript out as markdown.
- `/compact [focus]` summarizes the older history now ([sessions.md § compaction](sessions.md#compaction)).
- `/settings` chooses which status-line segments are shown; `/themes` picks the colour palette;
  `/mcp` switches MCP servers on and off.
- `/reload` rebuilds and relaunches (below); `/quit` exits.
- `!cargo test` runs a shell command whose output the model sees on the next turn; `!!git log`
  runs one whose output only you see ([configuration.md § shell commands](configuration.md#shell-commands)).
- Your own prompt templates, skills and MCP prompts join the `/` menu too
  ([extensions.md](extensions.md)).

Every picker filters as you type.

## status line

The status line carries the permission mode (`plan`, `auto`, or `DEVIL` in the warning
colour; manual shows nothing), the session, model, git branch, tokens used, how full the
context is and, when the provider reports it, how much of the last prompt came from its cache
(`cache 82%`). `/settings` hides or shows each part.

## transcript

The transcript marks what needs your eye with a background: your own message (grey), a failed
tool (red) and a tool waiting for your permission (amber). Other tool rows get only a thin
coloured edge: green when done, the spinner colour while running. A run of file reads folds
into one row, the model's narration between tools is dimmed under a rail, and a tool's time
leaves out the time you spent answering its permission prompt. On a light terminal set
`"light": true` in `~/.rusti/config.json` for pale tints instead of dark ones.

Model prose is rendered, not printed raw: ATX headings and `**bold**` come out bold, inline
code in cyan, `-`/`*` bullets as bullet glyphs, and fenced blocks in their own colour with the
fence lines hidden. No parser crate: markers become style runs over the visible text, so
wrapping still measures real columns. Fenced bodies are truncated rather than reflowed.

## thinking

The model's thinking is shown in italic grey. Ctrl+T hides it: each finished block becomes one
`thinking… (N lines · ctrl+t)` row, and the block still streaming keeps its last two rows under
that row, so you can see the model working. The choice is saved as `"hide_thinking": true` in
`~/.rusti/config.json`; press Ctrl+T again to show it.

## steering and follow-ups

You can type while the agent works.

- Enter queues a **steer**: it is sent as soon as the current batch of tool calls finishes,
  before the next model request, and everything queued goes as one message.
- Ctrl+Q queues a **follow-up**: when the task ends successfully, the first one runs as its own
  task, then the next. A steer typed just as the task ends, or after an Esc, becomes the first
  follow-up.
- Queued messages show as dim `↳ steer:` / `↳ next:` rows above the input; Alt+Up (or Alt+Q)
  puts them all back in the draft, and Esc does the same when it interrupts.
- Slash and `!` commands still wait.

Shift+Enter adds a newline to the draft (Alt+Enter where the terminal eats Shift).

## @ file references

Typing `@` at the start of a word opens a file picker over the project (`rg --files`, so
.gitignore is honoured; without ripgrep a walk that skips `.git`, `target`, `node_modules` and
the like). Each character narrows it, ↑/↓ moves, Tab or Enter inserts `@path`, Esc closes it.
A folder inserts `@dir/` and keeps the picker open on what is inside it; a path with a space
becomes `@"path with space"`. The reference stays plain text: the model is told that `@path`
names a file and reads it. The list is built once per draft and rebuilt after each turn.

With no menu open, Tab completes the path before the cursor (`~` is your home folder): one
match goes in outright, several fill what they share and open the list, folders first.

## images

An `@` reference to an image (`.png`, `.jpg`, `.gif`, `.webp`) is attached to that message as
an image part when you send it, shown as `  · attached shot.png (312 KB)`, so the model sees it
on the same request. Only `@` references attach; a path you merely mention stays text, so type
`@` before dropping a file into the terminal.

- Ctrl+V / Alt+V saves the clipboard's image under `.rusti/clips/` and types an `@` reference
  to it; copied image files come in as one reference each.
- The same works for a one-shot task: `rusti "@shot.png what is wrong here?"`.
- An image over 2000 px or 4.5 MB of base64 (`read_file` ones too) is scaled down first, by
  PowerShell's System.Drawing on Windows, `sips` on macOS or ImageMagick elsewhere, and the
  model is told the original size so it can map coordinates back.
- A profile with `"vision": false` refuses images outright (`this model profile has no vision;
  shot.png not sent`) instead of sending parts a text-only endpoint would drop without a word.

## /reload

Inside the TUI, `/reload` rebuilds rusti from its own source and relaunches in place, for
developing rusti with rusti. It preserves the session (`--resume`), the active model (even if
switched mid-session with `/use`), and the visible transcript. It only runs when idle (finish
or Esc-interrupt the current turn first); a build failure is reported inline and leaves the
running session untouched. An installed binary has no rusti source on the machine, so
there `/reload` skips the build and restarts the same executable with the same session, model
and flags, which re-reads `config.json`, models, themes and extensions. How the release build is tuned for that wait is in
[architecture.md § build profile](architecture.md#build-profile).
