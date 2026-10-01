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

use super::app::{self, App, CMDS, MENU_ROWS};
use super::theme;
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
    let mut rows = word_wrap(&thinking_fold(l), w);
    // paragraph breaks and the bare sentinel are blank rows, not a preview
    let body: Vec<String> =
        word_wrap(l, w).into_iter().filter(|r| !matches!(r.trim(), "" | "│")).collect();
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
}

impl RenderState {
    pub fn new() -> Self {
        Self { prev_lines: Vec::new(), prev_rows: 0, max_scroll: 0 }
    }
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
                all.push((b'm', format!("  {}{RESET}", highlight(&truncate_str(line, inner_w)))));
                i += 1;
                continue;
            }
            if let Some((rows, used)) = table_block(&src[i..]) {
                for row in render_table(&rows, inner_w) {
                    all.push((b'm', row));
                }
                i += used;
                continue;
            }
            let (cs, runs) = md_line(line);
            for (x, y) in wrap_ranges(&cs, inner_w) {
                all.push((b'm', md_row(&cs, &runs, x, y)));
            }
            i += 1;
        }
    };
    for l in visible(&app.lines, app.expand, app.hide_thinking) {
        push_wrapped(&l, &mut all);
    }
    if app.hide_thinking && app.current.starts_with(THINK) {
        all.extend(thinking_preview(&app.current, inner_w).into_iter().map(|r| (b't', r)));
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
    for (st, row) in &all[start..end] {
        // markdown rows are already wrapped and styled; truncating would cut
        // an escape sequence in half
        let styled = if *st == b'm' { row.clone() } else { colorize_row(*st, &truncate_str(row, inner_w), spin) };
        let t = theme::tints();
        frame.push(match st {
            b'u' => band(t.user, &styled, w),
            b'x' => band(t.fail, &styled, w),
            b'a' => band(t.ask, &styled, w),
            _ => styled,
        });
    }

    frame.push(String::new()); // blank spacer

    frame.extend(pending);
    frame.extend(panel);

    // Two rows, not one: the hint rides ABOVE the input because it is what
    // changes mid-turn and belongs next to the caret, while session/model/branch
    // stays BELOW, pinned to the bottom edge where it can be read at a glance.
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

    let footer = app::footer_right(
        &app.footer, crate::ai_core::plan_mode(), &app.session, &app.model, &app.branch,
        app.sess_tok, app.turn_ctx, crate::ai_core::context_limit(), app.cache_pct,
    );
    frame.push(format!("{}{}{RESET}", theme::current().dim, truncate_str(&footer, w)));

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
    let input_x = (2 + caret_col).min(w.saturating_sub(1)) as u16; // after "> "
    goto(&mut out, input_x, input_y);

    out.write_all(SYNC_END.as_bytes())?;
    out.flush()?;

    state.prev_lines = frame;
    state.prev_rows = h;
    Ok(())
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
        // readable rather than dim, since it is the row that needs reading
        b'x' => {
            return match s.strip_prefix("  ✗ ") {
                Some(rest) => format!("{f}▎{RESET} {f}✗{RESET} {rest}", f = t.fail),
                None => format!("  {s}"),
            };
        }
        b'a' => {
            return match s.strip_prefix("  ? ") {
                Some(rest) => format!("{w}▎{RESET} {BOLD}?{RESET} {rest}", w = t.warn),
                None => format!("  {s}"),
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
        // the user's own query: cyan "N›" marker so it's easy to find when
        // scrolling back, text at full brightness like the model's answer.
        // Wrapped rows carry no marker and must stay bright too — that's the
        // whole reason this isn't b'k'.
        b'u' => {
            let i = s.bytes().take_while(u8::is_ascii_digit).count();
            return if i > 0 && s[i..].starts_with("› ") {
                format!("{}{}›{RESET}{}", t.user, &s[..i], &s[i + '›'.len_utf8()..])
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
            _ => vis += 1,
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
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0))
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
            let pad = wid.saturating_sub(cell.chars().count()) + usize::from(c + 1 < cols);
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
            word_wrap(line, 80).iter().map(|r| colorize_row(st, &truncate_str(r, 80), '⠋')).collect()
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
        assert!(first("  ✗ edit failed").contains("\x1b[31m✗"), "tool fail must be red");
        assert!(first("  ⠋ cargo build").contains("\x1b[33m⠋"), "running must be yellow");
        assert!(first("  ⚠ interrupted").contains("\x1b[33m⚠"), "warn must be yellow");
        assert!(first("  ℹ renamed").contains("\x1b[34mℹ"), "info must be blue");
        assert!(first("  · 32 tok").starts_with(theme::current().dim), "stats must be dim");
        // a hunk row is a stats row until the sign is read, so the order of the
        // line_style arms is the whole feature: "  · - x" must not land on b's'
        assert!(first("  · -    2  let x = 1;").starts_with("\x1b[31m"), "a removed line must be red");
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
        assert!(rows.iter().all(|r| r.contains(theme::current().think)), "every row must be italic grey: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains('│')), "the sentinel bar is never drawn");
        // the answer is the plain one, so the two can never be confused
        assert!(!colorize_row(line_style("an answer"), "an answer", '⠋').contains(theme::current().think));
        // paragraph breaks inside a block survive and stay styled
        let two = "  │ first thought\n\nsecond thought";
        let st = line_style(two);
        assert!(word_wrap(two, 40).iter().all(|r| colorize_row(st, r, '⠋').contains(theme::current().think)));
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
}
