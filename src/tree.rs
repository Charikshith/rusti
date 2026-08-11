// Session-tree browser: pi-style main-screen renderer with diff updates.
// Synchronized output (CSI 2026) for atomic flicker-free rendering.
// Plain numbered list fallback for piped stdin.

use std::collections::HashMap;
use std::io::{Write, stdout};

use crossterm::{
    cursor,
    event::{self, Event as CEvent, KeyCode, KeyEventKind},
    execute,
    style::{Attribute, SetAttribute},
    terminal::{self, Clear, ClearType},
};

use crate::session::Session;

// ANSI helpers
fn bold(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Bold)); }
fn reset(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Reset)); }
fn goto(f: &mut impl Write, x: u16, y: u16) { let _ = execute!(f, cursor::MoveTo(x, y)); }

const SYNC_BEGIN: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";

/// Returns the prefilled prompt text, or None if cancelled.
pub fn browse(session: &mut Session) -> Option<String> {
    if is_terminal::is_terminal(std::io::stdin()) {
        browse_tui(session)
    } else {
        browse_plain(session)
    }
}

// ---------------------------------------------------------------------------
// Plain numbered list (piped stdin, SSH, dumb terminals)
// ---------------------------------------------------------------------------

fn browse_plain(session: &mut Session) -> Option<String> {
    let rows = rows(session);
    eprintln!("session tree:\n");
    for (i, (_, sel, label)) in rows.iter().enumerate() {
        eprintln!("  {} {}", if *sel { format!("{}.", i + 1) } else { "  ".into() }, label);
    }
    eprintln!("\n  enter number, or empty to cancel: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let n: usize = line.trim().parse().ok()?;
    if n == 0 || n > rows.len() || !rows[n - 1].1 {
        return None;
    }
    let id = rows[n - 1].0.clone();
    session.select(&id)
}

// ---------------------------------------------------------------------------
// Main-screen TUI with differential rendering (pi-style)
// ---------------------------------------------------------------------------

fn browse_tui(session: &mut Session) -> Option<String> {
    let rows = rows(session);
    let mut idx = rows.iter().rposition(|r| r.1).unwrap_or(0);
    let mut scroll = 0usize;
    let mut prev_frame: Vec<String> = Vec::new();
    let mut prev_h: usize = 0;

    terminal::enable_raw_mode().ok()?;
    let mut out = stdout();
    let _ = execute!(out, cursor::Hide);

    let picked: Option<usize> = loop {
        let (w, h) = terminal::size().unwrap_or((80, 24));
        let w = w as usize;
        let h = h as usize;
        let inner_w = w.saturating_sub(2);
        let list_h = h.saturating_sub(4); // title + list + hint + bottom border

        if idx < scroll { scroll = idx; }
        if idx >= scroll + list_h { scroll = idx + 1 - list_h; }

        // ── compose frame ──
        let mut frame: Vec<String> = Vec::with_capacity(h);

        // title border
        let mut top = String::from("┌─ session tree ");
        top.extend(std::iter::repeat('─').take(w.saturating_sub(17)));
        top.push('┐');
        frame.push(top);

        // list rows
        for i in 0..list_h {
            let ri = scroll + i;
            let mut row = String::from("│");
            if ri < rows.len() {
                let (ref _id, sel, ref label) = rows[ri];
                if ri == idx {
                    let marker = if sel { "▶ " } else { "  " };
                    let text: String = label.chars().take(inner_w.saturating_sub(2)).collect();
                    let content = format!("{marker}{text}");
                    let padded = format!("{content:<width$}", width = inner_w);
                    row.push_str(&padded);
                } else {
                    let prefix = "  ";
                    let text: String = label.chars().take(inner_w.saturating_sub(2)).collect();
                    row.push_str(prefix);
                    row.push_str(&text);
                    let used = prefix.len() + text.len();
                    if used < inner_w {
                        row.extend(std::iter::repeat(' ').take(inner_w - used));
                    }
                }
            } else {
                row.extend(std::iter::repeat(' ').take(inner_w));
            }
            row.push('│');
            frame.push(row);
        }

        // separator
        let mut sep = String::from("├");
        sep.extend(std::iter::repeat('─').take(w.saturating_sub(2)));
        sep.push('┤');
        frame.push(sep);

        // hint
        let hint = "↑/↓ select · Enter branch · Esc cancel";
        let mut hrow = format!("│ {hint}");
        let used = hrow.len();
        if used < w - 1 { hrow.extend(std::iter::repeat(' ').take(w - 1 - used)); }
        hrow.push('│');
        frame.push(hrow);

        // bottom border
        let mut bot = String::from("└");
        bot.extend(std::iter::repeat('─').take(w.saturating_sub(2)));
        bot.push('┘');
        frame.push(bot);

        // ── differential draw ──
        out.write_all(SYNC_BEGIN.as_bytes()).ok();

        let resized = h != prev_h;
        if resized || prev_frame.is_empty() {
            let _ = execute!(out, Clear(ClearType::All));
            for (i, line) in frame.iter().enumerate() {
                goto(&mut out, 0, i as u16);
                write!(out, "{line}").ok();
                let _ = execute!(out, Clear(ClearType::UntilNewLine));
            }
        } else {
            let max = frame.len().max(prev_frame.len());
            for i in 0..max {
                let new = frame.get(i).map(|s| s.as_str()).unwrap_or("");
                let old = prev_frame.get(i).map(|s| s.as_str()).unwrap_or("");
                if new != old {
                    goto(&mut out, 0, i as u16);
                    write!(out, "{new}").ok();
                    let _ = execute!(out, Clear(ClearType::UntilNewLine));
                }
            }
            for i in frame.len()..prev_frame.len() {
                goto(&mut out, 0, i as u16);
                let _ = execute!(out, Clear(ClearType::UntilNewLine));
            }
        }

        // bold the selected row inline
        let sel_y = (1 + idx - scroll) as u16;
        goto(&mut out, 1, sel_y);
        bold(&mut out);
        if idx < rows.len() {
            let (ref _id, sel, ref label) = rows[idx];
            let marker = if sel { "▶ " } else { "  " };
            let text: String = label.chars().take(inner_w.saturating_sub(2)).collect();
            write!(out, "{marker}{text}").ok();
        }
        reset(&mut out);

        out.write_all(SYNC_END.as_bytes()).ok();
        out.flush().ok()?;

        prev_frame = frame;
        prev_h = h;

        // ── input ──
        if event::poll(std::time::Duration::from_millis(100)).ok()? {
            if let CEvent::Key(k) = event::read().ok()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Esc => break None,
                        KeyCode::Enter => break Some(idx),
                        KeyCode::Up => {
                            for i in (0..idx).rev() {
                                if rows[i].1 { idx = i; break; }
                            }
                        }
                        KeyCode::Down => {
                            for i in (idx + 1)..rows.len() {
                                if rows[i].1 { idx = i; break; }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    let _ = execute!(out, cursor::Show);
    let _ = terminal::disable_raw_mode();

    picked.map(|i| {
        let id = rows[i].0.clone();
        session.select(&id).unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn rows(session: &Session) -> Vec<(String, bool, String)> {
    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for e in &session.entries {
        let d = e.parent.as_ref().and_then(|p| depth.get(p)).copied().unwrap_or(0) + 1;
        depth.insert(e.id.clone(), d);
        let snippet: String = e.content.lines().next().unwrap_or("").chars().take(60).collect();
        let role = if e.role == "tool" { "↳ tool".to_string() } else { e.role.clone() };
        let mark = if session.active.as_deref() == Some(e.id.as_str()) { " ◀" } else { "" };
        let selectable = e.role == "user" || e.role == "assistant";
        out.push((e.id.clone(), selectable, format!("{}{} {}{}", "  ".repeat(d), role, snippet, mark)));
    }
    out
}
