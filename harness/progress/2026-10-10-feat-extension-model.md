## Session 2026-10-10: feat-extension-model — Phase 3 extension model (E0 + E1 + E2)
- resources.rs (new): dirs() with the trust gate, hand-rolled frontmatter, prompt templates + expand/split_args, skills + system prompt section + /skill message.
- ai_core/hooks.rs (new): six events, JSON in/out, timeout, chaining; wired into run_agent (user_prompt, steers, session_start, pre_tool/post_tool), compact (pre_compact) and tui run_bash (user_shell, F09).
- ai_core/mcp.rs: prompts/list on connect, prompts(), prompt_args(), get_prompt(); fake server gained prompts.
- tui: dynamic / menu (Cmd owns its strings, BUILTINS + commands()), run_extension, Job::McpPrompt, theme files + hot reload, menu source tags.
- config.rs: "hooks" key, .agents/skills in the trust scan. Docs: --help, readme § extensions, docs/rusti-vs-pi.md rows.
- Verification: cargo test (84 + the trust e2e), clippy clean on new code, a one-shot run with a blocking hook.
- Not built: templates in one-shot runs, ${@:N:L}, project SYSTEM.md (separate task), E3/E4.
