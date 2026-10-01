## Session 2026-10-01: feat-image-input — image input (F03)
- tools.rs: load_image (vision flag, MAX_IMAGE 4.5 MB base64, fit ladder, platform shrink, image_dims,
  size_text); read_file images go through it; clipboard_image returns every copied image file.
- session.rs Entry.images; ai_core attach() on submit + run_agent images param; tui::app image_refs and
  Alt+V typing @ references; config ModelProfile.vision; readme, rusti-vs-pi rows, open-work follow-ups.
- Verification: 3a live (see harness/features/feat-image-input.json); 5 new tests, 1 extended; ./init.sh passed.
- Not built: inline display, images kill switch, MCP image normalising, two read_file images per batch (open-work).
