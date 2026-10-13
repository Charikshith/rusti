## 2026-10-10 - feat-bughunt-cosmetic
- Source files are CRLF in the working tree (autocrlf=true); a python heredoc replace with LF strings matched nothing, and Git Bash `sed -i` rewrote them as LF. Use the Edit tool.
- A helper named `cols` collided with `let cols` locals in draw and render_table; named it `columns`.
- input_window drew the caret at the window's end and printed the whole rest of the line after it once a line overflowed, not only the F1 panic.
- Contrast of the 16 ANSI colours depends on the terminal palette; measured as Campbell to match the report.
