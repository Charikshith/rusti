// Alternate-screen renderer with differential updates (pi-style).
// First frame: full draw. Subsequent frames: only changed lines.
// Synchronized output (CSI 2026) for atomic flicker-free updates.
// No box — plain terminal lines like pi. Status then input pinned at bottom.

use std::borrow::Cow;
use std::io::{self, Write, stdout};

use crossterm::{
    execute,
    terminal::{self, Clear, ClearType},
};

use super::app::{self, App, MENU_ROWS};
use super::theme;
use super::{goto, word_wrap_joined, SYNC_BEGIN, SYNC_END};

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
/// never reach the terminal. With Ctrl+T's hide_thinking on, a reasoning block
/// draws as its one-row fold. One filter point, so nothing else in the TUI has
/// to know hidden rows exist.
pub fn visible(lines: &[String], expand: bool, hide_thinking: bool) -> impl Iterator<Item = Cow<'_, str>> {
    lines.iter().filter_map(move |l| match l.strip_prefix(app::HIDDEN) {
        Some(body) => expand.then_some(Cow::Borrowed(body)),
        None if hide_thinking && l.starts_with(THINK) => Some(Cow::Owned(thinking_fold(l))),
        None => Some(Cow::Borrowed(l.as_str())),
    })
}

/// The sentinel every reasoning block starts with (app.rs pushes it).
const THINK: &str = "  │ ";

/// A hidden reasoning block as one row, still styled as reasoning: it says the
/// model thought, how much, and which key brings the text back.
fn thinking_fold(l: &str) -> String {
    let n = l.strip_prefix(THINK).unwrap_or(l).lines().filter(|r| !r.trim().is_empty()).count();
    format!("{THINK}thinking… ({n} line{} · ctrl+t)", if n == 1 { "" } else { "s" })
}

/// The live reasoning block while hidden: the fold plus the last two rows of
/// the thought, so you can still watch the model work. The count of rows is
/// fixed once two exist, so streaming never makes the block jump.
fn thinking_preview(l: &str, w: usize) -> Vec<String> {
    let mut rows = wrap(b't', &thinking_fold(l), w);
    // paragraph breaks and the bare sentinel are blank rows, not a preview
    let body: Vec<String> =
        wrap(b't', l, w).into_iter().filter(|r| !matches!(r.trim(), "" | "│")).collect();
    rows.extend_from_slice(&body[body.len().saturating_sub(2)..]);
    rows
}

/// State carried between frames for differential rendering.
pub struct RenderState {
    prev_lines: Vec<String>,
    prev_rows: usize,
    /// Rows the transcript can still scroll up by, as of the last frame. Only
    /// the renderer knows it — it depends on the wrapped line count and the
    /// terminal height — and the scroll keys need it, or scroll_up runs away
    /// past the top and every scroll back down is silently eaten.
    pub max_scroll: usize,
    /// The whole wrapped transcript as of the last frame, (style, row, join),
    /// with its height and width: what a mouse selection is measured against.
    /// join is Some for a row a soft wrap split off the one above, holding what
    /// the wrap dropped between them (a space, or nothing for a hard break).
    pub rows: Vec<(u8, String, Option<&'static str>)>,
    pub view_h: usize,
    pub inner_w: usize,
}

impl RenderState {
    pub fn new() -> Self {
        Self { prev_lines: Vec::new(), prev_rows: 0, max_scroll: 0, rows: Vec::new(), view_h: 0, inner_w: 0 }
    }

    /// The transcript cell under screen (x, y) as (row, column) with the
    /// transcript scrolled to scroll_up. Above the first row is its start and
    /// below the last is its end, so a drag past either edge still selects.
    pub fn cell(&self, x: u16, y: u16, scroll_up: usize) -> Option<(usize, usize)> {
        let (pad, start, end) = transcript_window(self.rows.len(), self.view_h, scroll_up);
        let y = y as usize;
        if start == end {
            None
        } else if y < pad {
            Some((start, 0))
        } else if y - pad >= end - start {
            Some((end - 1, usize::MAX))
        } else {
            let r = start + y - pad;
            Some((r, char_at(&self.text(r), x as usize)))
        }
    }

    /// Row i as the terminal shows it, without escapes.
    fn text(&self, i: usize) -> String {
        let (st, row, _) = &self.rows[i];
        plain(&styled(*st, row, self.inner_w, SPINNER[0]))
    }

    /// Double-click: the word under a cell. Triple-click: the whole logical
    /// line it sits in, every row it wrapped into.
    pub fn expand(&self, (r, c): (usize, usize), clicks: u8) -> ((usize, usize), (usize, usize)) {
        if clicks >= 3 {
            let (mut a, mut b) = (r, r);
            while a > 0 && self.rows[a].2.is_some() { a -= 1; }
            while b + 1 < self.rows.len() && self.rows[b + 1].2.is_some() { b += 1; }
            return ((a, 0), (b, usize::MAX));
        }
        let cs: Vec<char> = self.text(r).chars().collect();
        if c >= cs.len() || cs[c].is_whitespace() {
            return ((r, c), (r, c));
        }
        let (mut a, mut b) = (c, c);
        while a > 0 && !cs[a - 1].is_whitespace() { a -= 1; }
        while b + 1 < cs.len() && !cs[b + 1].is_whitespace() { b += 1; }
        ((r, a), (r, b))
    }

