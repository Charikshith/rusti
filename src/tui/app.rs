// App state + main-screen event loop (pi-style).
// No alternate screen — transcript stays in terminal scrollback.
// Always-visible input field; model name in bottom status line.
// Keyboard: Esc/Ctrl+C quit, Enter submit, arrows edit input, Up/Down recall
// history, PageUp/PageDown scroll transcript. Slash commands: /use /resume /tree.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};

use crate::ai_core;
use super::render::{self, RenderState};
use super::Job;

pub struct App {
    pub lines: Vec<String>,
    pub current: String,
    pub ask: Option<(String, tokio::sync::oneshot::Sender<String>)>,
    pub input: String,
    pub cursor: usize,        // char index into input
    pub done: bool,
    pub model: String,
    pub msg_num: usize,       // user-message counter for "N› " prefixes
    pub spinner: usize,       // status-line spinner frame
    pub scroll_up: usize,     // transcript lines pinned above the bottom
    pub history: Vec<String>,
    pub hist_idx: Option<usize>,
    pub tool_line: Option<usize>, // index of the active "⠋" tool line
}

impl App {
    pub fn flush(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }
}

pub fn ui_loop(
    job_tx: &Sender<Job>,
    rx: Receiver<ai_core::Event>,
    model: String,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let mut app = App {
        lines: Vec::new(), current: String::new(),
        ask: None, input: String::new(), cursor: 0, done: true, model,
        msg_num: 0, spinner: 0, scroll_up: 0,
        history: Vec::new(), hist_idx: None, tool_line: None,
    };
    let mut state = RenderState::new();

    loop {
        render::draw(&app, &mut state)?;
        app.spinner = app.spinner.wrapping_add(1);

        if event::poll(Duration::from_millis(50))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match (k.code, k.modifiers) {
                        // Esc interrupts the running turn (pi: cancel, never quit)
                        (KeyCode::Esc, _) => {
                            if !app.done {
                                cancel.store(true, Ordering::Relaxed);
                                // dismiss a pending question so the agent unblocks
                                if let Some((_, reply)) = app.ask.take() {
                                    let _ = reply.send("interrupted".into());
                                }
                            }
                        }
                        // Ctrl+C clears the input line (pi: clear editor)
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            app.input.clear();
                            app.cursor = 0;
                        }
                        // Ctrl+D exits, only when input is empty (pi: exit when editor empty)
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            if app.input.is_empty() {
                                if !app.done {
                                    cancel.store(true, Ordering::Relaxed); // don't wait for the turn
                                }
                                break;
                            }
                        }

                        // Submit answer to Ask
                        (KeyCode::Enter, _) if app.ask.is_some() => {
                            let (_, reply) = app.ask.take().unwrap();
                            let ans = app.input.trim().to_string();
                            app.msg_num += 1;
                            app.lines.push(format!("{}› {ans}", app.msg_num));
                            app.input.clear();
                            app.cursor = 0;
                            let _ = reply.send(ans);
                        }

                        // Enter: slash command or task
                        (KeyCode::Enter, _) if !app.input.trim().is_empty() => {
                            let raw = app.input.trim().to_string();
                            app.input.clear();
                            app.cursor = 0;
                            if raw.starts_with('/') {
                                handle_command(&raw, &mut app, job_tx);
                            } else {
                                app.flush();
                                app.msg_num += 1;
                                app.lines.push(format!("{}› {raw}", app.msg_num));
                                app.current.clear();
                                app.scroll_up = 0;
                                app.done = false;
                                if app.history.last().map(|h| h != &raw).unwrap_or(true) {
                                    app.history.push(raw.clone());
                                }
                                let _ = job_tx.send(Job::Task(raw));
                            }
                        }

                        // text editing (insert at cursor)
                        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                            let byte = app.input.char_indices().nth(app.cursor).map(|(i, _)| i).unwrap_or(app.input.len());
                            app.input.insert(byte, c);
                            app.cursor += 1;
                        }
                        (KeyCode::Backspace, _) => {
                            if app.cursor > 0 {
                                let byte = app.input.char_indices().nth(app.cursor - 1).map(|(i, _)| i).unwrap_or(0);
                                app.input.remove(byte);
                                app.cursor -= 1;
                            }
                        }
                        (KeyCode::Delete, _) => {
                            if app.cursor < app.input.chars().count() {
                                let byte = app.input.char_indices().nth(app.cursor).map(|(i, _)| i).unwrap_or(app.input.len());
                                app.input.remove(byte);
                            }
                        }
                        (KeyCode::Left, _) => { app.cursor = app.cursor.saturating_sub(1); }
                        (KeyCode::Right, _) => { if app.cursor < app.input.chars().count() { app.cursor += 1; } }
                        (KeyCode::Home, _) => { app.cursor = 0; }
                        (KeyCode::End, _) => { app.cursor = app.input.chars().count(); }

                        // history recall (not while answering Ask)
                        (KeyCode::Up, _) if !app.ask.is_some() && !app.history.is_empty() => {
                            let idx = match app.hist_idx {
                                None => app.history.len() - 1,
                                Some(0) => 0,
                                Some(i) => i - 1,
                            };
                            app.hist_idx = Some(idx);
                            app.input = app.history[idx].clone();
                            app.cursor = app.input.chars().count();
                        }
                        (KeyCode::Down, _) if !app.ask.is_some() && app.hist_idx.is_some() => {
                            let i = app.hist_idx.unwrap();
                            if i + 1 < app.history.len() {
                                app.hist_idx = Some(i + 1);
                                app.input = app.history[i + 1].clone();
                            } else {
                                app.hist_idx = None;
                                app.input.clear();
                            }
                            app.cursor = app.input.chars().count();
                        }

                        // transcript scrollback (0 = follow bottom)
                        (KeyCode::PageUp, _) => { app.scroll_up += 10; }
                        (KeyCode::PageDown, _) => { app.scroll_up = app.scroll_up.saturating_sub(10); }
                        _ => {}
                    }
                }
            }
        }

        while let Ok(ev) = rx.try_recv() {
            match ev {
                ai_core::Event::TextDelta(t) => app.current.push_str(&t),
                ai_core::Event::Text(t) => { app.flush(); app.lines.push(t); }
                ai_core::Event::ToolStart(t) => {
                    app.flush();
                    app.lines.push(format!("  ⠋ {t}"));
                    app.tool_line = Some(app.lines.len() - 1);
                }
                ai_core::Event::ToolEnd { summary, ok } => {
                    let line = format!("  {} {summary}", if ok { "✓" } else { "✗" });
                    match app.tool_line.take() {
                        Some(i) if i < app.lines.len() && app.lines[i].starts_with("  ⠋ ") => app.lines[i] = line,
                        _ => app.lines.push(line),
                    }
                }
                ai_core::Event::Ask { question, reply } => {
                    app.flush();
                    app.lines.push(format!("  ℹ {question}"));
                    app.ask = Some((question, reply));
                }
                ai_core::Event::TaskEnd { ok, error } => {
                    app.flush();
                    // success is self-evident (the answer ends the turn);
                    // only failures get a marker, with the reason
                    if !ok {
                        let line = match error.as_deref() {
                            Some("interrupted") => "  ⚠ interrupted".to_string(),
                            Some(e) => format!("  ✗ Task failed: {e}"),
                            None => "  ✗ Task failed".to_string(),
                        };
                        app.lines.push(line);
                    }
                    app.done = true;
                    app.scroll_up = 0;
                    if !app.lines.is_empty() {
                        app.lines.push(String::new());
                    }
                }
            }
        }
    }
    Ok(())
}

/// Slash commands: /use <name> switches model, /resume loads session.json,
/// /tree dumps the current session path.
fn handle_command(raw: &str, app: &mut App, job_tx: &Sender<Job>) {
    let mut parts = raw.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/use" => {
            if arg.is_empty() {
                app.lines.push("  ✗ usage: /use <name> (see --list)".into());
                return;
            }
            let cfg = crate::config::Config::load();
            match cfg.models.iter().find(|m| m.name == arg) {
                Some(p) => {
                    app.model = p.model.clone();
                    let _ = job_tx.send(Job::Model {
                        url: p.url.clone(),
                        key: p.key.clone(),
                        model: p.model.clone(),
                    });
                }
                None => app.lines.push(format!("  ✗ no saved model named '{arg}' (see --list)")),
            }
        }
        "/resume" => { let _ = job_tx.send(Job::Resume); }
        "/tree" => { let _ = job_tx.send(Job::Tree); }
        _ => app.lines.push(format!("  ✗ unknown command: {cmd}")),
    }
}
