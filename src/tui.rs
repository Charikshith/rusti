// TUI front end: custom renderer using ANSI escape sequences (no ratatui).
// Agent runs in a background thread on its own tokio runtime, sends Events
// over a channel; this renders a scrolling live transcript and an input box
// for ask_user questions. Esc quits.

use std::io::{self, Write, stdout};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crossterm::{
    cursor,
    event::{self, Event as CEvent, KeyCode, KeyEventKind},
    execute,
    style::{Attribute, SetAttribute, ResetColor},
    terminal::{self, Clear, ClearType},
};

use crate::ai_core::{self, llm};
use crate::session::Session;

// ANSI helpers (inline — no framework dependency)

fn bold(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Bold)); }
fn reset(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Reset), ResetColor); }
fn goto(f: &mut impl Write, x: u16, y: u16) { let _ = execute!(f, cursor::MoveTo(x, y)); }
fn clear_screen(f: &mut impl Write) { let _ = execute!(f, Clear(ClearType::All)); }

pub fn run(client: llm::Client, task: String, mut session: Session) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    ai_core::set_event_sink(tx.clone());
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        match rt.block_on(ai_core::run_agent(&client, &mut session, &task)) {
            Ok(_) => {}
            Err(e) => { let _ = tx.send(ai_core::Event::Text(format!("[error] {e}"))); }
        }
        let _ = tx.send(ai_core::Event::Done);
    });

    if is_terminal::is_terminal(std::io::stdin()) {
        // full TUI: alternate screen + raw mode
        terminal::enable_raw_mode()?;
        execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        let res = ui_loop(rx);
        let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        res
    } else {
        // plain stream: drain events to stdout (piped stdin, SSH, CI)
        plain_loop(rx)
    }
}

struct App {
    lines: Vec<String>,
    current: String,
    ask: Option<(String, tokio::sync::oneshot::Sender<String>)>,
    input: String,
    done: bool,
}

impl App {
    fn flush(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }
}

fn ui_loop(rx: Receiver<ai_core::Event>) -> io::Result<()> {
    let mut app = App {
        lines: Vec::new(), current: String::new(),
        ask: None, input: String::new(), done: false,
    };

    loop {
        render(&app)?;

        if event::poll(Duration::from_millis(50))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Esc => break,
                        KeyCode::Enter if app.ask.is_some() => {
                            let (_, reply) = app.ask.take().unwrap();
                            let ans = app.input.trim().to_string();
                            app.lines.push(format!("> {ans}"));
                            let _ = reply.send(ans);
                            app.input.clear();
                        }
                        KeyCode::Char(c) if app.ask.is_some() => app.input.push(c),
                        KeyCode::Backspace if app.ask.is_some() => { app.input.pop(); }
                        _ => {}
                    }
                }
            }
        }

        while let Ok(ev) = rx.try_recv() {
            match ev {
                ai_core::Event::TextDelta(t) => app.current.push_str(&t),
                ai_core::Event::Text(t) => { app.flush(); app.lines.push(t); }
                ai_core::Event::Tool(t) => { app.flush(); app.lines.push(format!("  ↳ {t}")); }
                ai_core::Event::Ask { question, reply } => {
                    app.flush();
                    app.lines.push(format!("? {question}"));
                    app.ask = Some((question, reply));
                }
                ai_core::Event::Done => {
                    app.flush();
                    app.done = true;
                    app.lines.push("[done]".into());
                }
            }
        }
    }
    Ok(())
}

/// Plain stream mode: drain events to stdout without raw mode.
/// Works with piped stdin, SSH sessions, CI, dumb terminals.
fn plain_loop(rx: Receiver<ai_core::Event>) -> io::Result<()> {
    let mut out = io::stdout();
    let mut err = io::stderr();
    loop {
        match rx.recv() {
            Ok(ai_core::Event::TextDelta(t)) => { write!(out, "{t}")?; out.flush()?; }
            Ok(ai_core::Event::Text(t)) => { writeln!(out, "{t}")?; }
            Ok(ai_core::Event::Tool(t)) => { writeln!(out, "  ↳ {t}")?; }
            Ok(ai_core::Event::Ask { question, reply }) => {
                writeln!(err, "? {question}")?;
                write!(err, "> ")?;
                err.flush()?;
                let mut line = String::new();
                io::stdin().read_line(&mut line)?;
                let ans = line.trim().to_string();
                writeln!(out, "> {ans}")?;
                let _ = reply.send(ans);
            }
            Ok(ai_core::Event::Done) => break,
            Err(_) => break,
        }
    }
    Ok(())
}

fn render(app: &App) -> io::Result<()> {
    let (w, h) = terminal::size()?;
    let w = w as usize;
    let h = h as usize;
    let inner_w = w.saturating_sub(2); // border columns

    let mut out = stdout();
    clear_screen(&mut out);

    // ── top border ──
    goto(&mut out, 0, 0);
    bold(&mut out);
    write!(out, "┌─ rustypi ")?;
    for _ in 0..w.saturating_sub(13) { write!(out, "─")?; }
    writeln!(out, "┐")?;
    reset(&mut out);

    // ── transcript area ──
    let input_h = if app.ask.is_some() { 4 } else { 1 }; // bottom section height (including borders)
    let transcript_h = h.saturating_sub(2 + input_h);     // minus top/bottom borders + input section

    // word-wrap all lines + current streaming text
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
            let _col = 1 + chars.len();
            for c in &chars { write!(out, "{c}")?; }
            // pad to fill width + closing border
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

// ponytail: naive word wrap on byte length — fine for ASCII; switch to
// unicode-width if CJK/emoji output ever garbles.
fn word_wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![String::new()]; }
    let mut out = Vec::new();
    for src in s.split('\n') {
        let mut line = String::new();
        for word in src.split(' ') {
            if line.is_empty() {
                line = word.to_string();
            } else if line.len() + 1 + word.len() > width {
                out.push(line);
                line = word.to_string();
            } else {
                line.push(' ');
                line.push_str(word);
            }
        }
        out.push(line);
    }
    out
}
