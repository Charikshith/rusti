## Session 2026-10-10: feat-bughunt-cosmetic — bug-hunt F5, F13, F14, F15
- tui/render.rs: width()/columns()/expand_tabs() beside wide(); truncate_str, wrap_ranges, band, render_table and input_window measure columns; push_wrapped expands tabs; the hardware caret sits after the drawn columns; hint_text shows the answer hint under any non-Ask picker.
- tui/mod.rs: word_wrap measures columns; columns and truncate_str re-exported for tree.rs. tree.rs pads and cuts by columns. app.rs footer_right too.
- tui/theme.rs: low-contrast roles raised in the table; current() returns a derived, cached light version under "light" (fg_rgb, readable_on, xterm palette).
- tui/app.rs: push_text replaces the todo list in place (todo_line, shifted with the other live rows); warn_once for /themes; a free-text Ask closes an open picker. ai_core/mod.rs emits the todo list as one text event.
- docs/tui.md, docs/tools.md updated; the open-work unicode-width item is resolved.
- Verification: cargo test (120 unit + 6 rpc + 1 trust) passes; clippy adds nothing new (see harness/features/feat-bughunt-cosmetic.json).
