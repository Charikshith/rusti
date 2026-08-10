// Session-tree browser: custom ANSI TUI for interactive terminals, plain
// numbered list for piped/non-interactive stdin. Selection semantics
// match pi's /tree:
//   user message  → leaf moves to parent, text returned as prefill
//   assistant msg → leaf moves to that entry, empty prefill

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

/// Returns the prefilled prompt text, or None if cancelled.
pub fn browse(session: &mut Session) -> Option<String> {
    if is_terminal::is_terminal(std::io::stdin()) {
        browse_tui(session)
    } else {
        browse_plain(session)
    }
}

// ---------------------------------------------------------------------------
// Plain numbered list (works with piped stdin, SSH, dumb terminals)
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
// Custom ANSI TUI (interactive terminals only)
// ---------------------------------------------------------------------------

fn browse_tui(session: &mut Session) -> Option<String> {
    let rows = rows(session);
    let mut idx = rows.iter().rposition(|r| r.1).unwrap_or(0);
    let mut scroll = 0usize;

    terminal::enable_raw_mode().ok()?;
    let mut out = stdout();
    let _ = execute!(out, terminal::EnterAlternateScreen, cursor::Hide);

    let picked: Option<usize> = loop {
        let (w, h) = terminal::size().unwrap_or((80, 24));
        let w = w as usize;
        let h = h as usize;
        let inner_w = w.saturating_sub(2);
        let list_h = h.saturating_sub(3); // minus title border + hint + bottom border

        // auto-scroll to keep selection visible
        if idx < scroll { scroll = idx; }
        if idx >= scroll + list_h { scroll = idx + 1 - list_h; }

        // draw
        execute!(out, Clear(ClearType::All)).ok()?;

        // title
        goto(&mut out, 0, 0);
        bold(&mut out);
        write!(out, "┌─ session tree ").ok();
        for _ in 0..w.saturating_sub(18) { write!(out, "─").ok(); }
        writeln!(out, "┐").ok();
        reset(&mut out);

        // list rows
        for i in 0..list_h {
            let y = 1 + i as u16;
            goto(&mut out, 0, y);
            write!(out, "│").ok();
            let ri = scroll + i;
            if ri < rows.len() {
                let (ref _id, sel, ref label) = rows[ri];
                goto(&mut out, 1, y);
                if ri == idx {
                    // highlighted row: bold + reverse
                    let _ = execute!(out,
                        SetAttribute(Attribute::Bold),
                        crossterm::style::SetAttribute(Attribute::Reverse),
                    );
                    let marker = if sel { "▶ " } else { "  " };
                    let text: String = label.chars().take(inner_w.saturating_sub(2)).collect();
                    write!(out, "{marker}{text}").ok();
                    // pad to fill
                    let used = marker.len() + text.chars().count();
                    if used < inner_w {
                        for _ in 0..(inner_w - used) { write!(out, " ").ok(); }
                    }
                    reset(&mut out);
                } else {
                    let prefix = if sel { "  " } else { "  " };
                    let text: String = label.chars().take(inner_w.saturating_sub(2)).collect();
                    write!(out, "{prefix}{text}").ok();
                }
            }
            goto(&mut out, (w - 1) as u16, y);
            write!(out, "│").ok();
        }

        // bottom border + hint
        let sep_y = 1 + list_h as u16;
        goto(&mut out, 0, sep_y);
        write!(out, "├").ok();
        for _ in 0..w.saturating_sub(2) { write!(out, "─").ok(); }
        writeln!(out, "┤").ok();

        goto(&mut out, 1, sep_y + 1);
        write!(out, "↑/↓ select · Enter branch · Esc cancel").ok();
        goto(&mut out, 0, sep_y + 2);
        write!(out, "└").ok();
        for _ in 0..w.saturating_sub(2) { write!(out, "─").ok(); }
        write!(out, "┘").ok();

        out.flush().ok()?;

        // input
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

    let _ = execute!(out, cursor::Show, terminal::LeaveAlternateScreen);
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
