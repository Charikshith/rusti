# Safety

Back to the [readme](../readme.md).

## permission prompts

Every tool that changes state (`write_file`, `edit_file`, `multi_edit`, `delete_file`,
`move_file`, `run_command`, `run_background`) or reaches the network (`web_fetch`) asks
before running:

```
ℹ allow run_command cargo test? [y]es / [n]o / [a]lways (saved)
```

`a` allows that tool from now on: it is saved to `model.json`'s `"allow"`, so later runs
don't ask again. A denial is returned to the model as a tool error so it can explain or ask
instead of retrying. With piped stdin there is nobody to answer, so everything is denied;
pass `--yolo` for scripted runs.

## permission modes

Shift+Tab in the TUI cycles four modes, manual → plan → auto → devil. The status line shows the
current one in place, so cycling adds nothing to the transcript ([tui.md § status line](tui.md#status-line)).

| mode | what happens |
|---|---|
| manual | the default: every tool above asks |
| plan | reads and searches only; every tool above is refused, whatever the saved answers say (same as `/plan`; leaving it returns to manual) |
| auto | runs without asking: `write_file`, `edit_file`, `multi_edit` and `move_file` inside the project root, and `run_command` / `run_background` unless the command is risky (below). Still asks: `delete_file`, `web_fetch`, every MCP tool not already answered [a]lways, and risky commands |
| devil | no prompts and no directory guard, the same as `--yolo` |

Risky commands, which auto mode still asks about:

- `rm`, `rmdir`, `del`, `erase`, `rd`, `Remove-Item`, `sudo`, `chmod`, `chown`, `icacls`,
  `takeown`;
- `git push`, `git reset --hard`, `git clean`, `git checkout --`, `git restore`,
  `git branch -D`, `find -delete`;
- package installs and removals (`npm`/`pnpm`/`yarn`, `pip`, `uv`, `cargo install`/`add`/`remove`,
  `gem`, `go install`, `apt`, `brew`, `winget`, `choco`, `scoop`);
- a download in the same command as a shell (`curl … | sh`, `irm … | iex`);
- a write (`>`, `cp`, `mv`, `tee`, `mkdir`, `sed`, `perl`, `curl`, `wget`, `tar`, `unzip`,
  `git clone`, `Copy-Item`, `Set-Content`, …) naming a path outside the project root (`..`,
  `~`, `$HOME`, `%USERPROFILE%`, an absolute path elsewhere, also as `of=/x`, `--output=../x`
  or `-C/x`, or a remote `host:/x` target).

The risky-command check matches words, not shell semantics, so it errs toward asking. In
every mode but devil the [directory guard](#directory-guard) stays on, and in auto, denials, project trust,
`pre_tool` hooks and saved [a]lways answers work as in manual. `--yolo` / `RUSTI_YOLO=1`
start in devil; any other run starts in manual.

MCP tools are always permission-gated and always refused in plan mode: rusti cannot read a
third-party tool's effects off its name, so it treats every one as if it mutates.

## directory guard

The file tools stay inside the working directory (the project root). `read_file`, `list_dir`,
`glob`, `grep`, `write_file`, `edit_file`, `multi_edit`, `delete_file` and `move_file` refuse a
path that resolves outside it, reads and searches included, even for files that do not exist
yet. The path is judged after resolving `..`, symlinks and drive or UNC prefixes, on the
deepest part that exists: a symlink inside the project that leads out is refused, a link that
dangles is refused, and the std-fallback `list_dir` / `glob` / `grep` do not walk through a
link that leaves the allowed folders. A refusal comes back to the model as a tool error naming
the allowed folders and saying to ask you for `/add-dir`; nothing is read.

To let the agent use another folder, add it yourself:

- `/add-dir <path>` grants that folder and everything under it for this run, to every file tool
  above (`~` works). `/add-dir` alone lists the grants and `/add-dir --remove <path>` takes one back.
- `--add-dir <path>` does the same at launch, repeatable (for one-shot, `--json` and `--rpc` runs,
  which have no `/` commands).

A grant is not a promise of no prompts: writes in an added folder still ask in manual mode, and
auto mode only runs unasked what is inside the project root itself. rusti's own docs
(`~/.rusti/docs`) and your skill folders (`~/.rusti/skills`, `~/.agents/skills`) stay readable
without a grant, because the system prompt sends the model there; they are never writable.
`--yolo` (devil mode) lifts the prompts and the guard together.

**What it does not cover.** Only the file tools are checked. `run_command`, `run_background`
and `!cmd` run a shell that can reach any path the user can, so a model that may run commands
can still `cat ../../secret`. rusti does not sandbox the shell: the manual-mode prompt, auto
mode's risky-command list (which asks about writes that name a path outside the root) and
plan mode are the controls there. MCP tools are third-party processes and are likewise
outside the guard. Treat the guard as a fence against the model wandering, not as isolation;
for isolation run rusti in a container or VM.

To tighten the policy further (say, refuse reads of `.env` files inside the project), write a
`pre_tool` [hook](extensions.md#hooks): it sees every tool call's arguments and can refuse it. A
hook can only narrow what the guard allows, never widen it; a built-in hook point for the
guard's own policy is not there yet.

## project trust

A cloned repo's `model.json` could otherwise start its own MCP servers, swap the shell or
pre-approve `run_command` before you type anything. So when `./model.json` sets `mcp`,
`shell`, `shell_command_prefix`, `hooks`, `read_allow` or `allow` (or `./.rusti/` has
`prompts`, `skills`, `themes`, `SYSTEM.md` or `APPEND_SYSTEM.md`, or there is a
`./.agents/skills`), the TUI asks once at launch:

```
This folder's model.json sets mcp, shell, allow. Trust it? [y]es / [N]o / [a]lways
```

`y` trusts it for this run, `a` also saves it by canonical path in `~/.rusti/trust.json`.
Untrusted, those keys and folders are ignored with one warning line (keys are saved back
untouched); `max_iters`, `context`, `theme` and `footer` still apply. Piped and one-shot runs
never ask and stay untrusted unless `--trust` is given. `--yolo` does not imply `--trust`.
