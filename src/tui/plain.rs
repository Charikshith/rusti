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
            Ok(ai_core::Event::ReasoningDelta(t)) => { write!(err, "{t}")?; err.flush()?; }
            Ok(ai_core::Event::Text(t)) => { writeln!(out, "{t}")?; }
            Ok(ai_core::Event::ToolStart(t)) => { writeln!(out, "  ⠋ {t}")?; }
            Ok(ai_core::Event::ToolEnd { summary, ok, ms, output }) => {
                writeln!(out, "  {} {summary}  {}", if ok { "✓" } else { "✗" }, ai_core::took(ms))?;
                for l in ai_core::fail_tail(&output) { writeln!(out, "{l}")?; }
            }
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
            Ok(ai_core::Event::Resumed { .. }) => {} // /resume is a TUI slash command
            Ok(ai_core::Event::SessionName(_)) => {} // TUI status-line label
            Ok(ai_core::Event::Notice(t)) => { writeln!(out, "  ℹ {t}")?; }
            // a pipe has no line to rewrite, so each attempt gets its own
            Ok(ai_core::Event::Retry { attempt, of, wait_ms, err }) => {
                writeln!(out, "  ⚠ {err} — retry {attempt}/{of} in {:.1}s", wait_ms as f64 / 1000.0)?;
            }
            Ok(ai_core::Event::Tree(_) | ai_core::Event::Prefill(_) | ai_core::Event::Git(_)) => {} // TUI-only
            Ok(ai_core::Event::Usage { .. }) => {} // per-turn stats are a TUI line
            Ok(ai_core::Event::TaskEnd { .. }) => break,
            Ok(ai_core::Event::Reload { .. }) => {} // /reload is a TUI-only slash command
            Err(_) => break,
        }
    }
    Ok(())
}
