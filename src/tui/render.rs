// Alternate-screen renderer with differential updates (pi-style).
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

/// Which slice of the wrapped transcript is on screen, as (blank rows above,
/// first line, one past the last).
///
/// The transcript rests ON the input the way a shell does: when it is shorter
/// than the viewport the blank space goes above it, not below, so the first
/// message appears just over the prompt and later ones rise from the bottom.
/// scroll_up pins lines above the fold and cannot walk off the first line.
pub fn transcript_window(total: usize, height: usize, scroll_up: usize) -> (usize, usize, usize) {
    let scroll = scroll_up.min(total.saturating_sub(height));
    let end = total - scroll;
    let start = end.saturating_sub(height);
    (height - (end - start), start, end)
}

/// The rows to draw: a row marked HIDDEN (a successful tool's output) is only
/// drawn once Ctrl+O is on, and the marker is stripped either way — it must
/// never reach the terminal. One filter point, so nothing else in the TUI has
/// to know hidden rows exist.
pub fn visible(lines: &[String], expand: bool) -> impl Iterator<Item = &str> {
    lines.iter().filter_map(move |l| match l.strip_prefix(app::HIDDEN) {
        Some(body) => expand.then_some(body),
        None => Some(l.as_str()),
    })
}

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

    // bottom is pinned: blank spacer, panel (picker or slash menu), input, status.
    // The input is as tall as the draft — Shift+Enter puts newlines in it.
    let panel = panel_rows(app, w);
    // While a picker is open the filter IS the draft: it types at the prompt,
    // not in the picker header. The real draft is hidden until Esc restores it.
    let (draft, caret) = match &app.pick {
        Some(p) => (p.filter.as_str(), p.filter.chars().count()),
        None => (app.input.as_str(), app.cursor),
    };
    let input = input_rows(draft, caret, w.saturating_sub(3));
    let bottom_rows = 2 + input.len() + panel.len();
    let transcript_h = h.saturating_sub(bottom_rows);

    // (style, row). The style is read from the logical line ONCE and carried to
    // every row it wraps into — a marker only exists on the first row, so styling
    // each row on its own left the rest of a wrapped block looking like plain text.
    let mut all: Vec<(u8, String)> = Vec::new();
    let push_wrapped = |l: &str, all: &mut Vec<(u8, String)>| {
        let st = line_style(l);
        if st != b'm' {
            all.extend(word_wrap(l, inner_w).into_iter().map(|r| (st, r)));
            return;
        }
        // model prose: markdown, one source line at a time so ``` fences keep
        // their state and code never gets reflowed
        let mut fence = false;
        for src in l.split('\n') {
            if src.trim_start().starts_with("```") {
                fence = !fence;
                continue; // the fence itself is not drawn; the body is coloured
            }
            if fence {
                all.push((b'm', format!("  {FENCED}{}{RESET}", truncate_str(src, inner_w))));
                continue;
            }
            let (cs, runs) = md_line(src);
            for (x, y) in wrap_ranges(&cs, inner_w) {
                all.push((b'm', md_row(&cs, &runs, x, y)));
            }
        }
    };
    for l in visible(&app.lines, app.expand) {
        push_wrapped(l, &mut all);
    }
    if !app.current.is_empty() {
        push_wrapped(&app.current.clone(), &mut all);
    }
    let (pad, start, end) = transcript_window(all.len(), transcript_h, app.scroll_up);

    // ── compose frame: one string per screen row ──
    let mut frame: Vec<String> = Vec::with_capacity(h);
    // ToolStart stores a literal "⠋"; the live frame is substituted at draw
    // time so the glyph actually turns. Storing the animation would mean
    // rewriting app.lines every tick just to move one character.
    let spin = SPINNER[app.spinner % SPINNER.len()];

    for _ in 0..pad {
        frame.push(String::new());
    }
    for (st, row) in &all[start..end] {
        // markdown rows are already wrapped and styled; truncating would cut
        // an escape sequence in half
        frame.push(if *st == b'm' { row.clone() } else { colorize_row(*st, &truncate_str(row, inner_w), spin) });
    }

    frame.push(String::new()); // blank spacer

    frame.extend(panel);

    let input_len = input.len();
    frame.extend(input);

    // status line: session/model/branch left, spinner + hint right
    let armed = app.armed();
    let hint = if armed {
        "press ctrl+c again to exit".to_string()
    } else if let Some(n) = app.notice() {
        format!("ℹ {n}")
    } else if !app.done {
        format!("{spin} working…")
    } else if app.fresh || app.scroll_up > 0 {
        // onboarding, not status: shown on a fresh prompt, and again when the
        // transcript is scrolled off the bottom (you're looking for the way out)
        "ctrl+c twice to quit".to_string()
    } else {
        String::new()
    };
    let footer = app::footer_right(
        &app.footer, crate::ai_core::plan_mode(), &app.session, &app.model, &app.branch,
        app.sess_tok, app.turn_ctx, crate::ai_core::context_limit(),
    );
    // the session info gets the width; the hint is short and yields to it
    let info = truncate_str(&footer, w.saturating_sub(hint.chars().count() + 3));
    let mut srow = format!("\x1b[2m{info}\x1b[0m");
    if !hint.is_empty() {
        // draw_line clears to end of line, so this padding only pushes the hint
        // to the right edge — it is not there to erase the previous frame
        let used = info.chars().count() + 1 + hint.chars().count();
        if used < w {
            srow.extend(std::iter::repeat(' ').take(w - used));
        }
        srow.push_str(&if app.done {
            format!("\x1b[2m{hint}\x1b[0m")
        } else {
            format!("\x1b[33m{spin}\x1b[0m\x1b[2m working…\x1b[0m")
        });
    }
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
    // the input block ends just above the status line; the caret sits in the
    // row holding the cursor, which is not the last one in a multi-line draft
    let (caret_line, caret_col) = caret_at(draft, caret);
    let input_top = h.saturating_sub(1 + input_len);
    let input_y = (input_top + caret_line).min(h.saturating_sub(2)) as u16;
    let input_x = (2 + caret_col).min(w.saturating_sub(1)) as u16; // after "> "
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
        let hint = match p.kind {
            app::PickKind::Session => "↑/↓ select · enter resume · type to filter · esc cancel",
            app::PickKind::Model => "↑/↓ select · enter switch · type to filter · esc cancel",
            app::PickKind::Tree => "↑/↓ select · enter branch here · type to filter · esc cancel",
            app::PickKind::Settings => "↑/↓ select · enter toggle · esc close",
            app::PickKind::Mcp => "↑/↓ select · enter connect/disconnect · esc close",
        };
        let vis = p.visible();
        let count = if p.filter.is_empty() {
            format!("({})", p.rows.len())
        } else {
            format!("({}/{})", vis.len(), p.rows.len())
        };
        let mut out = vec![sel_row(&format!("  {} {count}", p.title), false)];
        if vis.is_empty() {
            out.push(sel_row("  nothing matches", false));
        }
        for (i, (label, _)) in vis.iter().enumerate().skip(p.top).take(app::PICK_ROWS) {
            out.push(sel_row(&format!("{}{label}", if i == p.idx { "▸ " } else { "  " }), i == p.idx));
        }
        if vis.len() > app::PICK_ROWS {
            out.push(sel_row(&format!("  {hint}  · {}/{}", p.idx + 1, vis.len()), false));
        } else {
            out.push(sel_row(&format!("  {hint}"), false));
        }
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
/// Tool output text. 256-colour 245 (#8a8a8a) for the same reason THINK avoids
/// ESC[2m — and a shade under THINK so reasoning still reads as the brighter of
/// the two greys, with the italic carrying the rest of the difference.
const DIM: &str = "\x1b[38;5;245m";
const RESET: &str = "\x1b[0m";

