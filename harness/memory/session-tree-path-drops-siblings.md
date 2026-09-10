# A parent-chain walk drops fan-out siblings from the message list

**Why:** `session.add` lets several entries share one parent (fan-out), but
`path_messages()` reconstructs the request by walking a single parent chain from
the active leaf. Any sibling that is not on that chain is silently omitted. So a
tree shape that looks complete in `session.json` can still send an incomplete
message list.

## What happened

The model emitted two tool calls in one assistant turn (`run_command` +
`list_dir`). `run_agent` added both tool results as children of the assistant
entry:

```
m3 assistant [2 tool calls]
├── m4 tool (call_00)   ← dropped
└── m5 tool (call_01)
```

`session.active` ends at the last-added sibling (`m5`), so the next request went
`m3 → m5` and `m4` never reached the wire. The provider rejected the turn:
`Tool result is missing for tool call call_00_7dueXDf20YUXmQlI48NA3107`. Every
later turn re-sent the same broken prefix and failed identically.

## Fix

Chain results under each other instead of fanning them out, so all of them lie on
the active path in order:

```rust
let mut parent = a_id.clone();
for tc in &res.tool_calls {
    // ... run the tool ...
    parent = session.add(te, Some(parent));
}
```

## The general rule

A single-parent tree cannot represent a linear message list that branches. When a
turn needs N entries in the history (N tool results, or a user turn that is more
than one message), either chain them or make the path builder expand fan-out
explicitly — never leave them as siblings and assume `path()` sees them.

## Regression guard

`--self-test` now emits **two** tool calls in its first fake response and asserts
the path holds 6 messages, with `msgs[3]` and `msgs[4]` both `role: "tool"`. The
old sibling layout fails that assertion (5 messages, the second result missing).
