// TUI module: pi-style main-screen renderer with synchronized output.
// Differential rendering — only changed lines get redrawn.
// Always-visible input field, model name in status line.
// Falls back to plain stream for piped stdin.

mod app;
mod render;
mod plain;

use std::io::{self, Write, stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use crossterm::{cursor, execute, terminal};

use crate::ai_core::{self, llm};
use crate::session::Session;

// ── ANSI helpers ──

pub fn goto(f: &mut impl Write, x: u16, y: u16) { let _ = execute!(f, cursor::MoveTo(x, y)); }

/// Synchronized output begin — terminal batches writes until end.
const SYNC_BEGIN: &str = "\x1b[?2026h";
/// Synchronized output end — terminal flushes the batch atomically.
const SYNC_END: &str = "\x1b[?2026l";

struct AgentHandle(Option<JoinHandle<()>>);

impl Drop for AgentHandle {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() { let _ = h.join(); }
    }
}

/// Configuration for the TUI session.
pub struct TuiConfig {
    pub client: llm::Client,
    pub session: Session,
    pub model: String,
}

/// Jobs the TUI sends to the agent thread: a user task, a model switch,
/// resuming session.json, or dumping the current session tree.
pub enum Job {
    Task(String),
    Model { url: String, key: String, model: String },
    Resume,
    Tree,
}

/// Entry point: spawns agent in background thread, renders TUI or plain stream.
pub fn run(cfg: TuiConfig) -> io::Result<()> {
    let TuiConfig { client, session, model } = cfg;

    let (event_tx, event_rx) = mpsc::channel();
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    ai_core::set_event_sink(event_tx.clone());
    let cancel = Arc::new(AtomicBool::new(false)); // Esc sets it; agent thread checks it
    let cancel_agent = Arc::clone(&cancel);
    let mut agent = AgentHandle(Some(std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut session = session;
        let mut client = client;
        loop {
            match job_rx.recv() {
                Ok(Job::Task(task)) => {
                    cancel_agent.store(false, Ordering::Relaxed); // fresh turn = not cancelled
                    let ev = match rt.block_on(ai_core::run_agent(&client, &mut session, &task, &cancel_agent)) {
                        Ok(_) => ai_core::Event::TaskEnd { ok: true, error: None },
                        Err(e) => ai_core::Event::TaskEnd { ok: false, error: Some(e) },
                    };
                    let _ = event_tx.send(ev);
                }
                Ok(Job::Model { url, key, model }) => {
                    client = llm::Client::new(url, key, model.clone());
                    session.model = model.clone();
                    let _ = event_tx.send(ai_core::Event::Text(format!("  ✓ model switched to {model}")));
                }
                Ok(Job::Resume) => {
                    let loaded = Session::load();
                    if loaded.is_empty() {
                        let _ = event_tx.send(ai_core::Event::Text("  ✗ no session.json to resume".into()));
                    } else {
                        let n = loaded.entries.len();
                        session = loaded;
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ✓ resumed {n} entries from session.json")));
                    }
                }
                Ok(Job::Tree) => {
                    let path = session.path();
                    if path.is_empty() {
                        let _ = event_tx.send(ai_core::Event::Text("  ✗ no session yet".into()));
                    } else {
                        let mut out = String::new();
                        for (i, e) in path.iter().enumerate() {
                            let head = if e.role == "system" {
                                format!("{i}. system")
                            } else {
                                format!("{i}. {}: {}", e.role, one_line(&e.content))
                            };
                            out.push_str(&head);
                            out.push('\n');
                        }
                        let _ = event_tx.send(ai_core::Event::Text(out.trim_end().to_string()));
                    }
                }
                Err(_) => break, // TUI exited
            }
        }
    })));

    if is_terminal::is_terminal(std::io::stdin()) {
        terminal::enable_raw_mode()?;
        execute!(stdout(), cursor::Hide)?;
        let res = app::ui_loop(&job_tx, event_rx, model, &cancel);
        let _ = execute!(stdout(), cursor::Show);
        let _ = terminal::disable_raw_mode();
        agent.0.take().map(|h| h.join());
        res
    } else {
        plain::run(event_rx)
    }
}

/// First line of an entry, capped — for /tree output.
fn one_line(s: &str) -> String {
    s.replace('\n', " ").chars().take(80).collect()
}

/// Word wrap on byte length. Ponytail: naive — fine for ASCII.
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
