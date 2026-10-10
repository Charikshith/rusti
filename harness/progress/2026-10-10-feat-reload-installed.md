## Session 2026-10-10: feat-reload-installed — /reload works on an installed binary
- tui/mod.rs: Job::Reload's build moved into reload_exe(manifest_dir, event_tx); a missing
  MANIFEST_DIR/Cargo.toml skips cargo and relaunches std::env::current_exe() with a notice.
- Help text (main.rs), /reload row (tui/app.rs), readme command table, docs/tui.md § /reload updated.
- Verification: new test reload_without_source_relaunches_current_exe; cargo test all pass.
- Not built: self-update from /reload (out of scope).