    /// The selected text, plain: rows a soft wrap split are joined back into
    /// their line; the gutter (its ▎ edge or narration's │ rail included) and
    /// trailing padding are dropped.
    pub fn selected(&self, (a, b): ((usize, usize), (usize, usize))) -> String {
        let (lo, hi) = (a.min(b), a.max(b));
        let mut out = String::new();
        for i in lo.0..=hi.0.min(self.rows.len().saturating_sub(1)) {
            let cs: Vec<char> = self.text(i).chars().collect();
            let (mut x, y) = sel_cols(i, lo, hi);
            if self.rows[i].0 == b'n' {
                x = x.max(4);
            } else if cs.starts_with(&[' ', ' ']) || cs.starts_with(&['▎', ' ']) {
                x = x.max(2);
            }
            let (x, y) = (x.min(cs.len()), y.min(cs.len()));
            let part: String = cs[x..y.max(x)].iter().collect();
            let part = part.trim_end();
            if i == lo.0 {
                out.push_str(part);
            } else if let Some(join) = self.rows[i].2 {
                out.push_str(join);
                out.push_str(part.trim_start());
            } else {
                out.push('\n');
                out.push_str(part);
            }
        }
        out
    }
}

/// Rows of style st that each fit w columns where colorize_row draws them.
/// ponytail: styles each candidate row to measure it; a per-style gutter
/// width if wrapping ever shows up in a profile
fn wrap(st: u8, s: &str, w: usize) -> Vec<String> {
    wrap_joined(st, s, w).into_iter().map(|r| r.0).collect()
}

/// wrap, with each row's join for a copy (see word_wrap_joined).
fn wrap_joined(st: u8, s: &str, w: usize) -> Vec<(String, Option<&'static str>)> {
    word_wrap_joined(s, w, |r| columns(&plain(&colorize_row(st, r, ' '))))
}

/// One transcript row styled for the screen. Markdown rows are already
/// wrapped and styled; truncating would cut an escape sequence in half.
fn styled(st: u8, row: &str, inner_w: usize, spin: char) -> String {
    if st == b'm' {
        return row.to_string();
    }
    let row = colorize_row(st, row, spin);
    let cut = fit(&row, 0, inner_w + 2);
    if cut.len() < row.len() { format!("{cut}{RESET}") } else { cut }
}

/// The columns [x, y) of row i inside a selection running lo..=hi (both ends
/// included, so a one-cell selection still holds that cell).
fn sel_cols(i: usize, lo: (usize, usize), hi: (usize, usize)) -> (usize, usize) {
    let x = if i == lo.0 { lo.1 } else { 0 };
    let y = if i == hi.0 { hi.1.saturating_add(1) } else { usize::MAX };
    (x, y)
}

/// The char of a plain row under screen column x, measured the way the
/// terminal lays the row out from column 0: a tab runs to the next stop of 8
/// and wide chars take two columns. Past the end, the columns beyond it count
/// on as single cells.
fn char_at(row: &str, x: usize) -> usize {
    let mut col = 0;
    let mut n = 0;
    for c in row.chars() {
        col += step(col, c);
        if col > x {
            return n;
        }
        n += 1;
    }
    n + (x - col)
}

/// Two-column chars: CJK, Hangul, fullwidth forms and the common emoji blocks.
/// ponytail: hand-rolled ranges, not the full East Asian Width table; a rarer
/// wide or zero-width char shifts the selection by a column after it.
fn wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F
        | 0x1F680..=0x1F6FF | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

/// Columns a char takes on screen.
fn width(c: char) -> usize {
    if wide(c) { 2 } else { 1 }
}

/// Columns c takes when it starts at column col: a tab runs to the next stop of 8.
fn step(col: usize, c: char) -> usize {
    if c == '\t' { 8 - col % 8 } else { width(c) }
}

/// Columns a plain row takes on screen, from column 0.
pub fn columns(s: &str) -> usize {
    s.chars().fold(0, |col, c| col + step(col, c))
}

/// Tabs as the spaces a terminal moves over, to stops of 8 from the start of
/// each line; escapes take no columns. A raw tab moves the cursor without
/// erasing, so it left the last frame's glyphs in the cells it skipped.
/// Rows keep their tabs, so a copy gets them back: only the frame expands.
fn expand_tabs(s: &str) -> Cow<'_, str> {
    if !s.contains('\t') {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    let mut col = 0;
    let mut esc = false;
    for c in s.chars() {
        match c {
            '\x1b' => {
                out.push(c);
                esc = true;
            }
            c if esc => {
                out.push(c);
                esc = !c.is_ascii_alphabetic();
            }
            '\t' => {
                let n = 8 - col % 8;
                out.push_str(&" ".repeat(n));
                col += n;
            }
            '\n' => {
                out.push(c);
                col = 0;
            }
            c => {
                out.push(c);
                col += width(c);
            }
        }
    }
    Cow::Owned(out)
}

/// What the terminal shows for a styled row: the text without CSI escapes.
pub fn plain(row: &str) -> String {
    let mut out = String::new();
    let mut esc = false;
    for c in row.chars() {
        match c {
            '\x1b' => esc = true,
            c if esc => esc = !c.is_ascii_alphabetic(),
            c => out.push(c),
        }
    }
    out
}

/// A selected row: columns [x, y) in reverse video. The row is drawn plain —
/// the highlight is what needs to be seen, not the colours under it.
fn reverse(row: &str, (x, y): (usize, usize)) -> String {
    let cs: Vec<char> = plain(row).chars().collect();
    let (x, y) = (x.min(cs.len()), y.min(cs.len()));
    if x >= y {
        return row.to_string();
    }
    let s = |r: &[char]| r.iter().collect::<String>();
    format!("{}\x1b[7m{}{RESET}{}", s(&cs[..x]), s(&cs[x..y]), s(&cs[y..]))
}

/// Render one frame. Diff against previous frame — only changed lines are redrawn.
pub fn draw(app: &App, state: &mut RenderState) -> io::Result<()> {
    let (cols, rows) = terminal::size()?;
    let w = cols as usize;
    let h = rows as usize;
    let inner_w = w.saturating_sub(2); // transcript content width (2-space indent)

    // bottom is pinned: blank spacer, panel (picker or slash menu), hint, input,
    // footer. The input is as tall as the draft — Shift+Enter puts newlines in it.
    let panel = panel_rows(app, w);
    let pending = pending_rows(&crate::ai_core::steers(), &app.follow, w);
    // While a picker is open the filter IS the draft: it types at the prompt,
    // not in the picker header. The real draft is hidden until Esc restores it.
    let (draft, caret) = match &app.pick {
        Some(p) => (p.filter.as_str(), p.filter.chars().count()),
        None => (app.input.as_str(), app.cursor),
    };
    let input = input_rows(draft, caret, w.saturating_sub(3));
    let bottom_rows = 3 + input.len() + panel.len() + pending.len();
    let transcript_h = h.saturating_sub(bottom_rows);

    // (style, row, join). The style is read from the logical line ONCE and carried to
    // every row it wraps into — a marker only exists on the first row, so styling
    // each row on its own left the rest of a wrapped block looking like plain text.
    // join marks a row that continues the one above, so a copy can rejoin them.
    let mut all: Vec<(u8, String, Option<&'static str>)> = Vec::new();
    let push_wrapped = |l: &str, all: &mut Vec<(u8, String, Option<&'static str>)>| {
        let st = line_style(l);
        if st != b'm' {
            for src in l.split('\n') {
                all.extend(wrap(st, src, inner_w).into_iter().enumerate().map(|(k, r)| (st, r, (k > 0).then_some(" "))));
            }
            return;
        }
        // model prose: markdown, one source line at a time so ``` fences keep
        // their state and code never gets reflowed. A table is the exception —
        // its columns cannot be measured one line at a time, so it is taken as
        // a block and the cursor jumps past it.
        let src: Vec<&str> = l.split('\n').collect();
        let mut fence = false;
        let mut i = 0;
        while i < src.len() {
            let line = src[i];
            if line.trim_start().starts_with("```") {
                fence = !fence;
                i += 1;
                continue; // the fence itself is not drawn; the body is coloured
            }
            if fence {
                all.push((b'm', format!("  {}{RESET}", highlight(&fit(line, 2, inner_w + 2))), None));
                i += 1;
                continue;
            }
            if let Some((rows, used)) = table_block(&src[i..]) {
                for row in render_table(&rows, inner_w) {
                    all.push((b'm', row, None));
                }
                i += used;
                continue;
            }
            let (cs, runs) = md_line(line);
            for (x, y, join) in wrap_ranges(&cs, inner_w) {
                all.push((b'm', md_row(&cs, &runs, x, y), join));
            }
            i += 1;
        }
    };
    for l in visible(&app.lines, app.expand, app.hide_thinking) {
        push_wrapped(&l, &mut all);
    }
    if app.hide_thinking && app.current.starts_with(THINK) {
        all.extend(thinking_preview(&app.current, inner_w).into_iter().map(|r| (b't', r, None)));
    } else if !app.current.is_empty() {
        push_wrapped(&app.current.clone(), &mut all);
    }
    state.max_scroll = all.len().saturating_sub(transcript_h);
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
    let sel = app.sel.map(|(a, b)| (a.min(b), a.max(b)));
    for (i, (st, row, _)) in all.iter().enumerate().take(end).skip(start) {
        let styled = styled(*st, row, inner_w, spin);
        let t = theme::tints();
        let styled = match st {
            b'x' => band(t.fail, &styled, w),
            b'a' => band(t.ask, &styled, w),
            _ => styled,
        };
        let row = match sel.filter(|(lo, hi)| (lo.0..=hi.0).contains(&i)) {
            Some((lo, hi)) => reverse(&styled, sel_cols(i, lo, hi)),
            None => styled,
        };
        frame.push(expand_tabs(&row).into_owned());
    }
    state.rows = all;
    state.view_h = transcript_h;
    state.inner_w = inner_w;

    frame.push(String::new()); // blank spacer

    frame.extend(pending);
    frame.extend(panel);

    // Two rows, not one: the hint rides ABOVE the input because it is what
    // changes mid-turn and belongs next to the caret, while session/model/branch
    // stays BELOW, pinned to the bottom edge where it can be read at a glance.
    let hint = hint_text(app, spin);
    // the live frame gets the accent; every other hint is plain dim text
    frame.push(match hint.as_str() {
        "" => String::new(),
        h if h.starts_with(spin) => {
            let t = theme::current();
            format!("{}{spin}{RESET}{} working…{RESET}", t.warn, t.dim)
        }
        h => format!("{}{}{RESET}", theme::current().dim, truncate_str(h, w)),
    });

    let input_len = input.len();
    frame.extend(input);

    let mode = crate::ai_core::mode();
    let footer = app::footer_right(
        &app.footer, mode, &app.session, &app.model, &app.branch,
        app.sess_tok, app.turn_ctx, crate::ai_core::context_limit(), app.cache_pct, w,
    );
    let t = theme::current();
    // the mode leads in its own colour: manual plain, plan info, auto warn,
    // devil fail and bold so it is impossible to miss
    let (name, _) = app::mode_label(mode);
    let tint = match mode {
        crate::ai_core::Mode::Manual => t.dim.to_string(),
        crate::ai_core::Mode::Plan => t.info.to_string(),
        crate::ai_core::Mode::Auto => t.warn.to_string(),
        crate::ai_core::Mode::Devil => format!("{}{BOLD}", t.fail),
    };
    let footer = match footer.strip_prefix(name) {
        Some(rest) => format!("{tint}{name}{RESET}{}{rest}", t.dim),
        None => footer,
    };
    frame.push(format!("{}{footer}{RESET}", t.dim));

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
    // the footer holds the last row, so the input block ends one row short of it;
    // the caret sits in the row holding the cursor, not the last row of a draft
    let (caret_line, caret_col) = caret_at(draft, caret);
    let input_top = h.saturating_sub(1 + input_len);
    let input_y = (input_top + caret_line).min(h.saturating_sub(2)) as u16;
    // the columns drawn before the caret, not the chars before it: the row
    // scrolls, and a wide char takes two
    let line = draft.split('\n').nth(caret_line).unwrap_or("");
    let before = columns(&input_window(line, caret_col, w.saturating_sub(3)).0);
    let input_x = (2 + before).min(w.saturating_sub(1)) as u16; // after "> "
    goto(&mut out, input_x, input_y);

    out.write_all(SYNC_END.as_bytes())?;
    out.flush()?;

    state.prev_lines = frame;
    state.prev_rows = h;
    Ok(())
}

/// The hint row above the input: what the turn is waiting on, or how to get out.
fn hint_text(app: &App, spin: char) -> String {
    if app.armed() {
        "press ctrl+c again to exit".to_string()
    } else if let Some(n) = app.notice() {
        format!("ℹ {n}")
    } else if app.ask.is_some() && app.pick.is_none() {
        // the turn is waiting on you, not the model; a picker shows its own keys
        "answer above · enter sends · esc cancels".to_string()
    } else if !app.done {
        format!("{spin} working…")
    } else if app.fresh || app.scroll_up > 0 {
        // onboarding, not status: shown on a fresh prompt, and again when the
        // transcript is scrolled off the bottom (you're looking for the way out)
        "ctrl+c twice to quit".to_string()
    } else {
        String::new()
    }
}

/// Messages typed while the agent works, one dim row each above the panel:
/// steers (delivered after the current tool batch) then follow-ups (each its
/// own task later). The last row says how to take them back.
fn pending_rows(steers: &[String], follow: &std::collections::VecDeque<String>, w: usize) -> Vec<String> {
    let rows = steers.iter().map(|t| ("steer", t)).chain(follow.iter().map(|t| ("next", t)));
    let mut out: Vec<String> = rows
        .map(|(kind, t)| {
            let first = t.lines().next().unwrap_or("");
            let more = if t.contains('\n') { " …" } else { "" };
            format!("  ↳ {kind}: {first}{more}")
        })
        .collect();
    if let Some(last) = out.last_mut() {
        last.push_str("  · alt+↑ edit");
    }
    out.into_iter().map(|r| format!("{}{}{RESET}", theme::current().dim, truncate_str(&r, w))).collect()
}

/// The row block between transcript and input: the open session picker, or
/// the slash-command menu, or nothing. Coloured whole-line — cyan for the
/// selection, dim for the rest — like the tool rows.
fn panel_rows(app: &App, w: usize) -> Vec<String> {
    let sel_row = |s: &str, sel: bool| {
        let s = truncate_str(s, w);
        let t = theme::current();
        if sel { format!("{}{s}{RESET}", t.sel) } else { format!("{}{s}{RESET}", t.dim) }
    };

    if let Some(p) = &app.pick {
        let hint = match p.kind {
            app::PickKind::Session => "↑/↓ select · enter resume · type to filter · esc cancel",
            app::PickKind::Model => "↑/↓ select · enter switch · type to filter · esc cancel",
            app::PickKind::Tree => "↑/↓ select · enter branch here · type to filter · esc cancel",
            app::PickKind::Settings => "↑/↓ select · enter toggle · esc close",
            app::PickKind::Mcp => "↑/↓ select · enter connect/disconnect · esc close",
            app::PickKind::Theme => "↑/↓ select · enter apply · type to filter · esc close",
            app::PickKind::Ask => "↑/↓ select · enter confirm · 1-9 answer outright · esc no",
        };
        let vis = p.visible();
        // a question is not a list: its rows are the answers, so a count reads
        // as noise and the numbers belong on the rows instead
        let count = match p.kind {
            app::PickKind::Ask => String::new(),
            _ if p.filter.is_empty() => format!("({})", p.rows.len()),
            _ => format!("({}/{})", vis.len(), p.rows.len()),
        };
        let mut out = vec![sel_row(format!("  {} {count}", p.title).trim_end(), false)];
        if vis.is_empty() {
            out.push(sel_row("  nothing matches", false));
        }
        for (i, (label, _)) in vis.iter().enumerate().skip(p.top).take(app::PICK_ROWS) {
            let arrow = if i == p.idx { "▸ " } else { "  " };
            let label = match p.kind {
                app::PickKind::Ask => format!("{}. {label}", i + 1),
                _ => label.clone(),
            };
            out.push(sel_row(&format!("{arrow}{label}"), i == p.idx));
        }
        if vis.len() > app::PICK_ROWS {
            out.push(sel_row(&format!("  {hint}  · {}/{}", p.idx + 1, vis.len()), false));
        } else {
            out.push(sel_row(&format!("  {hint}"), false));
        }
        return out;
    }

    let menu = &app.menu;
    if menu.is_empty() {
        // the @ picker waits on its index; say so rather than look broken
        if app.index.is_some() && app::at_token(&app.input, app.cursor).is_some() {
            return vec![sel_row("  indexing files…", false)];
        }
        return Vec::new();
    }
    let pad = menu.iter().map(|it| match it {
        app::Item::Cmd(c) => c.name.chars().count(),
        _ => 0,
    }).max().unwrap_or(0);
    // a path row is its name, then the whole path dim beside it
    let name_pad = menu.iter().map(|it| match it {
        app::Item::Path { path, dir, .. } => path.rsplit('/').next().unwrap_or(path).chars().count() + *dir as usize,
        _ => 0,
    }).max().unwrap_or(0);
    let mut out: Vec<String> = menu
        .iter()
        .enumerate()
        .skip(app.menu_top)
        .take(MENU_ROWS)
        .map(|(i, it)| {
            let arrow = if i == app.menu_idx { "▸ " } else { "  " };
            let row = match it {
                app::Item::Cmd(c) => {
                    // where it comes from leads the text, so a narrow terminal cuts the description instead
                    let tag = if c.tag.is_empty() { String::new() } else { format!("[{}] ", c.tag) };
                    format!("{arrow}{:pad$}  {tag}{}{}", c.name, c.desc, if c.soon { "  · soon" } else { "" })
                }
                app::Item::Path { path, dir, .. } => {
                    let name = format!("{}{}", path.rsplit('/').next().unwrap_or(path), if *dir { "/" } else { "" });
                    format!("{arrow}{name:name_pad$}  {path}")
                }
            };
            sel_row(&row, i == app.menu_idx)
        })
        .collect();
    if menu.len() > MENU_ROWS {
        let keys = match menu[0] {
            app::Item::Cmd(_) => "tab completes · enter runs",
            app::Item::Path { .. } => "tab/enter inserts · esc closes",
        };
        out.push(sel_row(&format!("  ↑/↓ {}/{}  · {keys}", app.menu_idx + 1, menu.len()), false));
    }
    out
}

/// Write a line and clear to end of line (wipes stale trailing chars).
fn draw_line(out: &mut impl Write, line: &str) -> io::Result<()> {
    write!(out, "{line}")?;
    let _ = execute!(out, Clear(ClearType::UntilNewLine));
    Ok(())
}

/// The longest start of s that fits in max columns, its tabs expanded.
pub fn truncate_str(s: &str, max: usize) -> String {
    expand_tabs(&fit(s, 0, max)).into_owned()
}

/// The longest start of s, drawn from column col, that ends by column max.
/// Tabs are kept, measured to their stops; escapes take no columns.
fn fit(s: &str, mut col: usize, max: usize) -> String {
    let mut esc = false;
    s.chars()
        .take_while(|&c| {
            if c == '\x1b' {
                esc = true;
            } else if esc {
                esc = !c.is_ascii_alphabetic();
            } else {
                col += step(col, c);
            }
            col <= max
        })
        .collect()
}

/// One transcript row, colored by kind: status symbol colored, text plain.
/// User lines ("N› text") are flush-left with a dim number + cyan caret;
/// everything else gets the 2-space indent.
/// Reasoning: italic light grey, the whole block. 256-colour 249 (#b2b2b2) rather
/// than ESC[2m — Windows Terminal renders dim as barely-darker, which is what made
/// reasoning and answer text look identical.
/// Tool output text. 256-colour 245 (#8a8a8a) for the same reason THINK avoids
/// ESC[2m — and a shade under THINK so reasoning still reads as the brighter of
/// the two greys, with the italic carrying the rest of the difference.
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
    } else if l.starts_with("  · - ") {
        b'-' // a removed line in an edit's hunk
    } else if l.starts_with("  · + ") {
        b'+' // an added one
    } else if l.starts_with("  · ") {
        b's' // turn stats
    } else if l.starts_with("  ┆ ") {
        b'n' // the model's narration between tool calls
    } else if l.starts_with("  ✗ ") {
        b'x' // a failed tool: the one tool row that gets a background
    } else if l.starts_with("  ? ") {
        b'a' // a tool waiting on the user's permission
    } else if is_user_line(l) {
        b'u' // split from b'k': the query stays bright, tool rows do not
    } else if MARKERS.iter().any(|p| l.starts_with(p)) {
        b'k'
    } else {
        b'm'
    }
}

