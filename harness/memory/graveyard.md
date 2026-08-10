---
type: memory
title: "Memory Graveyard Template"
description: "Verdicts on abandoned routes: what was tried, why it failed, and the condition under which the verdict expires"
artifact: "harness/memory/graveyard.md"
tags: [memory, graveyard, negative-knowledge, abandoned, recheck]
---

# Graveyard

Routes tried and rejected. Not a lesson store — a **prohibition list with an expiry**.

> **Why this template**: memory records what worked. Without a record of what *didn't*, the
> same dead end gets re-explored every few sessions at full cost, because nothing in the
> repo says it was already tried. See [Dreaming](../references/dreaming-pattern.md).

| Route | Verdict | Because | Blast | Sessions | Recheck-if |
|---|---|---|---|---|---|
| `date-fns-tz` for scheduling | rejected | drops DST on half-hour offset zones | 2h | 3 | upstream #1483 closes |

- **Route** — what was tried, specifically enough to recognise a re-proposal of it.
- **Verdict** — `rejected` or `deferred`. Deferred means it would work but not yet.
- **Because** — the observed failure. Not a guess, not a preference.
- **Blast** — time spent before abandoning it. This is what a re-exploration costs.
- **Sessions** — how many sessions have hit this. Rising numbers mean the prohibition
  isn't being read.
- **Recheck-if** — the condition under which this verdict expires.

**Recheck-if is mandatory.** A row without one is folklore — nobody can tell whether it
still applies, so it is either obeyed forever or ignored entirely. Both are wrong. If you
genuinely cannot name an expiry condition, the verdict is a preference, not a finding, and
does not belong here.

## Not a lesson store

This file is deliberately **not** linked from `harness/memory/index.md` and is not a topic file.
Lessons say "do this"; graveyard rows say "this was tried and here is what it cost". It is
consulted directly, before proposing a library, a refactor, or a rewrite — not browsed via
the index.
