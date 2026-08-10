// App state + interactive event loop.

use std::io;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};

use crate::ai_core;
use super::render;

pub struct App {
    pub lines: Vec<String>,
    pub current: String,
    pub ask: Option<(String, tokio::sync::oneshot::Sender<String>)>,
    pub input: String,
    pub done: bool,
}

impl App {
    pub fn flush(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }
}

pub fn ui_loop(rx: Receiver<ai_core::Event>) -> io::Result<()> {
    let mut app = App {
        lines: Vec::new(), current: String::new(),
        ask: None, input: String::new(), done: false,
    };

    loop {
        render::draw(&app)?;

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
