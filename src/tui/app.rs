// App state + event loop (pi-style), rendered on the alternate screen.
// Transcript scrolls inside the app (PageUp/PageDown); the shell's primary
// buffer is untouched while the TUI runs.
// Always-visible input field; model name in bottom status line.
// Keyboard: Esc interrupts, Ctrl+C clears (twice quits), Ctrl+D quits when the
// input is empty, Enter submits, arrows edit input, Up/Down recall history,
// PageUp/PageDown scroll transcript.
// Slash: /use /model /resume /rename /tree /reload /quit.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};

use crate::ai_core;
use super::render::{self, RenderState};
use super::Job;

/// One row of the slash-command menu. `soon` marks a command that is queued in
/// harness/open-work.md but not built yet, so the menu stays honest.
pub struct Cmd {
    pub name: &'static str,
    pub desc: &'static str,
    pub soon: bool,
}

pub const CMDS: &[Cmd] = &[
    Cmd { name: "/model", desc: "pick from saved profiles, or /model <name> to switch", soon: false },
    Cmd { name: "/use", desc: "/use <name> - switch to a saved profile", soon: false },
    Cmd { name: "/resume", desc: "list sessions and switch: /resume, /resume <name>, /resume <n>", soon: false },
    Cmd { name: "/rename", desc: "/rename <new-name> - rename the active session", soon: false },
    Cmd { name: "/tree", desc: "browse the session tree and branch from an earlier entry", soon: false },
    Cmd { name: "/reload", desc: "rebuild rusti from source and relaunch", soon: false },
    Cmd { name: "/quit", desc: "exit rusti (same as ctrl+c twice)", soon: false },
    Cmd { name: "/undo", desc: "put back the files the last turn changed and rewind to before it", soon: false },
    Cmd { name: "/commit", desc: "stage the work and commit it with a drafted message", soon: false },
    Cmd { name: "/plan", desc: "toggle plan mode: read and propose, change nothing", soon: false },
    Cmd { name: "/settings", desc: "choose which segments the status line shows", soon: false },
    Cmd { name: "/mcp", desc: "list MCP servers and switch them on or off", soon: false },
    Cmd { name: "/test", desc: "/test <cmd> - loop until it exits 0", soon: true },
    Cmd { name: "/export", desc: "/export [file.md] - write the transcript out as markdown", soon: false },
];

/// Menu rows visible at once; up/down walks the whole filtered list.
pub const MENU_ROWS: usize = 5;

/// /commit is a prompt macro — the model already has git and the tools; the
/// system prompt already carries `git status --short`.
const COMMIT_TASK: &str = "Review the working tree with git status and git diff, then stage the files \
belonging to the work we just did and create one commit. Write a concise message saying why the change \
was made, not just what changed. Do not push.";

/// Picker rows visible at once; up/down scrolls the window for long lists.
pub const PICK_ROWS: usize = 8;

/// Commands matching the input, while it is a lone "/word" — a space means the
/// command is typed and its arguments have started, so the menu closes.
pub fn menu_items(app: &App) -> Vec<&'static Cmd> {
    if app.pick.is_some() || app.ask.is_some() || app.menu_off.as_deref() == Some(app.input.as_str()) {
        return Vec::new();
    }
    filter_cmds(&app.input)
}

/// Commands whose name completes `input`, or none when `input` isn't a lone
/// "/word" (a space means the arguments have started).
pub fn filter_cmds(input: &str) -> Vec<&'static Cmd> {
    let Some(q) = input.strip_prefix('/') else { return Vec::new() };
    if q.contains(char::is_whitespace) {
        return Vec::new();
    }
    let q = q.to_lowercase();
    CMDS.iter().filter(|c| c.name[1..].to_lowercase().starts_with(&q)).collect()
}

/// What an open list picker selects (Enter's action).
#[derive(PartialEq)]
pub enum PickKind {
    Session,  // resume the chosen session file
    Model,    // switch to the chosen model profile
    Tree,     // branch the session at the chosen entry
    Settings, // flip a status-line segment; the only kind Enter does not close
    Mcp,      // connect/disconnect an MCP server; also stays open on Enter
}

/// An open list picker (/resume with no argument, /model with no argument).
/// Rows are (label, value): a session path or a model profile name.
pub struct Pick {
    pub kind: PickKind,
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub idx: usize,   // indexes visible(), not rows
    pub top: usize,   // first visible row (window for long lists)
    pub filter: String, // typed while the picker is open; substring, case-insensitive
}

