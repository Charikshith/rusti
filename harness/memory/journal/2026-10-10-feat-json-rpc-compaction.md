## 2026-10-10 - feat-json-rpc-compaction
- The event SINK is a process-wide OnceLock: a unit test that set it would reroute every other test's emits and asks, so RPC is unit-tested without the sink and end-to-end through the real binary (tests/rpc.rs).
- A python script fed through a bash heredoc in the same command as another stdin reader hung the shell; write scripts to a file first, run them with stdin from /dev/null.
- A pump thread that never sees its channel close (the sink sender lives in a static) needs its own stop flag plus recv_timeout to drain before exit.
