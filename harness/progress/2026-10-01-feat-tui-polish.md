## Session 2026-10-01: feat-tui-polish — startup MCP rows clear on first message; readable user rows
- tui/app.rs: `drop_mcp_ok` runs in `start_task`, removes "  ✓ mcp … tools" rows, keeps ✗ rows, shifts live line indices.
- tui/render.rs: user rows go through `user_row` (coloured marker, bold text) and no longer get `band()`.
- tui/theme.rs: `Tints.user` removed. docs/rusti-vs-pi.md tinting row corrected.
- Verification: `cargo test` (two new tests, one updated), `./init.sh` passed.
