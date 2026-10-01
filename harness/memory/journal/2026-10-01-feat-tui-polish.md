## 2026-10-01 - tui polish (MCP startup rows, user row band)
- theme::set_light is never called, so the "light" tints are dead; the near-white band in the report was not reproduced from code, which is why the fix drops the fill instead of retuning it.
- Python via Bash heredoc again turned "\x1b" into a raw ESC in Rust source; a scratchpad .py file written with the Write tool avoided it.
- Removing rows from app.lines shifts tool_line / ask_line / retry_line / fold indices; any later row removal must shift them too.