/// Rows that carry a status marker; these keep the per-row glyph colouring.
const MARKERS: &[&str] = &["  ⠋ ", "  ✓ ", "  ✗ ", "  ⚠ ", "  ℹ "];

/// A user query: leading digits then the caret.
fn is_user_line(l: &str) -> bool {
    let n = l.bytes().take_while(u8::is_ascii_digit).count();
    n > 0 && l[n..].starts_with("› ")
}

/// Styles that must cover a whole wrapped block, not just the row holding the
/// marker. b'k' keeps the old per-row glyph colouring; b'm' is prose from the
/// model, which gets the markdown pass.
fn line_style(l: &str) -> u8 {
    if l.starts_with("  │ ") {
        b't' // reasoning_content
    } else if l.starts_with("  · ") {
        b's' // turn stats
    } else if is_user_line(l) {
        b'u' // split from b'k': the query stays bright, tool rows do not
    } else if MARKERS.iter().any(|p| l.starts_with(p)) {
        b'k'
    } else {
        b'm'
    }
}

fn colorize_row(style: u8, s: &str, spin: char) -> String {
    match style {
        // the │ is an internal sentinel for line_style, never drawn: italic grey
        // carries the block on its own, the way the reference terminals do it
        b't' => return format!("  {THINK}{}{RESET}", s.strip_prefix("  │ ").unwrap_or(s)),
        b's' => return format!("\x1b[2m{s}\x1b[0m"),
        // the user's own query: cyan "N›" marker so it's easy to find when
        // scrolling back, text at full brightness like the model's answer.
        // Wrapped rows carry no marker and must stay bright too — that's the
        // whole reason this isn't b'k'.
        b'u' => {
            let i = s.bytes().take_while(u8::is_ascii_digit).count();
            return if i > 0 && s[i..].starts_with("› ") {
                format!("\x1b[36m{}›\x1b[0m{}", &s[..i], &s[i + '›'.len_utf8()..])
            } else {
                format!("  {s}")
            };
        }
        _ => {}
    }
    // b'k': tool rows. Coloured glyph, dim text, so a wall of tool output
    // recedes behind the prose instead of competing with it.
    if let Some(rest) = s.strip_prefix("  ⠋ ") {
        // the one row that is still happening: turning spinner + bold text, so
        // "running" and "finished a while ago" cannot be confused at a glance.
        // ToolEnd rewrites this row to ✓/✗, which lands in the dim branches
        // below — that swap is what un-bolds it.
        format!("  \x1b[33m{spin}\x1b[0m {BOLD}{rest}{RESET}")
    } else if let Some(rest) = s.strip_prefix("  ✓ ") {
        format!("  \x1b[32m✓\x1b[0m {DIM}{rest}{RESET}")
    } else if let Some(rest) = s.strip_prefix("  ✗ ") {
        format!("  \x1b[31m✗\x1b[0m {DIM}{rest}{RESET}")
    } else if let Some(rest) = s.strip_prefix("  ⚠ ") {
        format!("  \x1b[33m⚠\x1b[0m {DIM}{rest}{RESET}")
    } else if let Some(rest) = s.strip_prefix("  ℹ ") {
        format!("  \x1b[34mℹ\x1b[0m {DIM}{rest}{RESET}")
    } else {
        format!("  {DIM}{s}{RESET}") // wrapped continuation of a tool row
    }
}