fn colorize_row(style: u8, s: &str, spin: char) -> String {
    let t = theme::current();
    let (think, dim) = (t.think, t.dim);
    match style {
        // the │ is an internal sentinel for line_style, never drawn: italic grey
        // carries the block on its own, the way the reference terminals do it
        b't' => return format!("  {think}{}{RESET}", s.strip_prefix("  │ ").unwrap_or(s)),
        // narration: dim italic under a thin rail, so the answer is the only
        // prose at full brightness. The sentinel is as wide as what replaces it
        b'n' => return format!("  {dim}│{RESET} {think}{}{RESET}", s.strip_prefix("  ┆ ").unwrap_or(s)),
        // a failed tool sits on the fail tint (band() in draw); its text stays
        // readable rather than dim, since it is the row that needs reading, and
        // takes the tint's own colour: the terminal's default may not suit it
        b'x' => {
            let x = theme::tints().fail_text;
            return match s.strip_prefix("  ✗ ") {
                Some(rest) => format!("{f}▎{RESET} {f}✗{RESET} {x}{rest}{RESET}", f = t.fail),
                None => format!("  {x}{s}{RESET}"),
            };
        }
        b'a' => {
            let x = theme::tints().ask_text;
            return match s.strip_prefix("  ? ") {
                Some(rest) => format!("{w}▎{RESET} {BOLD}{x}?{RESET} {x}{rest}{RESET}", w = t.warn),
                None => format!("  {x}{s}{RESET}"),
            };
        }
        b's' => return format!("{dim}{s}{RESET}"),
        // the sign is what the eye should catch, so it keeps full colour while
        // the code itself stays dim — a hunk is context, not the answer
        b'-' | b'+' => {
            let colour = if style == b'-' { t.del } else { t.add };
            // "  · - " is the marker; everything after it is the line's own code
            let cut = s.char_indices().nth(6).map(|(i, _)| i).unwrap_or(s.len());
            let (sign, rest) = s.split_at(cut);
            return format!("{colour}{sign}{RESET}{dim}{rest}{RESET}");
        }
        b'u' => return user_row(t, s),
        _ => {}
    }
    // b'k': tool rows. Coloured glyph, dim text, so a wall of tool output
    // recedes behind the prose instead of competing with it.
    if let Some(rest) = s.strip_prefix("  ⠋ ") {
        // the one row that is still happening: turning spinner + bold text, so
        // "running" and "finished a while ago" cannot be confused at a glance.
        // ToolEnd rewrites this row to ✓/✗, which lands in the dim branches
        // below — that swap is what un-bolds it.
        // The ▎ edge takes the indent's first column, so rows stay aligned:
        // a thin mark of state instead of a filled row, which a long run of
        // successful tools would turn into a wall of colour.
        format!("{w}▎{RESET} {w}{spin}{RESET} {BOLD}{rest}{RESET}", w = t.warn)
    } else if let Some(rest) = s.strip_prefix("  ✓ ") {
        format!("{o}▎{RESET} {o}✓{RESET} {dim}{rest}{RESET}", o = t.ok)
    } else if let Some(rest) = s.strip_prefix("  ⚠ ") {
        format!("  {}⚠{RESET} {dim}{rest}{RESET}", t.warn)
    } else if let Some(rest) = s.strip_prefix("  ℹ ") {
        format!("  {}ℹ{RESET} {dim}{rest}{RESET}", t.info)
    } else {
        format!("  {dim}{s}{RESET}") // wrapped continuation of a tool row
    }
}

