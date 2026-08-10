// ANSI renderer: full-screen redraw with word wrap, borders, streaming text.

use std::io::{self, Write, stdout};

use crossterm::{
    execute,
    terminal::{self, Clear, ClearType},
};

use super::app::App;
use super::{bold, goto, reset, word_wrap};

pub fn draw(app: &App) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    let w = w as usize;
    let h = h as usize;
    let inner_w = w.saturating_sub(2);

    let mut out = stdout();
    let _ = execute!(out, Clear(ClearType::All));

    // ── top border ──
    goto(&mut out, 0, 0);
    bold(&mut out);
    write!(out, "┌─ rustypi ")?;
    for _ in 0..w.saturating_sub(13) { write!(out, "─")?; }
    writeln!(out, "┐")?;
    reset(&mut out);

    // ── transcript area ──
    let input_h = if app.ask.is_some() { 4 } else { 1 };
    let transcript_h = h.saturating_sub(2 + input_h);

    let mut all: Vec<String> = Vec::new();
    for l in &app.lines { all.extend(word_wrap(l, inner_w)); }
    if !app.current.is_empty() { all.extend(word_wrap(&app.current, inner_w)); }

    let scroll = all.len().saturating_sub(transcript_h);
    for i in 0..transcript_h {
        let y = 1 + i as u16;
        goto(&mut out, 0, y);
        write!(out, "│")?;
        let li = scroll + i;
        if li < all.len() {
            let line = &all[li];
            let chars: Vec<char> = line.chars().take(inner_w).collect();
            for c in &chars { write!(out, "{c}")?; }
            goto(&mut out, (w - 1) as u16, y);
        } else {
            goto(&mut out, (w - 1) as u16, y);
        }
        write!(out, "│")?;
    }

    // ── bottom separator ──
    let sep_y = (1 + transcript_h) as u16;
    goto(&mut out, 0, sep_y);
    write!(out, "├")?;
    for _ in 0..w.saturating_sub(2) { write!(out, "─")?; }
    writeln!(out, "┤")?;

    // ── bottom section ──
    if let Some((q, _)) = &app.ask {
        let y = sep_y + 1;
        goto(&mut out, 0, y);
        let qline: String = q.chars().take(inner_w).collect();
        write!(out, "│ ? {qline}")?;
        goto(&mut out, (w - 1) as u16, y);
        writeln!(out, "│")?;

        let y2 = sep_y + 2;
        goto(&mut out, 0, y2);
        let inp: String = app.input.chars().take(inner_w.saturating_sub(2)).collect();
        write!(out, "│ > {inp}▌")?;
        goto(&mut out, (w - 1) as u16, y2);
        writeln!(out, "│")?;

        goto(&mut out, 0, sep_y + 3);
        write!(out, "└")?;
        for _ in 0..w.saturating_sub(2) { write!(out, "─")?; }
        writeln!(out, "┘")?;
    } else {
        let hint = if app.done { "esc to quit" } else { "working… esc to quit" };
        goto(&mut out, 0, sep_y + 1);
        write!(out, "│ {hint}")?;
        goto(&mut out, (w - 1) as u16, sep_y + 1);
        writeln!(out, "│")?;

        goto(&mut out, 0, sep_y + 2);
        write!(out, "└")?;
        for _ in 0..w.saturating_sub(2) { write!(out, "─")?; }
        writeln!(out, "┘")?;
    }

    out.flush()
}
