## Session 2026-10-10: feat-tools-phase4 — parallel tool calls + batch (F07), tool search (F08), cache markers (F10 C3)
- ai_core/mod.rs: run_agent step 1 (rows + permissions in order, batch opened), step 2 (parallel_groups + join_all,
  spawn_blocking for sync parallel-safe tools), step 3 (results in call order, batch_result); run_call, tool_search
  schema, tool_schemas(loaded) per request; compaction carries Entry.loaded; Event ToolStart/ToolEnd carry ids.
- ai_core/mcp.rs: Server.defer, deferred(), search() + BM25, reader lock held across a request.
- ai_core/llm.rs: Client.cache, mark_cache, cache_key; chat_stream takes the session's cache key.
- config.rs CacheMode + McpServer.defer; session.rs Entry.loaded + Session::loaded; tools.rs PENDING_IMAGES list;
  tui/app.rs tool_rows (id, line, hand) with shift_rows/insert_rows; main.rs + /model pass the profile's cache.
- Docs: readme (Use, architecture, MCP servers), help text, rusti-vs-pi rows, open-work entries updated.
- Verification: 8 new tests (see harness/features/feat-tools-phase4.json); cargo test all pass; --self-test OK.
- Not built: codemode (deferred), hooks (Phase 3; seam marked in step 1), cache warming, a 1h TTL option.
