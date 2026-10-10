## 2026-10-10 - feat-tools-phase4
- The shell hook joins heredoc lines that end in `\`, so python edit scripts with Rust line-continuation strings must go through a file, not a heredoc.
- Struct-update syntax (`..other()`) cannot move out of a type with Drop (mcp::Server); the test helper takes the fields instead.
- run_agent tests that run tools must hold STEER_TEST (a batch takes queued steers) and UNDO_TEST_LOCK when they write.
- Timing proof for concurrency: `ping -n N 127.0.0.1 && echo X` runs in both cmd and Git Bash, so one command string serves Windows.
