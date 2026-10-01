## 2026-10-01 - feat-image-input
- The :20128 proxy now passes image parts for cmc/ vision models; the memory lesson's 9router 0.4.80 strip is stale for them, while mimo-v2.5-pro (the default) still sees nothing.
- red/green/blue is the order a model guesses anyway, so the rusti run used a swapped blue/red/green image.
- tui::app is a private module, so ai_core cannot call at_token; the scan runs in the TUI thread and run_agent takes the paths.
- Windows has a System32 `convert.exe` (disk tool); the ImageMagick fallback is behind the non-Windows cfg for that reason.
