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
| devil | no prompts and no project-root guard, the same as `--yolo` |

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
every mode but devil the project-root guard stays on, and in auto, denials, project trust,
`pre_tool` hooks and saved [a]lways answers work as in manual. `--yolo` / `RUSTI_YOLO=1`
start in devil; any other run starts in manual.

MCP tools are always permission-gated and always refused in plan mode: rusti cannot read a
third-party tool's effects off its name, so it treats every one as if it mutates.

## project-root guard

Writes are refused outside the working directory (`../x`, absolute paths elsewhere), even for
files that do not exist yet. Reads are deliberately open so the model can inspect dependency
sources. `--yolo` (devil mode) lifts both the prompts and the guard.

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
