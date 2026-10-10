## 2026-10-10 - feat-stuck-hangs
- The proof came from outside rusti: 9router's requestDetails table logs a start row and a completion row per streamed request, so an orphan start row marks a provider stall; request sizes there also bounded the lost ask_user arguments.
- A fake server with Content-Length cannot reproduce a stall or a keep-alive after [DONE]: the client ends the body at the length; the held reply must send no length.
- The real TUI can be driven on Windows with pywinpty + pyte (Shift+Tab is ESC [ Z); worth it to confirm a screen before writing the RPC-level regression test.
- `python -I` ignores PYTHONIOENCODING; use `-X utf8` for non-ASCII output on cp1252 consoles.
