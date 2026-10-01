## Session 2026-10-01: feat-queue-while-working — steer, follow-up and dequeue while the agent works (F04)
- ai_core: `STEER` static with `steer` / `take_steers` / `steers`; `run_agent` drains it after each tool batch
  (after the results and the pending image, `DEPTH == 0` only) into one user entry and emits `Event::Delivered`.
- tui/app.rs: Enter while working steers; Ctrl+Q queues a follow-up in `App.follow`; Alt+Up / Alt+Q and an Esc
  interrupt call `dequeue`; TaskEnd calls `next_queued` (late steers to the front, start one on success).
- tui/render.rs: `pending_rows` draws the queue above the panel. plain.rs and `emit()` ignore `Delivered`.
- Docs: `--help` key lines, readme TUI section, docs/rusti-vs-pi.md rows; open-work "Follow-Up Queue" removed.
- Verification: `cargo test` (65 + 1 passed, three new tests), `./init.sh` passed.
