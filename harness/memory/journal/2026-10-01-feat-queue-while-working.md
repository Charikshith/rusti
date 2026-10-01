## 2026-10-01 - queue while working (F04)
- src/ files are CRLF in the working tree; a Python read with default newline mode silently rewrote ai_core/mod.rs as LF.
- A Bash heredoc fed to Python turned "\n" escapes into real newlines in Rust literals (one even compiled, inside a string).
- STEER is process-global, so the two tests that touch it share a lock; run_agent in any parallel test would drain it.
