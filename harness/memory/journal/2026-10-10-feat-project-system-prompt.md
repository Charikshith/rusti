## 2026-10-10 - feat-project-system-prompt
- The hook also unescapes `\n` inside python heredocs; edit scripts with Rust string escapes must be written to a file.
- trusted() is a process global no test toggles; the untrusted case is tested via prompt_dirs() under the default.