/// The user's own query: cyan "N›" marker so it's easy to find when scrolling
/// back, bold text so it stands apart from the model's answer. Wrapped rows
/// carry no marker and must stay bold too — that's the whole reason this isn't
/// b'k'. No background: a fill is only readable if it suits the terminal's own
/// colours, which rusti cannot see, and a pale one under light text was not.
fn user_row(t: &theme::Theme, s: &str) -> String {
    let i = s.bytes().take_while(u8::is_ascii_digit).count();
    if i > 0 && s[i..].starts_with("› ") {
        format!("{}{}›{RESET}{BOLD}{}{RESET}", t.user, &s[..i], &s[i + '›'.len_utf8()..])
    } else {
        format!("{BOLD}  {s}{RESET}")
    }
}

/// Fill a styled row with a background across the terminal. Every RESET inside
/// would drop the background mid-row, so the tint is re-applied after each one.
/// One column short of the edge: writing the last column makes some terminals
/// wrap onto the next row.
fn band(bg: &str, row: &str, w: usize) -> String {
    let mut vis = 0;
    let mut esc = false;
    for c in row.chars() {
        match c {
            '\x1b' => esc = true,
            c if esc => esc = !c.is_ascii_alphabetic(),
            c => vis += step(vis, c),
        }
    }
    let body = row.replace(RESET, &format!("{RESET}{bg}"));
    format!("{bg}{body}{}{RESET}", " ".repeat(w.saturating_sub(1).saturating_sub(vis)))
}

