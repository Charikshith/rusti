---
type: memory
title: "Session Journal"
description: "Append-only per-session friction log"
artifact: "harness/memory/journal.md"
---

# Session Journal

## 2026-08-11 — feat-001 through feat-008

- looked up: `crossterm::event::read()` on Windows with piped stdin — doesn't work in raw mode, had to add `is-terminal` check and plain stream fallback
- surprised: ratatui removal saved ~200KB deps but binary only shrunk 200KB (2.8→2.6 MB) — most weight is in reqwest/tokio
- corrected: `ListState.offset` is private in ratatui 0.30 — had to drop manual scroll tracking, List auto-scrolls to selection
- differently: would start with ANSI escapes from the beginning instead of adding ratatui then removing it
- surprised: `prompt()` with EOF loops forever if stdin closes — fixed with `Option` return and exit(1) on None
