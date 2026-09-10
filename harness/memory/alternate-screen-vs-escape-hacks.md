# A bottom-pinned TUI on the main screen always leaves a gap — use the alternate screen

A main-screen TUI that pins its input/status panel to the terminal's bottom row leaves a hole
between the shell's last line and the panel, and leaves its transcript in the shell's scrollback
on exit. No amount of quit-time clearing fixes this: ESC[2J alone leaves the gap, ESC[3J removes
the gap but eats the shell history the user wants to keep.

**Why:** the primary buffer is shared with the shell; anything drawn there either coexists with
the shell's content (gap, leftovers) or destroys it (3J). The alternate screen is the terminal's
built-in mechanism for exactly this: a scratch canvas that restores the primary buffer byte-for-byte
on leave. Switching to it deleted all the escape-sequence cleanup at quit — `EnterAlternateScreen`
on start, `LeaveAlternateScreen` on exit, farewell prints under the launch line, history intact.

**Rule:** when a cosmetic terminal fix needs escalating escape-sequence hacks, question the
screen-buffer model instead of tuning the sequences. (vim/htop/less all use the alternate screen.)
