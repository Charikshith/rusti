// Main-screen renderer with differential updates (pi-style).
// First frame: full draw. Subsequent frames: only changed lines.
// Synchronized output (CSI 2026) for atomic flicker-free updates.
// No box — plain terminal lines like pi. Input + status pinned at bottom.

use std::io::{self, Write, stdout};

use crossterm::{
    execute,
    terminal::{self, Clear, ClearType},
};

use super::app::{self, App, CMDS, MENU_ROWS};
use super::{goto, word_wrap, SYNC_BEGIN, SYNC_END};

/// Braille spinner frames (~20fps at the 50ms poll rate).
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// State carried between frames for differential rendering.
pub struct RenderState {
    prev_lines: Vec<String>,
    prev_rows: usize,
}

impl RenderState {
    pub fn new() -> Self {
        Self { prev_lines: Vec::new(), prev_rows: 0 }
    }
}

/// Render one frame. Diff against previous frame — only changed lines are redrawn.
pub fn draw(app: &App, state: &mut RenderState) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let w = cols as usize;
    let h = rows as usize;
    let inner_w = w.saturating_sub(2); // transcript content width (2-space indent)

    // bottom is pinned: blank spacer, panel (picker or slash menu), input, status
    let panel = panel_rows(app, w);
    let bottom_rows = 3 + panel.len();
    let transcript_h = h.saturating_sub(bottom_rows);

    // (style, row). The style is read from the logical line ONCE and carried to
    // every row it wraps into — a marker only exists on the first row, so styling
    // each row on its own left the rest of a wrapped block looking like plain text.
    let mut all: Vec<(u8, String)> = Vec::new();
    let push_wrapped = |l: &str, all: &mut Vec<(u8, String)>| {
        let st = line_style(l);
        all.extend(word_wrap(l, inner_w).into_iter().map(|r| (st, r)));
    };
    for l in &app.lines {
        push_wrapped(l, &mut all);
    }
    if !app.current.is_empty() {
        push_wrapped(&app.current.clone(), &mut all);
    }
    // scroll: 0 = follow bottom; scroll_up = lines pinned above it
    let view = all.len().saturating_sub(transcript_h + app.scroll_up);

    // ── compose frame: one string per screen row ──
    let mut frame: Vec<String> = Vec::with_capacity(h);

    for i in 0..transcript_h {
        let li = view + i;
        let content = if li < all.len() {
            colorize_row(all[li].0, &truncate_str(&all[li].1, inner_w))
        } else {
            String::new()
        };
        frame.push(content);
    }

    frame.push(String::new()); // blank spacer

    frame.extend(panel);

    // input line with a block cursor
    let avail = w.saturating_sub(3);
    let (pre, suf) = input_window(&app.input, app.cursor, avail);
    frame.push(format!("\x1b[36m> \x1b[0m{pre}▌{suf}"));

    // status line: spinner + hint left, model right
    let spin = SPINNER[app.spinner % SPINNER.len()];
    let armed = app.armed();
    let left_plain = if armed {
        "press ctrl+c again to exit".to_string()
    } else if !app.done {
        format!("{spin} working…")
    } else if app.fresh || app.scroll_up > 0 {
        // onboarding, not status: shown on a fresh prompt, and again when the
        // transcript is scrolled off the bottom (you're looking for the way out)
        "ctrl+c twice to quit".to_string()
    } else {
        String::new()
    };
    let right = if app.session.is_empty() {
        app.model.clone()
    } else {
        format!("{} · {}", app.session, app.model)
    };
    let model = truncate_str(&right, w.saturating_sub(left_plain.chars().count() + 3));
    let used = left_plain.chars().count() + 1 + model.chars().count();
    let mut srow = if app.done {
        format!("\x1b[2m{left_plain}\x1b[0m")
    } else {
        format!("\x1b[33m{spin}\x1b[0m\x1b[2m working…\x1b[0m")
    };
    if used < w {
        srow.extend(std::iter::repeat(' ').take(w - used));
    }
    srow.push_str(&format!("\x1b[2m{model}\x1b[0m"));
    frame.push(srow);

    // ── differential draw ──
    let mut out = stdout();
    out.write_all(SYNC_BEGIN.as_bytes())?;

    let resized = h != state.prev_rows;

    // if terminal resized or first frame, clear and redraw everything
    if resized || state.prev_lines.is_empty() {
        let _ = execute!(out, Clear(ClearType::All));
        goto(&mut out, 0, 0);
        for (i, line) in frame.iter().enumerate() {
            goto(&mut out, 0, i as u16);
            draw_line(&mut out, line)?;
        }
    } else {
        // only redraw changed lines
        let max = frame.len().max(state.prev_lines.len());
        for i in 0..max {
            let new = frame.get(i).map(|s| s.as_str()).unwrap_or("");
            let old = state.prev_lines.get(i).map(|s| s.as_str()).unwrap_or("");
            if new != old {
                goto(&mut out, 0, i as u16);
                draw_line(&mut out, new)?;
            }
        }
        // clear any leftover lines from previous taller frame
        for i in frame.len()..state.prev_lines.len() {
            goto(&mut out, 0, i as u16);
            let _ = execute!(out, Clear(ClearType::UntilNewLine));
        }
    }

    // ── position cursor at input ──
    let input_y = h.saturating_sub(2) as u16; // input is second from bottom (status last)
    let input_x = (2 + pre.chars().count()).min(w.saturating_sub(1)) as u16; // after "> "
    goto(&mut out, input_x, input_y);

    out.write_all(SYNC_END.as_bytes())?;
    out.flush()?;

    state.prev_lines = frame;
    state.prev_rows = h;
    Ok(())
}

