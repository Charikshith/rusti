## Session 2026-10-10: feat-project-system-prompt — project SYSTEM.md / APPEND_SYSTEM.md (F11 project half)
- ai_core/mod.rs: prompt_base takes a folder list; prompt_dirs() = ~/.rusti plus ./.rusti when trusted.
- config.rs: APPEND_SYSTEM.md added to GATED_RESOURCES (SYSTEM.md was already there).
- Docs: readme § project instructions and § project trust, --help text, rusti-vs-pi rows, open-work entry dropped.
- Verification: extended system_prompt_override test; ./init.sh (cargo test) passes.
