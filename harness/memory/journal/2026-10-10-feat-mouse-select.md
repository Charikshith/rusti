## 2026-10-10 - feat-mouse-select
- Python edit scripts via heredoc failed again in the Bash hook (quote/continuation mangling); writing the script to the scratchpad first worked.
- clip.exe keeps a UTF-16 BOM as a character on the clipboard; send UTF-16LE without one.
- colorize_row rewrites row text (drops the "│ " sentinel), so selection columns must be measured on the styled-then-stripped row, not the source row.