impl Pick {
    /// Rows matching the typed filter. 37 model profiles is a scroll; three
    /// letters is not.
    pub fn visible(&self) -> Vec<&(String, String)> {
        if self.filter.is_empty() {
            return self.rows.iter().collect();
        }
        let f = self.filter.to_lowercase();
        self.rows.iter().filter(|(label, _)| label.to_lowercase().contains(&f)).collect()
    }
}

/// How ui_loop ended: a plain quit, or a /reload handoff to a new process.
pub enum Exit {
    Quit,
    Reload { exe: String, args: Vec<String> },
}

pub struct App {
    pub lines: Vec<String>,
    pub current: String,
    pub ask: Option<(String, tokio::sync::oneshot::Sender<String>)>,
    pub input: String,
    pub cursor: usize,        // char index into input
    pub done: bool,
    pub model: String,
    pub session: String, // active session name (file stem), shown in the status line
    pub msg_num: usize,       // user-message counter for "N› " prefixes
    pub spinner: usize,       // status-line spinner frame
    pub scroll_up: usize,     // transcript lines pinned above the bottom
    pub history: Vec<String>,
    pub hist_idx: Option<usize>,
    pub tool_line: Option<usize>, // index of the active "⠋" tool line
    pub ask_line: Option<usize>,  // index of the pending question's line
    pub retry_line: Option<usize>, // index of the counting "⚠ … retry n/3" line
    /// Transient confirmation shown on the status line. Session bookkeeping
    /// ("resumed …", "exported …") is not part of the conversation, so it does
    /// not belong in the transcript.
    pub notice: Option<(String, std::time::Instant)>,
    pub footer: crate::config::Footer, // which status-line segments to draw
    pub exit_armed: Option<std::time::Instant>, // first ctrl+c seen; a second within 2s quits
    // per-turn accounting for the "· tok · tps · s" line
    pub turn_t0: std::time::Instant,
    pub turn_tok: u64,
    pub turn_gen_ms: u128,
    pub turn_ctx: u64,  // prompt tokens of the last LLM call = current context size
    pub sess_tok: u64,  // tokens generated across the whole session
    pub branch: String, // current git branch, refreshed after every job
    pub turn_est: bool, // tokens were estimated, not reported by the server
    pub thinking: bool, // currently accumulating a "  │ " reasoning block
    // slash-command menu
    pub menu_idx: usize,
    pub menu_top: usize,          // first visible row
    pub menu_for: String,         // input the selection belongs to
    pub menu_off: Option<String>, // input Esc dismissed the menu for
    pub fresh: bool, // nothing submitted yet this run — show the quit hint
    pub pick: Option<Pick>, // open session picker; owns the keyboard while set
}

/// How long a first Ctrl+C stays armed for the second one.
const ARM_WINDOW: Duration = Duration::from_secs(2);
/// How long a transient confirmation stays on the status line.
const NOTICE_TTL: Duration = Duration::from_secs(3);

impl App {
    /// True while a second Ctrl+C would exit.
    pub fn armed(&self) -> bool {
        self.exit_armed.map(|t| t.elapsed() < ARM_WINDOW).unwrap_or(false)
    }

    /// The current notice, if it hasn't aged out. Nothing clears it — the
    /// 50ms render loop simply stops showing it.
    pub fn notice(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, at)| at.elapsed() < NOTICE_TTL)
            .map(|(t, _)| t.as_str())
    }

    /// The per-turn stats row: tokens, tokens/sec over generation time only
    /// (tool waits don't dilute it), and wall clock for the whole turn.
    pub fn turn_stats(&self) -> Option<String> {
        stats_row(self.turn_tok, self.turn_ctx, self.turn_est, self.turn_gen_ms, self.turn_t0.elapsed().as_secs_f64())
    }

    /// Reset the selection when the filter text changed, then keep the
    /// selected row inside the visible window.
    pub fn sync_menu(&mut self) {
        if self.menu_for != self.input {
            self.menu_for = self.input.clone();
            self.menu_idx = 0;
            self.menu_top = 0;
        }
        let n = menu_items(self).len();
        if n == 0 {
            return;
        }
        self.menu_idx = self.menu_idx.min(n - 1);
        if self.menu_idx < self.menu_top {
            self.menu_top = self.menu_idx;
        }
        if self.menu_idx >= self.menu_top + MENU_ROWS {
            self.menu_top = self.menu_idx - MENU_ROWS + 1;
        }
    }

    pub fn flush(&mut self) {
        if !self.current.is_empty() {
            self.lines.push(std::mem::take(&mut self.current));
        }
    }
}