// ── markdown for model prose ────────────────────────────────────────────────
// Deliberately small: headings, **bold**, `code`, - bullets, ``` fences. No
// parser crate. Markers become style runs over the *visible* text, so wrapping
// measures real columns instead of counting asterisks it is about to delete.

const BOLD: &str = "\x1b[1m";
const CODE: &str = "\x1b[36m"; // inline `code`
const FENCED: &str = "\x1b[38;5;180m"; // fenced code block body

/// One source line as visible chars plus the style runs over them (char ranges).
fn md_line(src: &str) -> (Vec<char>, Vec<(usize, usize, &'static str)>) {
    let mut runs: Vec<(usize, usize, &'static str)> = Vec::new();
    let trimmed = src.trim_start();
    let lead = &src[..src.len() - trimmed.len()];
    let mut out: Vec<char> = lead.chars().collect();

    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    let heading = (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ');
    let body: &str = if heading {
        &trimmed[hashes + 1..]
    } else if let Some(r) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
        out.extend(['•', ' ']);
        r
    } else {
        trimmed
    };
    let body_start = out.len();

    let cs: Vec<char> = body.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        // **bold**
        if cs[i] == '*' && cs.get(i + 1) == Some(&'*') {
            if let Some(end) =
                (i + 2..cs.len().saturating_sub(1)).find(|&j| cs[j] == '*' && cs[j + 1] == '*')
            {
                let s = out.len();
                out.extend_from_slice(&cs[i + 2..end]);
                runs.push((s, out.len(), BOLD));
                i = end + 2;
                continue;
            }
        }
        // `code`
        if cs[i] == '`' {
            if let Some(end) = (i + 1..cs.len()).find(|&j| cs[j] == '`') {
                let s = out.len();
                out.extend_from_slice(&cs[i + 1..end]);
                runs.push((s, out.len(), CODE));
                i = end + 1;
                continue;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    if heading {
        runs.push((body_start, out.len(), BOLD));
    }
    (out, runs)
}

/// Greedy word wrap over chars, returning each row's range. Ranges (not strings)
/// keep the style runs addressable after wrapping.
fn wrap_ranges(cs: &[char], width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < cs.len() {
        if cs.len() - start <= width {
            rows.push((start, cs.len()));
            break;
        }
        let hard = start + width;
        let brk = (start..hard).rev().find(|&j| cs[j] == ' ').unwrap_or(start);
        let end = if brk == start { hard } else { brk };
        rows.push((start, end));
        start = end;
        while start < cs.len() && cs[start] == ' ' {
            start += 1;
        }
    }
    if rows.is_empty() {
        rows.push((0, 0)); // a blank source line is still a blank row
    }
    rows
}

/// Render chars [a,b) with whatever runs cover them. Styles are recomputed per
/// char so a `code` span nested inside a bold heading restores the bold after it.
fn md_row(cs: &[char], runs: &[(usize, usize, &'static str)], a: usize, b: usize) -> String {
    let mut s = String::from("  ");
    let mut cur = String::new();
    for i in a..b {
        let want: String =
            runs.iter().filter(|(x, y, _)| i >= *x && i < *y).map(|(_, _, c)| *c).collect();
        if want != cur {
            s.push_str(RESET);
            s.push_str(&want);
            cur = want;
        }
        s.push(cs[i]);
    }
    if !cur.is_empty() {
        s.push_str(RESET);
    }
    s
}

/// Input window around the cursor for a row that overflows: returns the
/// visible (before-cursor, after-cursor) slices; the block cursor sits
/// between them.
/// (line, column) of the cursor inside a multi-line draft.
pub fn caret_at(input: &str, cursor: usize) -> (usize, usize) {
    let mut seen = 0;
    for (i, line) in input.split('\n').enumerate() {
        let n = line.chars().count();
        if cursor <= seen + n {
            return (i, cursor - seen);
        }
        seen += n + 1;
    }
    (0, 0)
}

/// The input box, one screen row per line of the draft. The caret sits in the
/// row holding the cursor; continuation rows line up under the "> ".
fn input_rows(input: &str, cursor: usize, avail: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = 0; // chars before this line, the '\n's included
    for (i, line) in input.split('\n').enumerate() {
        let n = line.chars().count();
        let prefix = if i == 0 { "\x1b[36m> \x1b[0m" } else { "  " };
        // exactly one row owns the cursor: the next line starts at seen + n + 1,
        // so the ranges [seen, seen+n] never overlap
        out.push(if cursor >= seen && cursor <= seen + n {
            let (pre, suf) = input_window(line, cursor - seen, avail);
            format!("{prefix}{pre}▌{suf}")
        } else {
            format!("{prefix}{}", truncate_str(line, avail))
        });
        seen += n + 1;
    }
    out
}

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
            word_wrap(line, 80).iter().map(|r| colorize_row(st, &truncate_str(r, 80), '⠋')).collect()
        };
        let first = |line: &str| rows(line).remove(0);
        // a running tool must be impossible to mistake for a finished one:
        // the glyph turns with the frame, and the text is bold, not dim
        let run = colorize_row(line_style("  ⠋ pwd"), "  ⠋ pwd", '⠹');
        assert!(run.contains('⠹'), "spinner must show the live frame, not the stored ⠋: {run:?}");
        assert!(run.contains(BOLD) && !run.contains(DIM), "running tool must be bold: {run:?}");
        assert!(first("  ✓ Cargo.toml  0ms").contains(DIM), "a finished tool un-bolds to dim");
        assert!(!first("  ✓ Cargo.toml  0ms").contains(BOLD), "a finished tool is not bold");
        assert!(first("  ✓ Cargo.toml  0ms").contains("\x1b[32m✓"), "tool ok must be green");
        assert!(first("  ✗ edit failed").contains("\x1b[31m✗"), "tool fail must be red");
        assert!(first("  ⠋ cargo build").contains("\x1b[33m⠋"), "running must be yellow");
        assert!(first("  ⚠ interrupted").contains("\x1b[33m⚠"), "warn must be yellow");
        assert!(first("  ℹ renamed").contains("\x1b[34mℹ"), "info must be blue");
        assert!(first("  · 32 tok").starts_with("\x1b[2m"), "stats must be dim");
        // tool text is dim, so a wall of output recedes behind the prose
        assert!(first("  ✓ Cargo.toml  0ms").contains(DIM), "tool text must be dim");
        // the user's query keeps a cyan marker and BRIGHT text, on every row —
        // wrapped rows carry no marker, so they must not fall into the dim branch
        let q = rows("9› hello there, this is a long query that will certainly wrap past eighty columns");
        assert!(q[0].starts_with("\x1b[36m9›\x1b[0m"), "query marker must be cyan: {:?}", q[0]);
        assert!(q.len() > 1 && q.iter().all(|r| !r.contains(DIM)), "query must stay bright: {q:?}");
        assert_eq!(first("12› hi there"), "\x1b[36m12›\x1b[0m hi there");
    }

    /// Markdown markers must be gone from the visible text, replaced by styling.
    #[test]
    fn markdown_markers_become_styling_not_literal_characters() {
        let plain = |src: &str| md_line(src).0.iter().collect::<String>();
        assert_eq!(plain("### **File & Text Manipulation**"), "File & Text Manipulation");
        assert_eq!(plain("- `read_file`: Read a file's contents."), "• read_file: Read a file's contents.");
        assert_eq!(plain("* bullet"), "• bullet");
        assert_eq!(plain("plain sentence"), "plain sentence");
        // an unmatched marker is literal text, not a swallowed rest-of-line
        assert_eq!(plain("2 * 3 and a lone ` tick"), "2 * 3 and a lone ` tick");
        assert_eq!(plain("#nothashheading"), "#nothashheading");

        // the runs actually land on the right characters
        let (cs, runs) = md_line("- `grep`: Search **fast**.");
        let at = |i: usize| runs.iter().find(|(x, y, _)| i >= *x && i < *y).map(|(_, _, c)| *c);
        let s: String = cs.iter().collect();
        assert_eq!(at(s.find("grep").unwrap()), Some(CODE));
        assert_eq!(at(s.find("fast").unwrap()), Some(BOLD));
        assert_eq!(at(s.find("Search").unwrap()), None);

        // a heading keeps bold across an inline code span nested inside it
        let (cs, runs) = md_line("## use `grep` now");
        let row = md_row(&cs, &runs, 0, cs.len());
        assert!(row.contains(CODE) && row.matches(BOLD).count() >= 2, "{row:?}");
    }

    #[test]
    fn markdown_wrapping_measures_visible_text_not_markers() {
        // width is charged against the text the user sees, not the markers
        let (cs, _) = md_line("**aaaa** bbbb cccc dddd");
        let rows = wrap_ranges(&cs, 10);
        assert!(rows.iter().all(|(x, y)| y - x <= 10), "{rows:?}");
        assert_eq!(rows.iter().map(|(x, y)| cs[*x..*y].iter().collect::<String>()).collect::<Vec<_>>(),
                   vec!["aaaa bbbb", "cccc dddd"]);
        // a word longer than the width is hard-cut rather than dropped
        let long: Vec<char> = "supercalifragilistic".chars().collect();
        assert_eq!(wrap_ranges(&long, 5).len(), 4);
        // blank source line still produces one row
        assert_eq!(wrap_ranges(&[], 10), vec![(0, 0)]);
    }

    /// The regression behind "I can't see any difference": reasoning wraps over
    /// many rows and only the first carried the marker, so the rest rendered as
    /// plain text — indistinguishable from the answer that follows.
    #[test]
    fn every_row_of_a_wrapped_reasoning_block_is_styled() {
        let long = "  │ ".to_string() + &"thinking ".repeat(60);
        let st = line_style(&long);
        let rows: Vec<String> =
            word_wrap(&long, 40).iter().map(|r| colorize_row(st, &truncate_str(r, 40), '⠋')).collect();
        assert!(rows.len() > 3, "expected a wrapped block, got {}", rows.len());
        assert!(rows.iter().all(|r| r.contains(THINK)), "every row must be italic grey: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains('│')), "the sentinel bar is never drawn");
        // the answer is the plain one, so the two can never be confused
        assert!(!colorize_row(line_style("an answer"), "an answer", '⠋').contains(THINK));
        // paragraph breaks inside a block survive and stay styled
        let two = "  │ first thought\n\nsecond thought";
        let st = line_style(two);
        assert!(word_wrap(two, 40).iter().all(|r| colorize_row(st, r, '⠋').contains(THINK)));
    }

    /// Ctrl+O reveals a successful tool's output. The rows are always in
    /// app.lines; only their visibility changes — and the marker itself must
    /// never reach the terminal, where it would print as a control character.
    #[test]
    fn hidden_rows_appear_only_when_expanded() {
        let lines = vec![
            "  ✓ cargo test".to_string(),
            format!("{}  · 27 passed", app::HIDDEN),
            "  ✗ cargo build".to_string(),
            "  · error: no main".to_string(), // a failure tail is never hidden
        ];

        let folded: Vec<&str> = visible(&lines, false).collect();
        assert_eq!(folded, vec!["  ✓ cargo test", "  ✗ cargo build", "  · error: no main"]);

        let open: Vec<&str> = visible(&lines, true).collect();
        assert_eq!(open.len(), 4);
        assert_eq!(open[1], "  · 27 passed", "the marker must be stripped, not drawn");
        assert!(!open.iter().any(|r| r.contains(app::HIDDEN)));
    }
}
