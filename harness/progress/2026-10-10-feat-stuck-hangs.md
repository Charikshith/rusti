## Session 2026-10-10: feat-stuck-hangs — turns stuck on "working…"
- ai_core/llm.rs: Client::wait (100 ms cancel tick, 120 s STALL limit) around send, the error body and each chunk; handle_event returns true at [DONE] and the stream loop stops there.
- ai_core/mod.rs: run_agent refuses a call whose non-empty raw_arguments do not parse (beside the empty-name refusal); dispatch refuses ask_user with no non-empty question.
- tui/render.rs: hint_text() pulled out of draw; a pending free-text question shows "answer above · enter sends · esc cancels".
- tests/rpc.rs: fake_llm_held (socket held open, no Content-Length) + three repro tests; render and llm unit tests.
- docs/tui.md "waiting", docs/tools.md refused calls; open-work gets the two skipped follow-ups.
- Verification: cargo test (114 unit + 6 rpc + 1 trust) and ./init.sh pass; clippy adds nothing new (see harness/features/feat-stuck-hangs.json).
