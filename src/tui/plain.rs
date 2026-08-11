// Plain stream mode: drain events to stdout/stderr without raw mode.
// Works with piped stdin, SSH sessions, CI, dumb terminals.

use std::io::{self, Write};
use std::sync::mpsc::Receiver;

use crate::ai_core;

pub fn run(rx: Receiver<ai_core::Event>) -> io::Result<()> {
    let mut out = io::stdout();
    let mut err = io::stderr();
    loop {
        match rx.recv() {
            Ok(ai_core::Event::TextDelta(t)) => { write!(out, "{t}")?; out.flush()?; }
            Ok(ai_core::Event::Text(t)) => { writeln!(out, "{t}")?; }
            Ok(ai_core::Event::ToolStart(t)) => { writeln!(out, "  ⠋ {t}")?; }
            Ok(ai_core::Event::ToolEnd { summary, ok }) => { writeln!(out, "  {} {summary}", if ok { "✓" } else { "✗" })?; }
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
            Ok(ai_core::Event::TaskEnd { .. }) => break,
            Err(_) => break,
        }
    }
    Ok(())
}
