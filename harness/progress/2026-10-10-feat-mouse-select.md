## Session 2026-10-10: feat-mouse-select — select and copy transcript text with the mouse
- tui/render.rs: rows carry a soft-wrap flag; RenderState keeps rows/view_h/inner_w; cell(), expand(), selected(); plain(), reverse(), sel_cols(), styled().
- tui/app.rs: App.sel/click/dragged; Down/Drag/Up(Left) handling with edge auto-scroll and multi-click; any key or resize clears; copy_sel() + osc52().
- ai_core/tools.rs: clip_cmds(os) + copy_text() beside clipboard_image.
- main.rs HELP and docs/tui.md "selecting and copying" (Shift+drag for native selection).
- Verification: ./init.sh all pass, clippy clean (see harness/features/feat-mouse-select.json). Not driven with a real mouse in a live TUI.
