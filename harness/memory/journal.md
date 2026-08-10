---
type: template
title: "Memory Journal Template"
description: "Append-only per-session friction log; the raw corpus that out-of-band curation reads to find patterns"
artifact: "harness/memory/journal.md"
tags: [memory, journal, append-only, curation, evidence, session-end]
---

# Session Journal

Append-only. One dated block per session, written at the end of it. **Never rewrite an
earlier block** — this file is evidence, and curation counts how often things recur.

This is **not** a lesson store and **not** a progress log:

- `harness/progress.md` = where the work stopped (status)
- `harness/memory/index.md` + topic files = distilled, durable lessons
- **this file** = raw friction, unfiltered, so curation has something to count

> **Why this template**: out-of-band curation needs a corpus. Without it, "find recurring
> patterns" has nothing to read and every proposal is an opinion. See
> [Dreaming](../references/dreaming-pattern.md).

## How to write a block

3-5 lines, no prose, no feature restatement. Answer only:

- What did you have to look up?
- What surprised you?
- What correction did you receive?
- What did you try and abandon, and why? (→ candidate `harness/memory/graveyard.md` row)
- What would you do differently?

If a session produced nothing worth any of those lines, write the date and `nothing`.
That is a real signal, not a gap.

<!-- Format:

## YYYY-MM-DD — feat-00X

- looked up: `<command or path>` — wasn't documented anywhere
- surprised: <the thing that behaved unexpectedly>
- corrected: <what the user told you>
- differently: <what you'd change>

Keep commands, paths and symbols in backticks. Curation looks for backticked tokens
that recur across sessions — a journal with no backticks is a journal of feature
restatements and will yield no proposals.
-->

## Entries

_No entries yet. The first one belongs here at the end of this session._
