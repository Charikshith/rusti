# Sessions

Back to the [readme](../readme.md).

## session tree

Sessions are stored as a tree of entries with `id`/`parentId` fields, mirroring pi's session
model; the current position is the active leaf.

Each session is one file under `.rusti/sessions/`. `--session NAME` (or `/new NAME`) picks the
name; otherwise a launch or `/new` starts `s-YYYYMMDD-HHMM` (local time, `-2` and up when that
minute is taken), so a fresh start never overwrites an older session. `--resume` continues the
latest session from its active leaf and `--tree` browses it; with `--session` they open that
one instead. A `./session.json` from older versions is still listed and resumable.

In the TUI, `/new [name]` saves the session and starts an empty one in place, keeping the
model, permission mode, MCP servers and background jobs. `/resume` with no argument lists
saved sessions and switches to the one you pick; with a name or number it switches straight to
it. `/rename` names the session. `/new` and `/resume` also reset `/undo`, so it can only put
back files the current session changed.

`rusti --tree` or `/tree` browses the tree and branches from an earlier entry, with the same
selection semantics as pi's `/tree`:

| pick | leaf moves to | prompt |
|------|---------------|--------|
| user message | entry's parent | text prefilled (edit + resubmit = new branch) |
| assistant message | that entry | empty (continue from there) |

In interactive terminals: ↑/↓ to navigate, Enter to branch, Esc to cancel. With piped stdin:
a plain numbered list; type a number and press Enter.

## compaction

When the prompt passes the context limit, rusti summarizes the older part of the session and
carries on from the summary. The limit is `--context` / `RUSTI_CONTEXT` if given, else the
active profile's own `"context"`, else the config's `"context"`, else 100000 tokens. A
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
- **When:** before a request once the last prompt (or, at the start of a turn, the estimate)
  passes the limit; once on a provider's context-overflow error, after which the request is
  sent again; and on `/compact [focus]` in the TUI (`compact` over RPC), where `focus` tells
  the summary what to keep.
- The summary shows as one `⟳ compacted` row; Ctrl+O shows its text.
