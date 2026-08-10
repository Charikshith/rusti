// TUI module: custom ANSI renderer + plain stream fallback.
// No ratatui — direct escape sequences, ~2.6 MB binary.

mod app;
mod render;
mod plain;

use std::io::{self, Write, stdout};
use std::sync::mpsc;

use crossterm::{cursor, execute, terminal, style::{Attribute, SetAttribute, ResetColor}};

use crate::ai_core::{self, llm};
use crate::session::Session;

// ── ANSI helpers (shared by render + plain) ──

pub fn bold(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Bold)); }
pub fn reset(f: &mut impl Write) { let _ = execute!(f, SetAttribute(Attribute::Reset), ResetColor); }
pub fn goto(f: &mut impl Write, x: u16, y: u16) { let _ = execute!(f, cursor::MoveTo(x, y)); }

/// Entry point: spawns the agent in a background thread, then renders
/// the TUI (interactive) or streams to stdout (piped stdin).
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
        terminal::enable_raw_mode()?;
        execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        let res = app::ui_loop(rx);
        let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        res
    } else {
        plain::run(rx)
    }
}

// ponytail: naive word wrap on byte length — fine for ASCII; switch to
// unicode-width if CJK/emoji output ever garbles.
pub fn word_wrap(s: &str, width: usize) -> Vec<String> {
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
