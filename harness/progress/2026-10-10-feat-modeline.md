## Session 2026-10-10: feat-modeline — permission mode lives in the status line
- tui/app.rs: Shift+Tab and /plan no longer push a transcript notice or a hint; mode_notice -> mode_label
  (name, one-line meaning); footer_right takes the width and always leads with "name: meaning", the
  meaning ellipsised (then dropped) on a narrow terminal, the name never.
- tui/render.rs: mode name in its own theme role (manual dim, plan info, auto warn, DEVIL fail+bold).
- Captain changed the brief mid-task: the meaning stays permanently, no 3s expiry.
- docs/safety.md, docs/tui.md, readme key table updated.
- Verification: cargo test all pass (footer_always_names_and_explains_the_mode, plan_toggle_leaves_the_transcript_alone).