pub fn stats_row(tok: u64, ctx: u64, est: bool, gen_ms: u128, wall_s: f64) -> Option<String> {
    if tok == 0 {
        return None;
    }
    let tps = if gen_ms > 0 { tok as f64 * 1000.0 / gen_ms as f64 } else { 0.0 };
    let e = if est { "~" } else { "" };
    Some(format!("  · {e}{tok} tok · {tps:.1} tps · {wall_s:.1}s · ctx {e}{}", kilo(ctx)))
}

/// 1234 -> "1.2k", 950 -> "950".
pub fn kilo(n: u64) -> String {
    if n < 1000 { n.to_string() } else { format!("{:.1}k", n as f64 / 1000.0) }
}

pub fn ui_loop(
    job_tx: &Sender<Job>,
    rx: Receiver<ai_core::Event>,
    model: String,
    session: String,
    cancel: &AtomicBool,
    seed_lines: Vec<String>,
    seed_history: Vec<String>,
    seed_msg_num: usize,
) -> io::Result<Exit> {
    let cfg = crate::config::Config::load();
    let mut app = App {
        lines: seed_lines, current: String::new(),
        ask: None, input: String::new(), cursor: 0, done: true, model, session,
        msg_num: seed_msg_num, spinner: 0, scroll_up: 0,
        history: seed_history, hist_idx: None, tool_line: None, ask_line: None, retry_line: None,
        notice: None, exit_armed: None,
        footer: cfg.footer.clone(),
        turn_t0: std::time::Instant::now(), turn_tok: 0, turn_ctx: 0, turn_gen_ms: 0, turn_est: false,
        sess_tok: 0, branch: String::new(),
        thinking: false,
        menu_idx: 0, menu_top: 0, menu_for: String::new(), menu_off: None, fresh: true,
        pick: None,
    };
    // stderr is invisible under the alternate screen, so this goes in the
    // transcript — and stays there, a warning you must act on can't expire
    if let Some(e) = &cfg.err {
        app.lines.push(format!("  ⚠ {e}"));
        app.lines.push("  ⚠ no saved models, permissions or MCP servers loaded; saving is off".into());
    }
    let mut state = RenderState::new();

    loop {
        app.sync_menu();
        render::draw(&app, &mut state)?;
        app.spinner = app.spinner.wrapping_add(1);

        if event::poll(Duration::from_millis(50))? {
            if let CEvent::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    // any key other than a second ctrl+c disarms the exit prompt
                    if !matches!((k.code, k.modifiers), (KeyCode::Char('c'), KeyModifiers::CONTROL)) {
                        app.exit_armed = None;
                    }
                    match (k.code, k.modifiers) {
                        // ── session picker owns the keyboard while open ──
                        // Ctrl+key falls through, so ctrl+c/ctrl+d still work while it's open
                        (code, m) if app.pick.is_some() && !m.contains(KeyModifiers::CONTROL) => {
                            picker_key(&mut app, code, job_tx)
                        }
                        // ── slash-command menu (open while input is a lone "/word") ──
                        (KeyCode::Up, _) if !menu_items(&app).is_empty() => {
                            app.menu_idx = app.menu_idx.saturating_sub(1);
                        }
                        (KeyCode::Down, _) if !menu_items(&app).is_empty() => {
                            app.menu_idx += 1; // sync_menu clamps to the item count
                        }
                        (KeyCode::Tab, _) if !menu_items(&app).is_empty() => {
                            app.input = format!("{} ", menu_items(&app)[app.menu_idx].name);
                            app.cursor = app.input.chars().count();
                        }
                        (KeyCode::Esc, _) if !menu_items(&app).is_empty() => {
                            app.menu_off = Some(app.input.clone());
                        }
                        (KeyCode::Enter, _) if !menu_items(&app).is_empty() => {
                            let cmd = menu_items(&app)[app.menu_idx].name.to_string();
                            app.fresh = false;
                            app.input.clear();
                            app.cursor = 0;
                            if handle_command(&cmd, &mut app, job_tx) {
                                return Ok(Exit::Quit);
                            }
                        }
                        // Esc interrupts the running turn (pi: cancel, never quit)
                        (KeyCode::Esc, _) => {
                            if !app.done {
                                cancel.store(true, Ordering::Relaxed);
                                // dismiss a pending question so the agent unblocks
                                if let Some(reply) = close_ask(&mut app, "interrupted") {
                                    let _ = reply.send("interrupted".into());
                                }
                            }
                        }
                        // Ctrl+C: first press clears the input and cancels a running
                        // turn, a second within ARM_WINDOW quits (pi + prototype).
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            if app.armed() {
                                if !app.done {
                                    cancel.store(true, Ordering::Relaxed);
                                }
                                return Ok(Exit::Quit);
                            }
                            app.input.clear();
                            app.cursor = 0;
                            app.pick = None; // first press clears whatever is in the way
                            if !app.done {
                                cancel.store(true, Ordering::Relaxed);
                                if let Some(reply) = close_ask(&mut app, "interrupted") {
                                    let _ = reply.send("interrupted".into());
                                }
                            }
                            app.exit_armed = Some(std::time::Instant::now());
                        }
                        // Ctrl+D exits, only when input is empty (pi: exit when editor empty)
                        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            if app.input.is_empty() {
                                if !app.done {
                                    cancel.store(true, Ordering::Relaxed); // don't wait for the turn
                                }
                                return Ok(Exit::Quit);
                            }
                        }

                        // Submit answer to Ask
                        (KeyCode::Enter, _) if app.ask.is_some() => {
                            app.fresh = false;
                            let ans = app.input.trim().to_string();
                            app.input.clear();
                            app.cursor = 0;
                            if let Some(reply) = close_ask(&mut app, &ans) {
                                let _ = reply.send(ans);
                            }
                        }
                        // Shift+Enter (Alt+Enter where the terminal eats Shift):
                        // a newline in the input instead of submitting it
                        (KeyCode::Enter, m) if m.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                            let byte = app.input.char_indices().nth(app.cursor).map(|(i, _)| i).unwrap_or(app.input.len());
                            app.input.insert(byte, '\n');
                            app.cursor += 1;
                        }

                        // Enter: slash command or task
                        (KeyCode::Enter, _) if !app.input.trim().is_empty() => {
                            let raw = app.input.trim().to_string();
                            app.fresh = false;
                            app.input.clear();
                            app.cursor = 0;
                            if raw.starts_with('/') {
                                if handle_command(&raw, &mut app, job_tx) {
                                    if !app.done {
                                        cancel.store(true, Ordering::Relaxed);
                                    }
                                    return Ok(Exit::Quit);
                                }
                            } else if let Some(cmd) = raw.strip_prefix('!') {
                                let cmd = cmd.trim().to_string();
                                if cmd.is_empty() {
                                    app.lines.push("  ✗ usage: !<shell command>".into());
                                } else if !app.done {
                                    app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
                                } else {
                                    app.flush();
                                    app.scroll_up = 0;
                                    app.done = false;
                                    if app.history.last().map(|h| h != &raw).unwrap_or(true) {
                                        app.history.push(raw.clone());
                                    }
                                    let _ = job_tx.send(Job::Bash(cmd));
                                }
                            } else {
                                start_task(&mut app, job_tx, raw);
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

                        // multi-line input: the arrows walk its lines instead of
                        // recalling history, which would throw the draft away
                        (KeyCode::Up, _) if app.input.contains('\n') => {
                            app.cursor = move_line(&app.input, app.cursor, -1);
                        }
                        (KeyCode::Down, _) if app.input.contains('\n') => {
                            app.cursor = move_line(&app.input, app.cursor, 1);
                        }

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
                ai_core::Event::ReasoningDelta(t) => {
                    if !app.thinking {
                        app.flush();
                        app.thinking = true;
                        app.current.push_str("  │ ");
                    }
                    // keep the model's own paragraph breaks: a thought per block
                    // reads far better than one run-on wall (word_wrap splits on \n)
                    app.current.push_str(&t);
                }
                ai_core::Event::TextDelta(t) => {
                    if app.thinking {
                        app.flush(); // close the thinking block before the answer
                        app.thinking = false;
                        app.lines.push(String::new()); // breathing room before the answer
                    }
                    app.current.push_str(&t);
                }
                ai_core::Event::Text(t) => { app.flush(); app.lines.push(t); }
                ai_core::Event::ToolStart(t) => {
                    app.flush();
                    app.thinking = false;
                    app.lines.push(format!("  ⠋ {t}"));
                    app.tool_line = Some(app.lines.len() - 1);
                }
                ai_core::Event::ToolEnd { summary, ok, ms, output } => {
                    let line = format!("  {} {summary}  {}", if ok { "✓" } else { "✗" }, ai_core::took(ms));
                    match app.tool_line.take() {
                        Some(i) if i < app.lines.len() && app.lines[i].starts_with("  ⠋ ") => app.lines[i] = line,
                        _ => app.lines.push(line),
                    }
                    app.lines.extend(ai_core::fail_tail(&output)); // "  · " rows render dim like the stats line
                }
                ai_core::Event::SessionName(name) => app.session = name,
                ai_core::Event::Git(b) => app.branch = b,
                ai_core::Event::Notice(t) => app.notice = Some((t, std::time::Instant::now())),
                ai_core::Event::Retry { attempt, of, wait_ms, err } => {
                    let line = format!("  ⚠ {err} — retry {attempt}/{of} in {:.1}s", wait_ms as f64 / 1000.0);
                    // one row that counts up, rather than a new line per attempt
                    match app.retry_line {
                        Some(i) if attempt > 1 && i < app.lines.len() => app.lines[i] = line,
                        _ => {
                            app.flush();
                            app.lines.push(line);
                            app.retry_line = Some(app.lines.len() - 1);
                        }
                    }
                }
                ai_core::Event::Tree(rows) => {
                    // start on the active leaf (marked ◀ by tree::rows), like /model starts on the active profile
                    let idx = rows.iter().rposition(|(l, _)| l.ends_with(" ◀")).unwrap_or(rows.len() - 1);
                    let top = idx.saturating_sub(PICK_ROWS / 2);
                    app.pick = Some(Pick { kind: PickKind::Tree, title: "session tree".into(), rows, idx, top, filter: String::new() });
                }
                ai_core::Event::Prefill(t) => {
                    app.cursor = t.chars().count();
                    app.input = t;
                }
                ai_core::Event::Resumed { lines, history, msg_num } => {
                    app.lines = lines;
                    app.history = history;
                    app.current.clear();
                    app.hist_idx = None;
                    app.msg_num = msg_num;
                    app.scroll_up = 0;
                    app.sess_tok = 0; // a different session's totals aren't ours
                }
                ai_core::Event::Usage { tokens, prompt, est, gen_ms } => {
                    app.turn_tok += tokens;
                    app.sess_tok += tokens;
                    app.turn_ctx = prompt;
                    app.turn_gen_ms += gen_ms;
                    app.turn_est |= est;
                }
                ai_core::Event::Ask { question, reply } => {
                    app.flush();
                    app.lines.push(format!("  ℹ {question}"));
                    app.ask_line = Some(app.lines.len() - 1);
                    app.ask = Some((question, reply));
                }
                ai_core::Event::Reload { exe, args } => return Ok(Exit::Reload { exe, args }),
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
                    app.thinking = false;
                    if let Some(l) = app.turn_stats() {
                        app.lines.push(l);
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
}

/// Slash commands: /use <name> switches model, /model (no arg) lists saved
/// profiles or (with a name) switches like /use, /resume loads session.json,
/// /tree dumps the current session path, /reload rebuilds + relaunches,
/// /quit exits. Returns true when the TUI should exit.
fn handle_command(raw: &str, app: &mut App, job_tx: &Sender<Job>) -> bool {
    let mut parts = raw.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/use" => {
            if arg.is_empty() {
                app.lines.push("  ✗ usage: /use <name> (see --list)".into());
            } else {
                switch_model(app, job_tx, arg);
            }
        }
        "/model" => {
            if arg.is_empty() {
                pick_model(app);
            } else {
                switch_model(app, job_tx, arg);
            }
        }
        "/resume" => resume(app, job_tx, arg),
        "/rename" => {
            if arg.is_empty() {
                app.lines.push("  ✗ usage: /rename <new-name>".into());
            } else {
                let _ = job_tx.send(Job::Rename(arg.to_string()));
            }
        }
        "/settings" => pick_settings(app),
        "/mcp" => pick_mcp(app),
        "/tree" => { let _ = job_tx.send(Job::Tree); }
        "/export" => {
            let to = (!arg.is_empty()).then(|| arg.to_string());
            let _ = job_tx.send(Job::Export(to));
        }
        "/plan" => {
            let on = !ai_core::plan_mode();
            ai_core::set_plan(on);
            // the footer carries the state; this is just the confirmation
            app.notice = Some((
                match on {
                    true => "plan mode on — reads and searches only".to_string(),
                    false => "plan mode off".to_string(),
                },
                std::time::Instant::now(),
            ));
        }
        "/commit" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                start_task(app, job_tx, COMMIT_TASK.into());
            }
        }
        "/undo" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                let _ = job_tx.send(Job::Undo);
            }
        }
        "/reload" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                app.done = false; // keeps the spinner/status line showing "working…" during the build
                let _ = job_tx.send(Job::Reload);
            }
        }
        "/quit" => return true,
        _ if CMDS.iter().any(|c| c.name == cmd && c.soon) => {
            app.lines.push(format!("  ℹ {cmd} is queued, not built yet — see harness/open-work.md"))
        }
        _ => app.lines.push(format!("  ✗ unknown command: {cmd}")),
    }
    false
}

