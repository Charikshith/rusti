# TUI

Back to the [readme](../readme.md). `rusti --help` lists every slash command and key.

Bare `rusti` in a terminal opens the TUI; `--tui` forces it. A task argument or piped stdin is
a one-shot run, which streams to stdout/stderr and needs no terminal. `--tui` with no terminal
on stdin (piped, redirected, CI) exits 1 and says why.

rusti draws its own terminal UI with direct ANSI escape sequences (no ratatui, no heavy TUI
framework). That keeps the binary small (~2.2 MB) and the rendering fast: only changed lines are redrawn,
with word wrap, streaming text, and an input box for `ask_user` questions.

## slash commands

- `/model` picks a saved profile, or `/model <name>` switches (same as `/use <name>`).
- `/new [name]`, `/resume`, `/rename`, `/tree` manage sessions ([sessions.md](sessions.md)).
- `/plan` toggles plan mode: the agent reads and proposes but every write and command is
  refused; leaving it returns to manual ([safety.md](safety.md)).
- `/add-dir <path>` lets the file tools read and write another folder for this run; `/add-dir` alone
  lists them and `/add-dir --remove <path>` takes one back ([safety.md § directory guard](safety.md#directory-guard)).
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

The status line leads with the permission mode's name (`manual`, `plan`, `auto`, `DEVIL`), rewritten in place when Shift+Tab or `/plan` changes
it: manual plain, plan in the info colour, auto in the warning colour, `DEVIL` in bold red. Then come the
session, model, git branch, tokens used, how full the context is and, when the provider reports
it, how much of the last prompt came from its cache (`cache 82%`). `/settings` hides or shows
each part but the mode.

## input

A query wider than the terminal wraps onto further rows and the input box grows to hold it, so
the whole query stays visible and the cursor moves down a row.

## transcript

The transcript marks what needs your eye. Your own message is bold. A failed tool sits on a
dark red background and a tool waiting for your permission on a dark amber one, each with its
own bright text so it reads whatever your terminal's colours are. Other tool rows get only a thin
coloured edge: green when done, the spinner colour while running. A run of file reads folds
into one row, the model's narration between tools is dimmed under a rail, and a tool's time
leaves out the time you spent answering its permission prompt. `"light": true` in
`~/.rusti/config.json` is only for a terminal with a light background: it swaps those fills
for pale ones with dark text, and darkens any theme colour that would read under 4.5:1 there
(the built-in palettes are made for a dark terminal, where each colour reads at 4.5:1 or better). A project's `./model.json` can set `"light"` too, since its
top-level keys override the global config. rusti cannot see your terminal's background, so turn
it off again if you go back to a dark one.

Model prose is rendered, not printed raw: ATX headings and `**bold**` come out bold, inline
code in cyan, `-`/`*` bullets as bullet glyphs, and fenced blocks in their own colour with the
fence lines hidden. No parser crate: markers become style runs over the visible text, so
wrapping still measures real columns. Fenced bodies are truncated rather than reflowed.

## selecting and copying

rusti captures the mouse so the wheel scrolls the transcript, which also takes the terminal's
own click-and-drag selection. Selection is done in the app instead: drag over the transcript
to highlight text (dragging to the top or bottom edge scrolls it), and on release it is copied
as plain text, with wrapped rows joined back into their lines, and `copied N lines` shows on
the status line. Double-click copies a word, triple-click a whole line; a plain click or any
key clears the highlight.

Rows are wrapped and cut, and clicks mapped to the text drawn under them, in terminal columns:
a tab is drawn as spaces to the next stop of 8, and CJK, Hangul, fullwidth forms and the common
emoji take two columns. That table is hand-rolled, so a rarer wide character, or a zero-width
one such as a combining accent, puts its row and the selection a column off.

The copy goes two ways at once: OSC 52, which asks the terminal to set the clipboard (Windows
Terminal, iTerm2, kitty and WezTerm honour it, over SSH too), and the platform's own tool:
`clip.exe` on Windows, `pbcopy` on macOS, `wl-copy`, `xclip` or `xsel` on Linux. When none of
those tools works the notice says so; the text then reached the clipboard only if the terminal
honours OSC 52.

To use the terminal's native selection instead (to copy the input line or the status line,
say), hold Shift while dragging: Windows Terminal and most others pass Shift+drag through to
their own selection even while an app captures the mouse.

## thinking

The model's thinking is shown in italic grey. Ctrl+T hides it: each finished block becomes one
`thinking… (N lines · ctrl+t)` row, and the block still streaming keeps its last two rows under
that row, so you can see the model working. The choice is saved as `"hide_thinking": true` in
`~/.rusti/config.json`; press Ctrl+T again to show it.

## waiting

The row above the input says what the turn is waiting on. `working…` means the model. When
`ask_user` has asked you something the question shows as a highlighted `Question` block, the
prompt changes from `>` to `?`, and the row reads `waiting for your answer · enter sends · esc
cancels`: type the answer and press Enter. Esc interrupts the turn at any point, including while
the provider
has gone quiet or a command is running: the command is killed with everything it started. A provider that sends nothing for 10 minutes before its reply starts fails the
turn with `no response for 10 min`; once the reply is streaming, 120 seconds of silence fails it
with `stream stalled: no data for 120s`.

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
running session untouched. An installed binary has no rusti source checkout on the machine
(a `cargo install --git` build lives in cargo's cache, which does not count), so
there `/reload` skips the build and restarts the same executable with the same session, model
and flags, which re-reads `config.json`, models, themes and extensions. How the release build
is tuned for that wait is in
[architecture.md § build profile](architecture.md#build-profile).