/// The row block between transcript and input: the open session picker, or
/// the slash-command menu, or nothing. Coloured whole-line — cyan for the
/// selection, dim for the rest — like the tool rows.
fn panel_rows(app: &App, w: usize) -> Vec<String> {
    let sel_row = |s: &str, sel: bool| {
        let s = truncate_str(s, w);
        if sel { format!("[36m{s}[0m") } else { format!("[2m{s}[0m") }
    };

    if let Some(p) = &app.pick {
        let mut out = vec![sel_row(&format!("  {} ({})", p.title, p.rows.len()), false)];
        for (i, (label, _)) in p.rows.iter().enumerate() {
            out.push(sel_row(&format!("{}{label}", if i == p.idx { "▸ " } else { "  " }), i == p.idx));
        }
        out.push(sel_row("  ↑/↓ select · enter resume · esc cancel", false));
        return out;
    }

    let menu = app::menu_items(app);
    if menu.is_empty() {
        return Vec::new();
    }
    let pad = CMDS.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out: Vec<String> = menu
        .iter()
        .enumerate()
        .skip(app.menu_top)
        .take(MENU_ROWS)
        .map(|(i, c)| {
            let row = format!(
                "{}{:pad$}  {}{}",
                if i == app.menu_idx { "▸ " } else { "  " },
                c.name,
                c.desc,
                if c.soon { "  · soon" } else { "" },
            );
            sel_row(&row, i == app.menu_idx)
        })
        .collect();
    if menu.len() > MENU_ROWS {
        out.push(sel_row(
            &format!("  ↑/↓ {}/{}  · tab completes · enter runs", app.menu_idx + 1, menu.len()),
            false,
        ));
    }
    out
}

/// Write a line and clear to end of line (wipes stale trailing chars).
fn draw_line(out: &mut impl Write, line: &str) -> io::Result<()> {
    write!(out, "{line}")?;
    let _ = execute!(out, Clear(ClearType::UntilNewLine));
    Ok(())
}

fn truncate_str(s: &str, max: usize) -> String {
    if max == 0 { return String::new(); }
    s.chars().take(max).collect()
}

/// One transcript row, colored by kind: status symbol colored, text plain.
/// User lines ("N› text") are flush-left with a dim number + cyan caret;
/// everything else gets the 2-space indent.
/// Reasoning: italic light grey, the whole block. 256-colour 249 (#b2b2b2) rather
/// than ESC[2m — Windows Terminal renders dim as barely-darker, which is what made
/// reasoning and answer text look identical.
const THINK: &str = "\x1b[3;38;5;249m";
const RESET: &str = "\x1b[0m";

/// Styles that must cover a whole wrapped block, not just the row holding the
/// marker. b' ' means "decide per row", which is right for the short marker lines.
fn line_style(l: &str) -> u8 {
    if l.starts_with("  │ ") {
        b't' // reasoning_content
    } else if l.starts_with("  · ") {
        b's' // turn stats
    } else {
        b' '
    }
}

