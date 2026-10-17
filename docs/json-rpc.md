# JSON and RPC modes

Back to the [readme](../readme.md).

Two modes let another program run rusti without scraping text. stdout carries only JSON lines
(one object per LF-terminated line, every object with `"v": 1` and a `"type"`); everything for
humans goes to stderr. `"v"` changes only when a field changes meaning or goes away; new
fields and new types can appear in v1, so ignore what you do not know.

Neither mode prompts on stdin. With no saved model and no `--url`, rusti exits 1 with the reason
on stderr instead of asking for one; `--tree`, and `--json --resume` with no TASK, are refused
the same way.

## --json

`rusti --json [flags] TASK` runs one task. The first line is `session`, then the run's events,
then a last `task_end` that adds `"answer"` (the final reply). Exit code 0 or 1. Questions
cannot be answered here: a permission prompt is refused unless `--yolo`, and `ask_user` gets
`"no answer given"`; the `ask` line still shows what was asked. Like any one-shot run, the
folder is untrusted unless `--trust`. The file tools stay in the working directory; pass
`--add-dir PATH` (repeatable) to let them use another folder ([safety.md](safety.md#directory-guard)).

```sh
rusti --json "run the tests and fix what fails" | jq -c 'select(.type=="tool_end")'
```

## events

Both modes write these:

| type | fields |
|------|--------|
| `session` | `name`, `path`, `model`, `messages` (user messages on the path), `cwd`, `thinking` |
| `text_delta` / `thinking_delta` | `text`: a streamed chunk of the reply / of the model's reasoning |
| `text` | `text`: a whole transcript line (attachments, todo lists, warnings); a todo list is one `text`, its items joined by newlines |
| `tool_start` | `id`, `summary`, e.g. `"read src/main.rs"` |
| `tool_end` | `id`, `summary`, `ok`, `ms`, `output` (the tool's result) |
| `usage` | `tokens`, `prompt`, `estimated`, `gen_ms`, `cached` (null when not reported); none for compaction's summary request |
| `retry` | `attempt`, `of`, `wait_ms`, `error` |
| `ask` | `id`, `question`, `choices`: `[{label, value}]`, empty for a free-text question |
| `steer_delivered` | `text`: queued steer text that just joined the conversation |
| `compaction_start` | `reason` (`threshold`, `overflow`, `manual`), `messages`, `tokens` |
| `compaction_end` | `summary`, `tokens` (estimated, after) |
| `notice`, `git`, `session_name`, `resumed` | status: `text`, `branch`, `name`, `messages` |
| `task_end` | `ok`, `error` (`"interrupted"` after an abort); `--json` adds `answer` |

Tool calls can run in parallel, so a `tool_end` belongs to the `tool_start` with the same `id`.

## --rpc

`rusti --rpc [flags]` stays up and reads one command per stdin line. It writes the `session`
line at start, then a `response` for every command and the same events as `--json`. A command
may carry any JSON `"id"`; its response echoes it:
`{"v":1,"type":"response","id":7,"command":"prompt","ok":true,"data":{...}}`, or `"ok":false`
with `"error"`. Match responses by id, not by position: events a command causes can arrive
before or after its response. Closing stdin lets the running task and the queue finish,
refuses any question still open, and exits 0.

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

`new_session`, `resume_session`, `set_model`, `set_thinking` and `compact` need the agent idle
and fail with an error while a task runs, and their response marks the end; `new_session`,
`resume_session`, `set_model` and `set_thinking` send a fresh `session` line (when it changed)
before that response. Permission prompts and `ask_user` become `ask` lines that wait for an
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
