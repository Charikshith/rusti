## Session 2026-10-10: feat-shifttab-modes — Shift+Tab permission modes
- ai_core/mod.rs: Mode enum + MODE, mode()/set_mode()/set_plan()/plan_mode(); RISKY/FETCHERS/WRITERS tables,
  risky(), auto_allows(), runs_unasked(); permitted() takes args; gate() hands args to the permit closure.
- ai_core/tools.rs: YOLO removed, guard() reads Mode::Devil. main.rs: --yolo sets devil; help text updated.
- tui/app.rs: BackTab -> cycle_mode + mode_notice; footer_right takes the mode; /plan menu text. render.rs: DEVIL in warn colour.
- main.rs start_mode() picks devil for --yolo / RUSTI_YOLO=1; readme Use, TUI and safety (mode table) updated.
- Verification: cargo test all pass; --self-test OK (see harness/features/feat-shifttab-modes.json).