// ── markdown for model prose ────────────────────────────────────────────────
// Deliberately small: headings, **bold**, `code`, - bullets, ``` fences. No
// parser crate. Markers become style runs over the *visible* text, so wrapping
// measures real columns instead of counting asterisks it is about to delete.

const BOLD: &str = "\x1b[1m";
fn code() -> &'static str { theme::current().code }     // inline `code`

// ── tables ──────────────────────────────────────────────────────────────────
// A pipe table only means anything as a block: the columns are as wide as the
// widest cell in them, which cannot be known one line at a time. Raw `| … |`
// rows in the transcript were the whole complaint.

/// The rows of a table starting at `src[0]` and **how many source lines they
/// came from**, or None if this is not one. A table is a pipe row, then a
/// `|---|` separator, then pipe rows: the separator is what tells a table from
/// a line of prose that happens to contain a pipe.
///
/// The count is returned rather than recomputed by the caller because it is not
/// `rows.len()`: the separator is consumed but is not a row. Deriving it at the
/// call site left the last row to be drawn twice — once in the table, once as
/// raw pipes underneath it.
fn table_block<'a>(src: &[&'a str]) -> Option<(Vec<Vec<&'a str>>, usize)> {
    let is_row = |s: &str| s.trim_start().starts_with('|');
    let is_rule = |s: &str| {
        let t = s.trim();
        t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) && t.contains('-')
    };
    if !is_row(src.first()?) || !is_rule(src.get(1)?) {
        return None;
    }
    let cells = |s: &'a str| -> Vec<&'a str> {
        let t = s.trim().trim_start_matches('|').trim_end_matches('|');
        t.split('|').map(str::trim).collect()
    };
    let mut rows = vec![cells(src[0])];
    for line in src.iter().skip(2).take_while(|s| is_row(s)) {
        rows.push(cells(line));
    }
    let used = rows.len() + 1; // + the separator
    Some((rows, used))
}

/// Header in the accent, a rule under it, then the body. Columns are as wide as
/// their widest cell; if the total will not fit, the widest column gives back
/// first, because that is the one with slack. Cells truncate rather than wrap —
/// a wrapped cell destroys the alignment that makes a table worth drawing.
fn render_table(rows: &[Vec<&str>], w: usize) -> Vec<String> {
    let t = theme::current();
    // Cells are markdown too: `code` and **bold** have to lose their markers
    // before anything is measured, or a column is padded for characters that
    // are never drawn — and the backticks show up in the table.
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| md_line(c).0.iter().collect::<String>()).collect())
        .collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return Vec::new();
    }
    let mut width: Vec<usize> = (0..cols)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| columns(s)).max().unwrap_or(0))
        .collect();
    // gap of 1 between columns, 2 for the transcript indent
    let budget = w.saturating_sub(2);
    while width.iter().sum::<usize>() + cols.saturating_sub(1) > budget {
        let widest = width.iter().enumerate().max_by_key(|(_, n)| **n).map(|(i, _)| i).unwrap_or(0);
        if width[widest] <= 3 {
            break; // nothing left to give; the last column will be cut instead
        }
        width[widest] -= 1;
    }
    let lay = |cells: &[String]| {
        let mut s = String::from("  ");
        for (c, wid) in width.iter().enumerate() {
            let cell = cells.get(c).map(String::as_str).unwrap_or("");
            let cell = truncate_str(cell, *wid);
            s.push_str(&cell);
            let pad = wid.saturating_sub(columns(&cell)) + usize::from(c + 1 < cols);
            s.extend(std::iter::repeat(' ').take(pad));
        }
        s.trim_end().to_string()
    };
    let mut out = vec![format!("{}{BOLD}{}{RESET}", t.head(), lay(&rows[0]))];
    let rule: Vec<String> = width.iter().map(|n| "─".repeat(*n)).collect();
    out.push(format!("{}  {}{RESET}", t.rule(), rule.join(" ")));
    for row in rows.iter().skip(1) {
        out.push(lay(row));
    }
    out
}

// ── syntax highlighting ─────────────────────────────────────────────────────
// One tokeniser for every language, because the alternative is a grammar per
// language and this only has to beat "the whole block is one colour". It marks
// what is unambiguous across C-likes, JSON, TOML and shell: quoted strings, a
// string used as a key, numbers, the three word-literals, and punctuation.

/// Colour one line of a fenced block. Anything not recognised keeps the block's
/// own colour, so an unknown language degrades to what rusti did before.
fn highlight(line: &str) -> String {
    let t = theme::current();
    let cs: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '"' || c == '\'' {
            let mut j = i + 1;
            while j < cs.len() && cs[j] != c {
                j += if cs[j] == '\\' { 2 } else { 1 }; // an escaped quote does not close it
            }
            let end = (j + 1).min(cs.len());
            let text: String = cs[i..end].iter().collect();
            // a string followed by ':' names something — that is a key
            let key = cs[end..].iter().find(|c| !c.is_whitespace()) == Some(&':');
            out.push_str(&format!("{}{text}{RESET}", if key { t.key() } else { t.string() }));
            i = end;
        } else if c.is_ascii_digit() && !cs[..i].last().is_some_and(|p| p.is_alphanumeric() || *p == '_') {
            let j = cs[i..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == '.').count() + i;
            out.push_str(&format!("{}{}{RESET}", t.num(), cs[i..j].iter().collect::<String>()));
            i = j;
        } else if c.is_alphabetic() || c == '_' {
            let j = cs[i..].iter().take_while(|c| c.is_alphanumeric() || **c == '_').count() + i;
            let word: String = cs[i..j].iter().collect();
            match word.as_str() {
                "true" | "false" | "null" | "None" | "nil" => {
                    out.push_str(&format!("{}{word}{RESET}", t.boolean()))
                }
                _ => out.push_str(&format!("{}{word}{RESET}", t.fenced)),
            }
            i = j;
        } else if "{}[]()<>,:;=".contains(c) {
            out.push_str(&format!("{}{c}{RESET}", t.punct()));
            i += 1;
        } else {
            out.push_str(&format!("{}{c}{RESET}", t.fenced));
            i += 1;
        }
    }
    out
}

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
                runs.push((s, out.len(), code()));
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

