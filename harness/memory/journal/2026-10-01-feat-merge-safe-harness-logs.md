## 2026-10-01 - merge-safe harness logs
- The harness files are CRLF in the working tree (core.autocrlf=true): a node string-insert keyed on "\n" missed; the Edit tool did not.
- The pre-commit hook hard-coded the old paths, so the convention change had to land there too or every feature commit would be rejected.
- A merge check needs a control case that must conflict, or a broken simulation passes silently.