/// Close a pending question, recording the answer on the question's own line.
/// An answer to a permission prompt is not a turn in the conversation, so it
/// must not take a message number of its own.
fn close_ask(app: &mut App, ans: &str) -> Option<tokio::sync::oneshot::Sender<String>> {
    let (question, reply) = app.ask.take()?;
    if let Some(i) = app.ask_line.take() {
        if i < app.lines.len() {
            app.lines[i] = format!("  ℹ {question} → {ans}");
        }
    }
    Some(reply)
}

/// The cursor one line up (-1) or down (+1) in a multi-line input, keeping the
/// column where the shorter line allows it.
pub fn move_line(input: &str, cursor: usize, delta: isize) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let cursor = cursor.min(chars.len());
    let line_start = |mut i: usize| {
        while i > 0 && chars[i - 1] != '\n' {
            i -= 1;
        }
        i
    };
    let start = line_start(cursor);
    let col = cursor - start;
    if delta < 0 {
        if start == 0 {
            return cursor; // already on the first line
        }
        let prev = line_start(start - 1);
        prev + col.min(start - 1 - prev)
    } else {
        let mut end = cursor;
        while end < chars.len() && chars[end] != '\n' {
            end += 1;
        }
        if end >= chars.len() {
            return cursor; // already on the last line
        }
        let next = end + 1;
        let mut next_end = next;
        while next_end < chars.len() && chars[next_end] != '\n' {
            next_end += 1;
        }
        next + col.min(next_end - next)
    }
}

