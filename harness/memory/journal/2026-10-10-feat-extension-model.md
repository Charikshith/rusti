## 2026-10-10 - feat-extension-model
- Python heredocs through the Bash tool broke on some contents ("unexpected EOF"); writing the edit script with the Write tool and running `python -X utf8 file` worked every time.
- Python reading a heredoc on Windows used cp1252, so string matches containing ⟳/· failed until `-X utf8`.
- Global OnceLock config (hooks, shell) keeps tests hermetic; a Mutex the tests could set would race across parallel tests.
- `Result::unwrap_err` needs the Ok type to be Debug; `.err().unwrap()` avoids deriving Debug on Theme.
