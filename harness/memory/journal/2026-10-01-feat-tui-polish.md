## 2026-10-01 - tui polish (MCP startup rows, user row band)
- The fix drops the user row's fill instead of retuning it. Correction (2026-10-10): this line first said theme::set_light is never called, so the "light" tints were dead. That was wrong: tui/app.rs calls set_light(cfg.light) at startup, and the captain's config had "light": true. The near-white band came from that flag, and the fail and ask rows kept the pale tints until feat-tint-text.
- Python via Bash heredoc again turned "\x1b" into a raw ESC in Rust source; a scratchpad .py file written with the Write tool avoided it.
- Removing rows from app.lines shifts tool_line / ask_line / retry_line / fold indices; any later row removal must shift them too.
