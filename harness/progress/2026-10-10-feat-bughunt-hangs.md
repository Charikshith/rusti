## Session 2026-10-10: feat-bughunt-hangs — crashes and stuck turns from the bughunt (PR 1 of 3)
- tui/render.rs: input_window drops the first visible char as a char, not a byte (F1).
- ai_core/llm.rs: truncate cuts the error body on a char boundary (F2).
- ai_core/mod.rs: `unpanic` turns a panic in a turn into its error; run_agent wraps the loop (now `turn`) in it; dispatch relays the turn's cancel into a per-call flag for the blocking pool; run_sync passes it to run_command (F2, F3).
- ai_core/tools.rs: read_live collects into a shared buffer and `drain` takes it 500 ms after the shell exits; run_command_live takes `stop`; `own_group` + `Tree` (Unix process group, Windows job object over kernel32) kill the whole tree on timeout, Esc and job_stop; job_output cuts its tail on a char boundary (F3, F4, F11).
- ai_core/hooks.rs: the same bounded read and tree kill (F3, F11).
- tui/mod.rs: --tui without a terminal returns an error before anything starts; the plain fallback and tui/plain.rs are gone; `!cmd` gets the cancel flag, reset first (F9, F3). rpc.rs and the TUI run manual compaction through unpanic.
- main.rs: --rpc/--json never prompt: no model exits 1, --tree is refused there, --json --resume needs a TASK (F10).
- docs: tui.md, tools.md, configuration.md, extensions.md, json-rpc.md, architecture.md, rusti-vs-pi.md; Cargo.toml's panic=abort comment now names unpanic; open-work drops the "Esc does not kill commands" item.
- Verification: ./init.sh (125 unit + 10 rpc + 1 trust) passes on Windows; clippy adds nothing. Risk: the Unix group kill is first exercised by the release workflow's Linux and macOS cargo test.