/// Show the message, reset the per-turn counters, hand it to the agent thread.
fn start_task(app: &mut App, job_tx: &Sender<Job>, raw: String) {
    app.flush();
    app.msg_num += 1;
    app.lines.push(format!("{}› {raw}", app.msg_num));
    app.current.clear();
    app.scroll_up = 0;
    app.done = false;
    app.turn_t0 = std::time::Instant::now();
    app.turn_tok = 0;
    app.turn_ctx = 0;
    app.turn_gen_ms = 0;
    app.turn_est = false;
    if app.history.last().map(|h| h != &raw).unwrap_or(true) {
        app.history.push(raw.clone());
    }
    let _ = job_tx.send(Job::Task(raw));
}

/// The status line: what this session is, and how full it is. Every segment
/// except `plan` is toggleable via /settings — plan is a mode you are IN, not
/// decoration, so hiding it would hide the reason writes are being refused.
pub fn footer_right(
    f: &crate::config::Footer, plan: bool, session: &str, model: &str, branch: &str,
    sess_tok: u64, ctx: u64, limit: u64,
) -> String {
    let mut parts = Vec::new();
    if plan {
        parts.push("plan".to_string()); // first, so a narrow terminal truncates it last
    }
    if f.session && !session.is_empty() {
        parts.push(session.to_string());
    }
    if f.model {
        parts.push(model.to_string());
    }
    if f.branch && !branch.is_empty() {
        parts.push(format!("⎇ {branch}"));
    }
    if f.tokens && sess_tok > 0 {
        parts.push(format!("{} tok", kilo(sess_tok)));
    }
    // no `ctx > 0` gate: turn_ctx resets to 0 at the start of every turn, so
    // gating made the reading vanish exactly when you were watching it
    if f.context {
        parts.push(format!("ctx {}%", ctx * 100 / limit.max(1)));
    }
    parts.join(" · ")
}

