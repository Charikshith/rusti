## Session 2026-10-10: feat-tint-text — failed and asking rows pin their own text colour
- tui/theme.rs: `Tints` gains `fail_text` / `ask_text` (DARK #E4E4E4, LIGHT #1C1C1C); `tints_for(theme, light)` is the pure form `tints()` calls; theme files accept `tints.fail_text` / `ask_text` as foregrounds.
- tui/render.rs: b'x' and b'a' rows, header and wrapped rows, draw their text in the tint's text colour.
- docs/tui.md and docs/extensions.md: `"light"` is only for light-background terminals; new tint keys. The 2026-10-01 tui-polish journal line that said set_light was never called is corrected.
- Verification: `cargo test` 113 passed (one new test, two extended).
