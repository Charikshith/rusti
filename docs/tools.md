# Tools

Back to the [readme](../readme.md). What each tool may do without asking is in
[safety.md](safety.md).

The built-in tools: `read_file` (offset/limit), `write_file`, `edit_file`, `multi_edit`,
`delete_file`, `move_file`, `grep` / `glob` / `list_dir` (ripgrep if installed, std
fallback), `run_command` (with a timeout), `run_background` / `job_output` / `job_stop`,
`todo`, `delegate`, `ask_user`, `web_fetch` (tags stripped) and `batch`; plus
`tool_search` and `mcp__<server>__<tool>` from [MCP servers](extensions.md#mcp-servers).

The file tools (`read_file`, `list_dir`, `glob`, `grep` and every writer) refuse paths outside the
working directory unless added with `/add-dir`; `run_command` is not covered
([safety.md § directory guard](safety.md#directory-guard)).

A call whose arguments are not valid JSON is refused back to the model with the parse error,
so it can resend the call, instead of running on defaults; an `ask_user` with no question is
refused the same way rather than shown to you as an empty one.

`run_command` is killed with every process it started when it reaches `timeout_secs` (default
120) or you press Esc. A process it leaves running in the background (`npm run dev &`) does not
hold the call open: the result comes back when the shell exits, with a note to use
`run_background` for servers and watchers.

A command that asks on the terminal (an ssh or git password, `sudo`) cannot prompt inside
rusti: it waits until its timeout and is killed. Use key-based or otherwise non-interactive
auth, or run such commands outside rusti.

## read_file limits

`read_file` returns at most 2000 lines or 50 KB per call. A cut read ends with
`[Showing lines 1-2000 of 5231. Use offset=2001 to continue.]`, so the model can page through
the rest; a single line over the cap gets a `sed -n 'Np' FILE | head -c N` hint instead.
`"read_max_bytes"` (in either config file, default 51200) changes the byte cap; `0` removes
both caps, at your own risk: every result is re-sent on each later turn until compaction.

## parallel tool calls and batch

When the model asks for several tools in one turn, every permission prompt comes first, one
at a time in call order. Then reads, searches, `run_command`, `run_background`, `web_fetch`
and MCP calls that sit next to each other run at the same time; a write, `ask_user`,
`delegate` or `todo` runs alone in its place, so a read the model put after a write sees it.
Results reach the model in call order.

The `batch` tool (`{"calls": [{"name", "args"}]}`) does the same for a model that makes one
call per turn, and returns one combined result.

## sub-agents and background jobs

- `delegate(task)` runs a fresh agent with the same tools in its own session file
  (`.rusti/sessions/sub-<pid>-<ts>.json`) and returns only its final report, keeping the
  subtask's reads and edits out of the main context. One level deep; the sub-agent streams
  into the same transcript.
- `run_background` starts a server or watcher and returns a job id; `job_output` reads what
  it has printed so far, `job_stop` kills the whole process tree. On Linux and macOS jobs,
  commands and hooks still running die with rusti (when it exits, or its terminal closes, or
  it gets SIGTERM); on Windows jobs outlive the agent if not stopped.
- `todo` lets the model keep a visible checklist for multi-step work; each update replaces the
  list in the transcript rather than printing another copy.
