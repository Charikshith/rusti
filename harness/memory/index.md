---
type: template
title: "Memory Index Template"
description: "Bounded always-on index of agent-written lessons; one line per topic file, loaded every session"
artifact: "harness/memory/index.md"
tags: [memory, index, progressive-disclosure, session-continuity, learning]
---

# Memory Index

One line per lesson. This file is read **every session**; topic files are opened only
when their line looks relevant.

**Hard cap: ~200 lines.** Overflow is a signal to merge or delete lessons, not to raise
the cap. See [Memory Persistence](../references/memory-persistence-pattern.md) and
[Dreaming](../references/dreaming-pattern.md).

> **Why this template**: instantiates the bounded-index half of the Memory Persistence
> pattern. The index is always-on context; the topic files are on-demand detail.

## What this file is not

Not status. "Where the work stopped" belongs in `harness/progress.md` and `harness/feature_list.json`.
This file holds only what was **learned**.

## Lessons

<!-- Format:  - [title](file.md) — one-line hook, enough to decide whether to open it
     Add newest at the bottom. Never edit someone else's line to mean something new;
     retire it and add a new one.

     Link targets here are the ONE exception to this harness's repo-root-relative path
     convention: write them sibling-relative (`lesson.md`, or `topics/lesson.md`), not
     `harness/memory/lesson.md`. Everything else in the harness is anchored at the repo
     root, so this is easy to get wrong — the validator accepts both, but sibling links
     keep this file readable on its own. -->

_No lessons recorded yet. The first correction you receive belongs here._

## Retired

<!-- Move lines here instead of deleting them, with a reason. A wrong lesson is the
     record of how far it propagated. -->