fn colorize_row(style: u8, s: &str) -> String {
    match style {
        // the │ is an internal sentinel for line_style, never drawn: italic grey
        // carries the block on its own, the way the reference terminals do it
        b't' => return format!("  {THINK}{}{RESET}", s.strip_prefix("  │ ").unwrap_or(s)),
        b's' => return format!("\x1b[2m{s}\x1b[0m"),
        _ => {}
    }
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && s[i..].starts_with("› ") {
        // the user's own query: plain white, flush left, no accent colours.
        // (the old form also ate the space after ›, printing "1›hello")
        s.to_string()
    } else if let Some(rest) = s.strip_prefix("  ⠋ ") {
        format!("  \x1b[33m⠋\x1b[0m {rest}")
    } else if let Some(rest) = s.strip_prefix("  ✓ ") {
        format!("  \x1b[32m✓\x1b[0m {rest}")
    } else if let Some(rest) = s.strip_prefix("  ✗ ") {
        format!("  \x1b[31m✗\x1b[0m {rest}")
    } else if let Some(rest) = s.strip_prefix("  ⚠ ") {
        format!("  \x1b[33m⚠\x1b[0m {rest}")
    } else if let Some(rest) = s.strip_prefix("  ℹ ") {
        format!("  \x1b[34mℹ\x1b[0m {rest}")
    } else {
        format!("  {s}")
    }
}

/// Input window around the cursor for a row that overflows: returns the
/// visible (before-cursor, after-cursor) slices; the block cursor sits
/// between them.
fn input_window(s: &str, c: usize, max: usize) -> (String, String) {
    if max == 0 { return (String::new(), String::new()); }
    let chars: Vec<char> = s.chars().collect();
    let c = c.min(chars.len());
    if chars.len() <= max {
        return (chars[..c].iter().collect(), chars[c..].iter().collect());
    }
    let start = c.saturating_sub(max * 3 / 4).min(chars.len() - max);
    let mut pre: String = chars[start..start + max].iter().collect();
    let suf: String = chars[start + max..].iter().collect();
    if start > 0 { pre = format!("…{}", &pre[1..]); }
    (pre, suf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this pins: word_wrap used to drop the leading indent, so every
    /// marker row missed its colorize_row branch and rendered plain. Both halves
    /// must agree, so run a row through the real pipeline, not colorize_row alone.
    #[test]
    fn status_markers_keep_their_colour_through_word_wrap() {
        // every row of a logical line, the way draw() feeds them
        let rows = |line: &str| -> Vec<String> {
            let st = line_style(line);
            word_wrap(line, 80).iter().map(|r| colorize_row(st, &truncate_str(r, 80))).collect()
        };
        let first = |line: &str| rows(line).remove(0);
        assert!(first("  ✓ Cargo.toml  0ms").contains("\x1b[32m✓"), "tool ok must be green");
        assert!(first("  ✗ edit failed").contains("\x1b[31m✗"), "tool fail must be red");
        assert!(first("  ⠋ cargo build").contains("\x1b[33m⠋"), "running must be yellow");
        assert!(first("  ⚠ interrupted").contains("\x1b[33m⚠"), "warn must be yellow");
        assert!(first("  ℹ renamed").contains("\x1b[34mℹ"), "info must be blue");
        assert!(first("  · 32 tok").starts_with("\x1b[2m"), "stats must be dim");
        // the user's query stays plain white — no escapes at all — and keeps
        // the space after the caret that the old branch swallowed
        assert_eq!(first("1› hello"), "1› hello");
        assert_eq!(first("12› hi there"), "12› hi there");
    }

    /// The regression behind "I can't see any difference": reasoning wraps over
    /// many rows and only the first carried the marker, so the rest rendered as
    /// plain text — indistinguishable from the answer that follows.
    #[test]
    fn every_row_of_a_wrapped_reasoning_block_is_styled() {
        let long = "  │ ".to_string() + &"thinking ".repeat(60);
        let st = line_style(&long);
        let rows: Vec<String> =
            word_wrap(&long, 40).iter().map(|r| colorize_row(st, &truncate_str(r, 40))).collect();
        assert!(rows.len() > 3, "expected a wrapped block, got {}", rows.len());
        assert!(rows.iter().all(|r| r.contains(THINK)), "every row must be italic grey: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains('│')), "the sentinel bar is never drawn");
        // the answer is the plain one, so the two can never be confused
        assert!(!colorize_row(line_style("an answer"), "an answer").contains(THINK));
        // paragraph breaks inside a block survive and stay styled
        let two = "  │ first thought\n\nsecond thought";
        let st = line_style(two);
        assert!(word_wrap(two, 40).iter().all(|r| colorize_row(st, r).contains(THINK)));
    }
}
