## 2026-10-10 - feat-bughunt-hangs
- Surprise: `taskkill /T` reported SUCCESS and still left the inner `sh` and `sleep` of a Git Bash command alive. Win32_Process showed their parent pid belonged to an exited process (an MSYS exec); a job object caught them all. Lesson saved.
- `--tui`'s plain fallback had been noted as a hang in the 2026-10-01 journal and left alone; its only fix was removal.
- A `let`-bound `#[cfg]` was the safe form for `Tree::of`: a `#[cfg]` on a tail block expression could not be compiled for Unix here (only the msvc target is installed).
- Do differently: write the tree-kill test with a grandchild that writes a marker file from the start. Killing the shell alone "passes" when the shell itself was going to do the write.