/// Greedy word wrap over chars, measured in columns, returning each row's range and its join: what
/// the break before it dropped (a space, or nothing for a hard break inside a
/// word), None on the first row. Ranges (not strings) keep the style runs
/// addressable after wrapping.
fn wrap_ranges(cs: &[char], max: usize) -> Vec<(usize, usize, Option<&'static str>)> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut join = None;
    while start < cs.len() {
        // the most chars that fit in max columns, and at least one; md_row
        // draws them from column 2, where the tab stops are counted
        let mut hard = start;
        let mut used = 0;
        while hard < cs.len() && used + step(used + 2, cs[hard]) <= max {
            used += step(used + 2, cs[hard]);
            hard += 1;
        }
        let hard = hard.max(start + 1);
        if hard == cs.len() {
            rows.push((start, cs.len(), join));
            break;
        }
        let brk = (start..hard).rev().find(|&j| cs[j] == ' ').unwrap_or(start);
        let end = if brk == start { hard } else { brk };
        rows.push((start, end, join));
        join = Some(if cs[end] == ' ' { " " } else { "" });
        start = end;
        while start < cs.len() && cs[start] == ' ' {
            start += 1;
        }
    }
    if rows.is_empty() {
        rows.push((0, 0, None)); // a blank source line is still a blank row
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
        let prompt = format!("{}> {RESET}", theme::current().user);
        let prefix: &str = if i == 0 { &prompt } else { "  " };
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

/// The part of a line that fits max columns around the caret, as (before,
/// after) it: the caret sits about 3/4 across, unless the end of the line
/// leaves room to show more of what comes before it.
fn input_window(s: &str, c: usize, max: usize) -> (String, String) {
    if max == 0 { return (String::new(), String::new()); }
    let chars: Vec<char> = s.chars().collect();
    let c = c.min(chars.len());
    if columns(s) <= max {
        return (chars[..c].iter().collect(), chars[c..].iter().collect());
    }
    let (mut start, mut used) = (c, 0);
    while start > 0 && used + width(chars[start - 1]) <= max * 3 / 4 {
        start -= 1;
        used += width(chars[start]);
    }
    let tail: usize = chars[c..].iter().map(|&ch| width(ch)).sum();
    while start > 0 && used + tail + width(chars[start - 1]) <= max {
        start -= 1;
        used += width(chars[start]);
    }
    let mut end = c;
    while end < chars.len() && used + width(chars[end]) <= max {
        used += width(chars[end]);
        end += 1;
    }
    let mut pre: String = chars[start..c].iter().collect();
    if start > 0 && !pre.is_empty() {
        pre = format!("…{}", pre.chars().skip(1).collect::<String>());
    }
    (pre, chars[c..end].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every theme's user row keeps the terminal's own background and draws
    /// nothing faint, so the text is never light-on-pale.
    #[test]
    fn user_row_has_no_background_in_any_theme() {
        for t in theme::THEMES {
            for row in ["3› what is this", "wrapped tail of it"] {
                let out = user_row(t, row);
                assert!(bare(&out).contains(row.trim_start_matches("3› ")), "{}: text lost", t.name);
                for sgr in out.split('\x1b').skip(1) {
                    let params = sgr.strip_prefix('[').and_then(|r| r.split_once('m')).map_or("", |(p, _)| p);
                    let codes: Vec<&str> = params.split(';').collect();
                    let mut k = 0;
                    while k < codes.len() {
                        match codes[k] {
                            // extended fg: skip its colour arguments
                            "38" => k += if codes.get(k + 1) == Some(&"5") { 3 } else { 5 },
                            c => {
                                let n: u32 = c.parse().unwrap_or(0);
                                assert!(!matches!(n, 2 | 7 | 40..=49 | 100..=107),
                                    "{}: user row sets {n} (faint, reverse or background) in {out:?}", t.name);
                                k += 1;
                            }
                        }
                    }
                }
            }
        }
    }


    /// Strip SGR sequences: these tests care about layout and which colour a
    /// token got, not about where the escapes land.
    #[cfg(test)]
    fn bare(s: &str) -> String {
        let mut out = String::new();
        let mut cs = s.chars();
        while let Some(c) = cs.next() {
            if c == '\x1b' {
                for c in cs.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Queued messages show above the input in queue order, a multi-line one
    /// by its first line, and the last row carries the way to edit them.
    #[test]
    fn pending_rows_list_steers_then_follow_ups() {
        let follow: std::collections::VecDeque<String> = ["then commit".to_string()].into();
        let rows: Vec<String> = pending_rows(&["use tabs\nand spaces".into()], &follow, 80).iter().map(|r| bare(r)).collect();
        assert_eq!(rows, ["  ↳ steer: use tabs …", "  ↳ next: then commit  · alt+↑ edit"]);
        assert!(pending_rows(&[], &Default::default(), 80).is_empty(), "nothing queued, no rows");
    }

    /// The separator row is what makes it a table: without it, a line of prose
    /// with a pipe in it would be swallowed as one. The columns then have to
    /// line up, which is the entire reason for rendering instead of printing.
    #[test]
    fn a_pipe_table_becomes_aligned_columns() {
        let src = vec![
            "| Tool | Description |",
            "|------|-------------|",
            "| `index_repository` | Index a codebase |",
            "| `search_graph` | Search the graph |",
            "not part of the table",
        ];
        let (rows, used) = table_block(&src).expect("this is a table");
        assert_eq!(rows.len(), 3, "the prose line after it is not a row: {rows:?}");
        assert_eq!(rows[0], vec!["Tool", "Description"]);
        // the separator is consumed but is not a row: deriving this from
        // rows.len() drew the last row twice, once as raw pipes
        assert_eq!(used, 4, "header + separator + two body rows");

        let out: Vec<String> = render_table(&rows, 60).iter().map(|r| bare(r)).collect();
        assert_eq!(out.len(), 4, "header, rule, two body rows");
        // every cell of column 2 starts at the same column
        let at = |r: &str, needle: &str| r.find(needle).unwrap();
        assert_eq!(at(&out[0], "Description"), at(&out[2], "Index a codebase"));
        assert_eq!(at(&out[2], "Index a codebase"), at(&out[3], "Search the graph"));
        assert!(out[1].contains("─"), "the header gets a rule: {:?}", out[1]);
        assert!(render_table(&rows, 60)[0].contains(theme::current().head()), "header takes the accent");
        // a cell is markdown too: the backticks must not reach the screen, and
        // the column must not be padded for characters that are never drawn
        assert!(out[2].contains("index_repository"), "{:?}", out[2]);
        assert!(!out[2].contains('`'), "backticks must be stripped: {:?}", out[2]);
        assert_eq!(at(&out[0], "Description"), at(&out[3], "Search the graph"));

        // a line with a pipe but no rule under it is prose, not a table
        assert!(table_block(&["a | b", "still prose"]).is_none());
        // narrow: columns give back width, rows do not multiply
        let narrow: Vec<String> = render_table(&rows, 26).iter().map(|r| bare(r)).collect();
        assert_eq!(narrow.len(), 4, "a cramped table truncates, it does not wrap");
        assert!(narrow.iter().all(|r| r.chars().count() <= 26), "{narrow:?}");
    }

    /// The key/string split is the subtle half: both are quoted, and what tells
    /// them apart is the colon that follows one of them.
    #[test]
    fn a_fenced_line_is_coloured_by_token() {
        let t = theme::current();
        let line = highlight(r#"  "command": "rusti.exe", "timeout": 30, "enabled": true"#);
        let colour_of = |needle: &str| {
            let i = line.find(needle).expect(needle);
            let start = line[..i].rfind('\x1b').expect("a colour before it");
            line[start..i].to_string()
        };
        assert_eq!(colour_of(r#""command""#), t.key(), "a quoted name before a colon is a key");
        assert_eq!(colour_of(r#""rusti.exe""#), t.string(), "the value is a string");
        assert_eq!(colour_of("30"), t.num());
        assert_eq!(colour_of("true"), t.boolean());
        assert_eq!(bare(&line), r#"  "command": "rusti.exe", "timeout": 30, "enabled": true"#,
            "highlighting must not change one visible character");

        // an escaped quote does not end the string, or the rest of the line
        // would be coloured as if it were outside it
        let esc = highlight(r#""a\"b" 12"#);
        assert_eq!(bare(&esc), r#""a\"b" 12"#);
        assert_eq!(esc.matches(t.string()).count(), 1, "one string, not two: {esc:?}");
    }

    /// The bug this pins: word_wrap used to drop the leading indent, so every
    /// marker row missed its colorize_row branch and rendered plain. Both halves
    /// must agree, so run a row through the real pipeline, not colorize_row alone.
    #[test]
    fn status_markers_keep_their_colour_through_word_wrap() {
        // every row of a logical line, the way draw() feeds them
        let rows = |line: &str| -> Vec<String> {
            let st = line_style(line);
            wrap(st, line, 80).iter().map(|r| colorize_row(st, &truncate_str(r, 80), '⠋')).collect()
        };
        let first = |line: &str| rows(line).remove(0);
        // a running tool must be impossible to mistake for a finished one:
        // the glyph turns with the frame, and the text is bold, not dim
        let run = colorize_row(line_style("  ⠋ pwd"), "  ⠋ pwd", '⠹');
        assert!(run.contains('⠹'), "spinner must show the live frame, not the stored ⠋: {run:?}");
        assert!(run.contains(BOLD) && !run.contains(theme::current().dim), "running tool must be bold: {run:?}");
        assert!(first("  ✓ Cargo.toml  0ms").contains(theme::current().dim), "a finished tool un-bolds to dim");
        assert!(!first("  ✓ Cargo.toml  0ms").contains(BOLD), "a finished tool is not bold");
        assert!(first("  ✓ Cargo.toml  0ms").contains("\x1b[32m✓"), "tool ok must be green");
        assert!(first("  ✗ edit failed").contains("\x1b[38;5;203m✗"), "tool fail must be red");
        // tinted rows pin their text colour on every wrapped row, never the
        // terminal's default foreground, which may not read on the tint
        let long = format!("  ✗ run {}", "word ".repeat(30));
        let fail = rows(&long);
        assert!(fail.len() > 1 && fail.iter().all(|r| r.contains(theme::tints().fail_text)), "{fail:?}");
        assert!(rows(&format!("  ? run {}  allow?", "word ".repeat(30))).iter().all(|r| r.contains(theme::tints().ask_text)));
        assert!(first("  ⠋ cargo build").contains("\x1b[33m⠋"), "running must be yellow");
        assert!(first("  ⚠ interrupted").contains("\x1b[33m⚠"), "warn must be yellow");
        assert!(first("  ℹ renamed").contains("\x1b[94mℹ"), "info must be bright blue: 34 is 2.4:1 on black");
        assert!(first("  · 32 tok").starts_with(theme::current().dim), "stats must be dim");
        // a hunk row is a stats row until the sign is read, so the order of the
        // line_style arms is the whole feature: "  · - x" must not land on b's'
        assert!(first("  · -    2  let x = 1;").starts_with("\x1b[38;5;203m"), "a removed line must be red");
        assert!(first("  · +    2  let x = 2;").starts_with("\x1b[32m"), "an added line must be green");
        assert!(first("  · +    2  let x = 2;").contains(theme::current().dim), "the code itself stays dim");
        assert!(first("  · … 3 more changed lines").starts_with(theme::current().dim), "the overflow row is not a hunk row");
        // tool text is dim, so a wall of output recedes behind the prose
        assert!(first("  ✓ Cargo.toml  0ms").contains(theme::current().dim), "tool text must be dim");
        // the user's query keeps a cyan marker and BRIGHT text, on every row —
        // wrapped rows carry no marker, so they must not fall into the dim branch
        let q = rows("9› hello there, this is a long query that will certainly wrap past eighty columns");
        assert!(q[0].starts_with("\x1b[36m9›\x1b[0m"), "query marker must be cyan: {:?}", q[0]);
        assert!(q.len() > 1 && q.iter().all(|r| !r.contains(theme::current().dim)), "query must stay bright: {q:?}");
        assert_eq!(first("12› hi there"), "\x1b[36m12›\x1b[0m\x1b[1m hi there\x1b[0m");
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
        assert_eq!(at(s.find("grep").unwrap()), Some(code()));
        assert_eq!(at(s.find("fast").unwrap()), Some(BOLD));
        assert_eq!(at(s.find("Search").unwrap()), None);

        // a heading keeps bold across an inline code span nested inside it
        let (cs, runs) = md_line("## use `grep` now");
        let row = md_row(&cs, &runs, 0, cs.len());
        assert!(row.contains(code()) && row.matches(BOLD).count() >= 2, "{row:?}");
    }

    #[test]
    fn markdown_wrapping_measures_visible_text_not_markers() {
        // width is charged against the text the user sees, not the markers
        let (cs, _) = md_line("**aaaa** bbbb cccc dddd");
        let rows = wrap_ranges(&cs, 10);
        assert!(rows.iter().all(|(x, y, _)| y - x <= 10), "{rows:?}");
        assert_eq!(rows.iter().map(|(x, y, _)| cs[*x..*y].iter().collect::<String>()).collect::<Vec<_>>(),
                   vec!["aaaa bbbb", "cccc dddd"]);
        // a word longer than the width is hard-cut rather than dropped
        let long: Vec<char> = "supercalifragilistic".chars().collect();
        assert_eq!(wrap_ranges(&long, 5).len(), 4);
        // a copy rejoins a space break with its space, a hard cut with nothing
        let joins = |cs: &[char], w| wrap_ranges(cs, w).into_iter().map(|r| r.2).collect::<Vec<_>>();
        assert_eq!(joins(&cs, 10), vec![None, Some(" ")]);
        assert_eq!(joins(&long, 5), vec![None, Some(""), Some(""), Some("")]);
        let exact: Vec<char> = "aaaa bbbb".chars().collect();
        assert_eq!(joins(&exact, 4), vec![None, Some(" ")], "a cut that lands on a space dropped it");
        // blank source line still produces one row
        assert_eq!(wrap_ranges(&[], 10), vec![(0, 0, None)]);
    }

    /// The regression behind "I can't see any difference": reasoning wraps over
    /// many rows and only the first carried the marker, so the rest rendered as
    /// plain text — indistinguishable from the answer that follows.
    #[test]
    fn every_row_of_a_wrapped_reasoning_block_is_styled() {
        let long = "  │ ".to_string() + &"thinking ".repeat(60);
        let st = line_style(&long);
        let rows: Vec<String> =
            wrap(st, &long, 40).iter().map(|r| colorize_row(st, &truncate_str(r, 40), '⠋')).collect();
        assert!(rows.len() > 3, "expected a wrapped block, got {}", rows.len());
        assert!(rows.iter().all(|r| r.contains(theme::current().think)), "every row must be italic grey: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains('│')), "the sentinel bar is never drawn");
        // the answer is the plain one, so the two can never be confused
        assert!(!colorize_row(line_style("an answer"), "an answer", '⠋').contains(theme::current().think));
        // paragraph breaks inside a block survive and stay styled
        let two = "  │ first thought\n\nsecond thought";
        let st = line_style(two);
        assert!(wrap(st, two, 40).iter().all(|r| colorize_row(st, r, '⠋').contains(theme::current().think)));
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

        let folded: Vec<Cow<str>> = visible(&lines, false, false).collect();
        assert_eq!(folded, vec!["  ✓ cargo test", "  ✗ cargo build", "  · error: no main"]);

        let open: Vec<Cow<str>> = visible(&lines, true, false).collect();
        assert_eq!(open.len(), 4);
        assert_eq!(open[1], "  · 27 passed", "the marker must be stripped, not drawn");
        assert!(!open.iter().any(|r| r.contains(app::HIDDEN)));
    }

    /// Ctrl+T folds every finished reasoning block to one row that still
    /// reads as reasoning; the live block keeps the fold plus its last two
    /// rows, so a hidden thought still shows the model working.
    #[test]
    fn hidden_thinking_folds_but_the_live_block_previews_two_rows() {
        let lines = vec![
            "1› hi".to_string(),
            "  │ first thought

second thought".to_string(),
            "the answer".to_string(),
        ];
        let shown: Vec<Cow<str>> = visible(&lines, false, false).collect();
        assert_eq!(shown[1], lines[1], "visible by default: the block is untouched");
        let folded: Vec<Cow<str>> = visible(&lines, false, true).collect();
        assert_eq!(folded, vec!["1› hi", "  │ thinking… (2 lines · ctrl+t)", "the answer"]);
        assert_eq!(line_style(&folded[1]), b't', "the fold is styled as reasoning");

        let live = "  │ ".to_string() + &"word ".repeat(40) + "

last bit";
        let rows = thinking_preview(&live, 40);
        assert_eq!(rows.len(), 3, "fold + two rows, however long the thought: {rows:?}");
        assert_eq!(rows[0], "  │ thinking… (2 lines · ctrl+t)");
        assert_eq!(rows[2], "last bit", "the preview is the newest text");
        assert!(rows[1].starts_with("word"), "a paragraph break is skipped, not shown blank");
        // a block that has only just started previews what little it has
        assert_eq!(thinking_preview("  │ ", 40), vec!["  │ thinking… (0 lines · ctrl+t)"]);
        assert_eq!(thinking_preview("  │ hm", 40).len(), 2);
    }

    /// The three rows that get a background keep it across the whole row, even
    /// past the RESETs their own colouring emits; every other row has none.
    #[test]
    fn user_failure_and_permission_rows_are_banded() {
        let bg = "\x1b[48;2;1;2;3m";
        let row = colorize_row(line_style("  ✗ run pwd  not found"), "  ✗ run pwd  not found", '⠋');
        let b = band(bg, &row, 40);
        assert!(b.starts_with(bg) && b.ends_with(RESET));
        assert_eq!(b.matches(RESET).count(), row.matches(RESET).count() + 1);
        assert!(b.replace(RESET, "").matches(bg).count() >= row.matches(RESET).count(), "tint re-applied after each reset");
        let visible: String = {
            let mut out = String::new();
            let mut esc = false;
            for c in b.chars() {
                match c { '\x1b' => esc = true, c if esc => esc = !c.is_ascii_alphabetic(), c => out.push(c) }
            }
            out
        };
        assert_eq!(visible.chars().count(), 39, "padded to one short of the terminal edge");
        assert_eq!(line_style("1› hi"), b'u');
        assert_eq!(line_style("  ✗ run pwd"), b'x');
        assert_eq!(line_style("  ? run pwd  allow?"), b'a');
        assert_eq!(line_style("  ┆ narration"), b'n');
        assert_eq!(line_style("  ✓ read x"), b'k', "a success gets an edge, not a band");
        assert!(colorize_row(b'k', "  ✓ read x", '⠋').contains('▎'));
    }

    /// A free-text question waits on the user, not the model: while one is
    /// pending the hint says how to answer instead of "working…", which made an
    /// unanswered ask_user look like a hang. A permission picker shows its own keys.
    #[test]
    fn a_pending_question_asks_for_an_answer_not_working() {
        let mut app = app::App::new("m".into(), "s".into(), vec![], vec![], 0, Default::default());
        app.done = false;
        assert_eq!(hint_text(&app, '⠋'), "⠋ working…");
        app.ask = Some(("push where?".into(), tokio::sync::oneshot::channel().0));
        assert_eq!(hint_text(&app, '⠋'), "answer above · enter sends · esc cancels");

        // F15: a typed question closes a list left open, so the prompt is yours
        app.ask = None;
        app.pick = Some(app::Pick { kind: app::PickKind::Theme, title: "t".into(), rows: vec![], idx: 0, top: 0, filter: String::new() });
        app::ask(&mut app, "push where?".into(), vec![], tokio::sync::oneshot::channel().0);
        assert!(app.pick.is_none());
        assert_eq!(hint_text(&app, '⠋'), "answer above · enter sends · esc cancels");
    }

    /// A copy gives back the tabs the text had; only the drawn frame expands
    /// them. A Makefile pasted with spaces for tabs no longer builds.
    #[test]
    fn a_copy_keeps_tabs_the_frame_expands_them() {
        let mut state = RenderState::new();
        state.inner_w = 40;
        state.rows = vec![
            (b'm', format!("  {}{RESET}", highlight(&fit("\techo hi", 2, 42))), None),
            (b's', fit("  \tx", 0, 40), None),
        ];
        assert_eq!(state.selected(((0, 0), (1, usize::MAX))), "\techo hi\n\tx");
        // the tab at column 2 runs to 8, and the cell after it is the 'e'
        assert_eq!(char_at(&state.text(0), 8), 3);
        assert!(!expand_tabs(&styled(b's', &state.rows[1].1, 40, '⠋')).contains('\t'));
        assert_eq!(fit("\tab", 2, 9), "\ta", "a tab drawn from column 2 takes 6");

        // a row drawn after a gutter its raw text lacks has its tabs moved: it
        // wraps where it is drawn, so nothing is cut and the copy is whole
        let words = format!("abcdef\t{}", "yy ".repeat(40).trim_end());
        for (st, line, copy) in [
            (b't', format!("  │ {words}"), words.clone()),
            (b'n', format!("  ┆ {words}"), words.clone()),
            (b'u', format!("1› q\n{words}"), format!("1› q\n{words}")),
            // a CJK query has no spaces: it is cut where each row fills up,
            // and the copy rejoins the cuts with nothing
            (b'u', format!("1› {}", "修复登录错误的问题".repeat(10)), format!("1› {}", "修复登录错误的问题".repeat(10))),
        ] {
            assert_eq!(line_style(&line), st);
            let mut state = RenderState::new();
            state.inner_w = 78;
            state.rows.extend(wrap_joined(st, &line, 78).into_iter().map(|(r, join)| (st, r, join)));
            for (_, row, _) in &state.rows {
                assert!(columns(&plain(&expand_tabs(&styled(st, row, 78, '⠋')))) <= 78, "{}: {row:?}", st as char);
            }
            assert_eq!(state.selected(((0, 0), (state.rows.len() - 1, usize::MAX))), copy, "{}", st as char);
        }
    }

    /// F5: rows are measured in terminal columns. A CJK char takes two and a
    /// tab runs to the next stop of 8, so a row cut or wrapped by counting
    /// chars or bytes ran past the edge and the terminal wrapped it over the
    /// next row.
    #[test]
    fn rows_are_measured_in_columns_not_chars() {
        assert_eq!(columns("ab修复c"), 7);
        assert_eq!(expand_tabs("\tx\n12\ty"), format!("{}x\n12{}y", " ".repeat(8), " ".repeat(6)));
        assert_eq!(truncate_str("修复登录错误", 5), "修复", "a wide char that would straddle the edge goes");
        assert_eq!(truncate_str("a\tb", 9), format!("a{}b", " ".repeat(7)), "tabs are expanded, never written raw");
        assert_eq!(truncate_str("abc", 0), "");

        // model prose: a CJK reply wraps at the width, not at twice it
        let cjk: Vec<char> = "修复登录错误的问题".repeat(5).chars().collect();
        let rows = wrap_ranges(&cjk, 40);
        assert!(rows.iter().all(|(x, y, _)| columns(&cjk[*x..*y].iter().collect::<String>()) <= 40), "{rows:?}");
        assert_eq!(rows.iter().map(|(x, y, _)| y - x).sum::<usize>(), cjk.len(), "nothing dropped");
        assert_eq!(wrap_ranges(&['修'], 1), vec![(0, 1, None)], "a char wider than the row still moves on");

        // a failed row's band fills to one short of the edge, not past it
        let row = colorize_row(b'x', "  ✗ run git commit -m \"修复登录错误的问题\"  exit 1", '⠋');
        assert_eq!(columns(&plain(&band(theme::tints().fail, &row, 60))), 59);

        // table columns line up when a cell is wide
        let out: Vec<String> = render_table(&[vec!["名前", "x"], vec!["ab", "y"]], 40).iter().map(|r| bare(r)).collect();
        assert_eq!(columns(&out[0][..out[0].find('x').unwrap()]), columns(&out[2][..out[2].find('y').unwrap()]), "{out:?}");

        // the composer: the visible part fits and the caret column is real
        let draft = "修".repeat(40);
        for c in [0, 1, 20, 39, 40] {
            let (pre, suf) = input_window(&draft, c, 30);
            assert!(columns(&pre) + columns(&suf) <= 30, "c={c}: {pre:?} {suf:?}");
            assert!(!suf.is_empty() || c == 40, "c={c}: what follows the caret is shown");
        }
        let (pre, suf) = input_window("aé—ü".repeat(30).as_str(), 61, 20);
        assert!(pre.starts_with('…') && columns(&pre) + columns(&suf) <= 20, "a multi-byte start is cut by char");
        assert_eq!(input_window("ab修c", 3, 10), ("ab修".into(), "c".into()), "a short line is left whole");
    }
}
