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

Not status. "Where the work stopped" belongs in `harness/progress/` and `harness/features/` (older: `harness/progress.md`, `harness/feature_list.json`).
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

- [RTK filters command output](rtk-filters-command-output.md) — a filtered grep cannot prove a symbol is absent; use `rtk proxy` first.
- [Alternate screen beats escape hacks](alternate-screen-vs-escape-hacks.md) — a bottom-pinned main-screen TUI always leaves a gap; switch the buffer model, don't tune the sequences.
- [Parent-chain walk drops fan-out siblings](session-tree-path-drops-siblings.md) — several entries under one parent look fine in session.json but the single-parent path walk omits the non-active ones; chain, don't fan out.
- [A silent default that is later saved destroys the file](silent-default-then-save-destroys.md) — a forgiving loader plus any settings toggle equals a delete command with a delay.
- [An image part can be dropped by the client or by the proxy](image-parts-dropped-in-two-places.md) — same symptom from two unrelated layers; log what the proxy transform receives, and forbid decoding in a vision test.

## Retired

<!-- Move lines here instead of deleting them, with a reason. A wrong lesson is the
     record of how far it propagated. -->
