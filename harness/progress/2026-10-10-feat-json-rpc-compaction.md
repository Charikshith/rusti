## Session 2026-10-10: feat-json-rpc-compaction — --json, --rpc, compaction improvements
- rpc.rs (new): event_json (versioned line per Event), pump (sink -> stdout; asks held for RPC answers or refused), run_json, run_rpc (stdin command loop + agent thread).
- ai_core/mod.rs: compact() rewritten — token keep window (compact_cut), structured summary prompt over a text transcript, cumulative file lists stored on Entry.compaction, iterative update, focus text, pre_compact seam; run_agent compacts on an estimate before the first request and once on a context-overflow error; CompactStart/CompactEnd events; profile/config/flag context precedence.
- ai_core/llm.rs: chat_stream `quiet` (summary does not stream as a reply), is_overflow, optional reasoning_effort.
- TUI: /compact [focus] (Job::Compact), compaction rows with the summary behind Ctrl+O, /model applies the profile's context. main.rs: --json, --rpc. config.rs: ModelProfile.context.
- Docs: readme.md § compaction and § JSON and RPC modes, help text, docs topic map; docs/rusti-vs-pi.md rows corrected; open-work "Compaction Follow-ups" and "--json NDJSON Output" removed (done).
- Verification: cargo test (84 unit + 2 rpc e2e + 1 trust e2e), cargo run -- --self-test, clippy clean for the new code.
- Risks: compaction sends one summary request however large the summarized part is (ponytail note in compact()); RPC events and responses are ordered per thread only, documented as "match by id".
