## 2026-10-10 - feat-reload-installed
- env!("CARGO_MANIFEST_DIR") in non-test code is the CI build dir on a release binary; check it exists before using it.
- A Python edit script in a bash heredoc still turns `\\n` meant for Rust strings into real newlines here; use the Edit tool for those lines.