/// Move a picker selection by delta and keep it inside the visible window.
pub fn picker_nav(idx: usize, top: usize, n: usize, delta: isize) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let idx = if delta < 0 { idx.saturating_sub(1) } else { (idx + 1).min(n - 1) };
    let top = if idx < top {
        idx
    } else if idx >= top + PICK_ROWS {
        idx - PICK_ROWS + 1
    } else {
        top
    };
    (idx, top)
}

/// Keys while the picker is open: arrows move, typing filters, Enter selects,
/// Esc cancels, everything else is swallowed. Ctrl+key never reaches here, so
/// Ctrl+C still quits instead of typing a 'c'.
fn picker_key(app: &mut App, code: KeyCode, job_tx: &Sender<Job>) {
    match code {
        KeyCode::Esc => {
            app.pick = None;
            return;
        }
        KeyCode::Enter => {
            let Some(p) = app.pick.as_ref() else { return };
            let Some((_, value)) = p.visible().get(p.idx).map(|(l, v)| (l.clone(), v.clone())) else {
                return; // filtered down to nothing: Enter has nothing to pick
            };
            // toggle lists, not choosers: flipping one row is not a reason to
            // close, you usually came to flip more than one
            if p.kind == PickKind::Settings {
                toggle_footer(app, &value);
                return;
            }
            if p.kind == PickKind::Mcp {
                toggle_mcp(app, &value);
                return;
            }
            let Some(p) = app.pick.take() else { return };
            match p.kind {
                PickKind::Session => { let _ = job_tx.send(Job::ResumePath(value)); }
                PickKind::Model => switch_model(app, job_tx, &value),
                PickKind::Tree => { let _ = job_tx.send(Job::Select(value)); }
                PickKind::Settings | PickKind::Mcp => {} // returned above; closing is Esc's job
            }
            return;
        }
        KeyCode::Char(c) => {
            if let Some(p) = app.pick.as_mut() {
                p.filter.push(c);
                p.idx = 0; // the old selection may not even be on the list now
                p.top = 0;
            }
            return;
        }
        KeyCode::Backspace => {
            if let Some(p) = app.pick.as_mut() {
                p.filter.pop();
                p.idx = 0;
                p.top = 0;
            }
            return;
        }
        KeyCode::Up | KeyCode::Down => {}
        _ => return,
    }
    let Some(p) = app.pick.as_mut() else { return };
    let delta: isize = if code == KeyCode::Up { -1 } else { 1 };
    let (idx, top) = picker_nav(p.idx, p.top, p.visible().len(), delta);
    p.idx = idx;
    p.top = top;
}

