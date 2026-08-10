---
type: template
title: "Dream Queue Template"
description: "Bounded queue of out-of-band memory curation proposals, each with evidence and prevalence, awaiting human accept or reject"
artifact: "harness/dream-queue.md"
tags: [memory, dreaming, curation, review, proposals, human-gate]
---

# Dream Queue

Proposals from out-of-band memory curation. **Nothing here has been applied.** Each line
is a suggestion awaiting a human decision.

**Drain this queue before starting new feature work.**

> **Why this template**: instantiates the [Dreaming](../references/dreaming-pattern.md)
> pattern. Curation proposes; the human decides. The queue is bounded so that review
> stays possible.

## Rules

- **Cap: 5 open proposals.** A curation pass that found more must merge or drop its own
  before adding. Volume is the proposer's problem, not the reviewer's.
- Every proposal states **claim**, **evidence**, and **prevalence**. No citation, no entry.
- Curation may propose changes to `harness/memory/` only. It may *suggest* an instruction change
  but never edits `AGENTS.md` itself.
- Record rejections in **Decided** below. Without that record the same proposal returns
  every cycle.

## Open

_No open proposals._

<!-- When proposals exist, use this table. Keep it to 5 rows.

| # | Proposal | Evidence | Seen | Disposition |
|---|---|---|---|---|

| 1 | Add lesson: deploy via `make ship`, not ./deploy.sh | journal 07-14, 07-19, 07-22 | 3x | pending |
| 2 | Retire harness/memory/build-cache.md — contradicts harness/memory/ci-flow.md | both files | 1x | pending |
-->

## Decided

| Date | Proposal | Outcome | Reason |
|---|---|---|---|
| — | | | |

## Last curation pass

**Date:** never
**Inputs reviewed:** —
**Cadence:** every ~10 sessions, or weekly. A pattern needs several sessions to exist;
running this after every session produces noise and trains the reviewer to skim.
