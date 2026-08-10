---
type: template
title: "Open Work Template"
description: "Markers for work seen but declined under scope discipline — recruitable by a later or concurrent session"
artifact: "harness/open-work.md"
tags: [scope, recruitment, backlog, multi-agent]
---

# Open Work

Things noticed but not done, because they were out of scope for the active feature.

One line each: `- [reason-code] <one-line description> (seen in feat-XXX)`

Reason codes:

- `blocked-on` — cannot proceed until something else lands
- `needs-review` — done but wants a second pair of eyes
- `cheap-parallel-win` — small, independent, safe to pick up alongside other work
- `flaky` — intermittent, not reliably reproducible yet

> **Why this template**: scope discipline tells a session to decline unrelated work. It
> never said where the observation should go, so it went into `harness/progress.md` prose that
> nothing reads. The information was produced and then discarded. This file is the landing
> place, in a shape another session can act on.

## Not a backlog

An item earns a line here only because a session **actually saw it** and declined it under
scope discipline. This is not a wishlist, not a roadmap, and not a place for ideas nobody
has encountered in the code. Features belong in `harness/feature_list.json`; curation proposals
belong in `harness/dream-queue.md`.

Remove a line when the work is done or promoted to a real feature. A line that has sat
here across many sessions is itself a signal — either it is not actually cheap, or nobody
is reading this file.

## Items

_No open items yet._
