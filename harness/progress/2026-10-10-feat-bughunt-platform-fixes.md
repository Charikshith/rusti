## Session 2026-10-10: feat-bughunt-platform-fixes — report findings F6, F7, F8, F12
- ai_core/tools.rs: multi_edit retries an unmatched LF old_text with CRLF line breaks in a CRLF file and writes new_text CRLF.
- ai_core/mcp.rs: build() spawns the command directly; resolve() looks a bare name up on PATH with .com/.exe/.bat/.cmd. set_enabled re-checks config and registry before registering a server.
- tui/app.rs: toggle_mcp runs set_enabled on a spawned thread and posts a Notice; a Notice refreshes an open /mcp list.
- tui/mod.rs: reload_exe treats a manifest dir with cargo's `.cargo-ok` as installed; docs/tui.md § /reload says so.
- Verification: four regression tests (see harness/features/feat-bughunt-platform-fixes.json); cargo test 129 pass; ./init.sh passes.
