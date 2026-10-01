## 2026-10-01 - feat-at-file-refs
- Caching menu rows behind a key missed state that changes without touching the input (Esc's menu_off, a Tab list): the key must hold everything menu_items reads.
- Python edits through a bash heredoc mangled a non-ASCII em dash in the match string; the Edit tool matched it fine.
- rg --files lists files only, so the picker's folders have to be derived from the file paths.