/// /resume: no argument opens the picker, an index or name resumes directly.
fn resume(app: &mut App, job_tx: &Sender<Job>, arg: &str) {
    let saved = crate::session::list();
    if saved.is_empty() {
        app.lines.push("  ✗ no saved sessions (run a task first)".into());
        return;
    }
    if !arg.is_empty() {
        let found = arg
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1 && *n <= saved.len())
            .map(|n| saved[n - 1].path.clone())
            .or_else(|| saved.iter().find(|s| s.name == arg).map(|s| s.path.clone()));
        match found {
            Some(path) => { let _ = job_tx.send(Job::ResumePath(path)); }
            None => app.lines.push(format!("  ✗ no session '{arg}' (/resume to pick)")),
        }
        return;
    }
    app.pick = Some(Pick {
        kind: PickKind::Session,
        title: "sessions".into(),
        rows: saved
            .iter()
            .map(|s| {
                (
                    format!(
                        "{:<15}{:>3} entries  {:<11}{}",
                        s.name,
                        s.entries,
                        crate::session::ago(s.age_s),
                        s.head
                    ),
                    s.path.clone(),
                )
            })
            .collect(),
        idx: 0,
        top: 0,
        filter: String::new(),
    });
}

/// Switch to a saved model.json profile by name (shared by /use and /model).
fn switch_model(app: &mut App, job_tx: &Sender<Job>, name: &str) {
    let cfg = crate::config::Config::load();
    match cfg.models.iter().find(|m| m.name == name) {
        Some(p) => {
            app.model = p.model.clone();
            let _ = job_tx.send(Job::Model {
                url: p.url.clone(),
                key: p.key.clone(),
                model: p.model.clone(),
            });
        }
        None => app.lines.push(format!("  ✗ no saved model named '{name}' (see --list)")),
    }
}

