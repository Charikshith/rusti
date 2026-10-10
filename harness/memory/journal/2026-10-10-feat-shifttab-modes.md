## 2026-10-10 - feat-shifttab-modes
- The heredoc `\`-joining bit again: a Rust string continuation written through a python heredoc lost its backslash; use the Edit tool for those lines.
- `cat > file` with no stdin hangs the Bash tool until timeout; never start a command with it.
- tools::guard reads the global mode, so unit tests must never set Devil while a parallel test checks the guard.
