## 2026-10-10 - feat-bughunt-platform-fixes
- Cargo's git checkouts contain .git, so "is a git tree" cannot tell them from a dev checkout; `.cargo-ok` (git checkouts and registry sources) can.
- std::process::Command escapes arguments itself when the program path ends in .cmd/.bat; resolving the shim and spawning it is enough, no cmd /C needed.
- No unit test set RUSTI_HOME before; a test that needs a configured MCP server now does, and leaves it set.
- `sort` is a portable never-answering stdio server for MCP timeout tests (reads to EOF, prints nothing).