/// /model (no argument): open an interactive picker over the saved profiles,
/// marking the active one. Up/down navigate, Enter switches, Esc cancels.
/// /settings: the status-line segments as an on/off list. Rebuilt in place
/// after every toggle so the marks are the live state, keeping the cursor and
/// the typed filter where they were — you are usually flipping a second row.
fn pick_settings(app: &mut App) {
    let f = app.footer.clone();
    // no on-marker here: the picker draws ▸ for the cursor, and a second ▸ for
    // "enabled" just reads as two cursors. The on/off column already says it.
    let row = |key: &str, on: bool, what: &str| {
        (format!("{:<9} {:<3}  {what}", key, if on { "on" } else { "off" }), key.to_string())
    };
    let rows = vec![
        row("session", f.session, "session name"),
        row("model", f.model, "model id"),
        row("branch", f.branch, "git branch"),
        row("tokens", f.tokens, "tokens generated this session"),
        row("context", f.context, "how full the context window is"),
    ];
    let (idx, top, filter) = match app.pick.take() {
        Some(p) if p.kind == PickKind::Settings => (p.idx, p.top, p.filter),
        _ => (0, 0, String::new()),
    };
    let mut p = Pick { kind: PickKind::Settings, title: "status line".into(), rows, idx, top, filter };
    // the labels carry "on"/"off", so a filter can match fewer rows after a
    // toggle than before it — clamp rather than point past the end
    p.idx = p.idx.min(p.visible().len().saturating_sub(1));
    app.pick = Some(p);
}

/// /mcp: configured servers with their live state. "on" means enabled in
/// model.json; the tool count is what actually connected, so an enabled server
/// showing no count is one that failed to start.
fn pick_mcp(app: &mut App) {
    let rows: Vec<(String, String)> = crate::ai_core::mcp::status()
        .into_iter()
        .map(|(name, enabled, connected, n)| {
            let state = match (enabled, connected) {
                (false, _) => "off".to_string(),
                (true, true) => format!("on   {n} tools"),
                (true, false) => "on   not connected".to_string(),
            };
            (format!("{name:<16} {state}"), name)
        })
        .collect();
    let (idx, top, filter) = match app.pick.take() {
        Some(p) if p.kind == PickKind::Mcp => (p.idx, p.top, p.filter),
        _ => (0, 0, String::new()),
    };
    let mut p = Pick { kind: PickKind::Mcp, title: "mcp servers".into(), rows, idx, top, filter };
    p.idx = p.idx.min(p.visible().len().saturating_sub(1));
    app.pick = Some(p);
}

/// Enter on an /mcp row. Turning a server ON connects it here and now, which
/// can take seconds and can fail, so the outcome goes to the status line
/// either way rather than being silently swallowed.
fn toggle_mcp(app: &mut App, name: &str) {
    let on = crate::ai_core::mcp::status()
        .into_iter()
        .find(|(n, ..)| n == name)
        .map(|(_, enabled, ..)| enabled)
        .unwrap_or(false);
    let msg = match crate::ai_core::mcp::set_enabled(name, !on) {
        Ok(m) => m,
        Err(e) => format!("mcp {name}: {e}"),
    };
    app.notice = Some((msg, std::time::Instant::now()));
    pick_mcp(app);
}

/// Flip one segment and write it back to model.json. Load-then-save keeps the
/// rest of the file (models, allow, defaults) intact.
fn toggle_footer(app: &mut App, key: &str) {
    let f = &mut app.footer;
    match key {
        "session" => f.session = !f.session,
        "model" => f.model = !f.model,
        "branch" => f.branch = !f.branch,
        "tokens" => f.tokens = !f.tokens,
        "context" => f.context = !f.context,
        _ => return,
    }
    let mut cfg = crate::config::Config::load();
    cfg.footer = app.footer.clone();
    if let Err(e) = cfg.save() {
        app.notice = Some((format!("settings not saved: {e}"), std::time::Instant::now()));
    }
    pick_settings(app);
}

fn pick_model(app: &mut App) {
    let cfg = crate::config::Config::load();
    if cfg.models.is_empty() {
        app.lines.push("  ✗ no saved models (see model.json)".into());
        return;
    }
    let active = app.model.clone();
    let rows: Vec<(String, String)> = cfg
        .models
        .iter()
        .map(|m| {
            let mark = if m.model == active { "▸" } else { " " };
            (format!("{mark} {:<26} {}", m.name, m.model), m.name.clone())
        })
        .collect();
    // start on the active row, not the top — it's the one you're looking for
    let idx = cfg.models.iter().position(|m| m.model == active).unwrap_or(0);
    let top = idx.saturating_sub(PICK_ROWS / 2);
    app.pick = Some(Pick {
        kind: PickKind::Model,
        title: "models".into(),
        rows,
        idx,
        top,
        filter: String::new(),
    });
}
