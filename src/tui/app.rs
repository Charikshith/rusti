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
    Cmd { name: "/resume", desc: "pick a saved session and continue it", soon: false },
    Cmd { name: "/rename", desc: "/rename <new-name> - rename the active session", soon: false },
    Cmd { name: "/tree", desc: "browse the session tree and branch from an earlier entry", soon: false },
    Cmd { name: "/reload", desc: "rebuild rusti from source and relaunch", soon: false },
    Cmd { name: "/quit", desc: "exit rusti (same as ctrl+c twice)", soon: false },
    Cmd { name: "/undo", desc: "put back the files the last turn changed and rewind to before it", soon: false },
    Cmd { name: "/commit", desc: "stage the work and commit it with a drafted message", soon: false },
    Cmd { name: "/plan", desc: "toggle plan mode: read and propose, change nothing", soon: false },
    Cmd { name: "/test", desc: "/test <cmd> - loop until it exits 0", soon: true },
    Cmd { name: "/export", desc: "/export [file.md] - write out the transcript", soon: true },
    Cmd { name: "/session", desc: "/session list|switch <name>", soon: true },
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
pub enum PickKind {
    Session, // resume the chosen session file
    Model,   // switch to the chosen model profile
    Tree,    // branch the session at the chosen entry
}

/// An open list picker (/resume with no argument, /model with no argument).
/// Rows are (label, value): a session path or a model profile name.
pub struct Pick {
    pub kind: PickKind,
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub idx: usize,
    pub top: usize, // first visible row (window for long lists)
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

impl App {
    /// True while a second Ctrl+C would exit.
    pub fn armed(&self) -> bool {
        self.exit_armed.map(|t| t.elapsed() < ARM_WINDOW).unwrap_or(false)
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
    let mut app = App {
        lines: seed_lines, current: String::new(),
        ask: None, input: String::new(), cursor: 0, done: true, model, session,
        msg_num: seed_msg_num, spinner: 0, scroll_up: 0,
        history: seed_history, hist_idx: None, tool_line: None, exit_armed: None,
        turn_t0: std::time::Instant::now(), turn_tok: 0, turn_ctx: 0, turn_gen_ms: 0, turn_est: false,
        sess_tok: 0, branch: String::new(),
        thinking: false,
        menu_idx: 0, menu_top: 0, menu_for: String::new(), menu_off: None, fresh: true,
        pick: None,
    };
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
                        (code, _) if app.pick.is_some() => picker_key(&mut app, code, job_tx),
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
                                if let Some((_, reply)) = app.ask.take() {
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
                            if !app.done {
                                cancel.store(true, Ordering::Relaxed);
                                if let Some((_, reply)) = app.ask.take() {
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
                ai_core::Event::Tree(rows) => {
                    // start on the active leaf (marked ◀ by tree::rows), like /model starts on the active profile
                    let idx = rows.iter().rposition(|(l, _)| l.ends_with(" ◀")).unwrap_or(rows.len() - 1);
                    let top = idx.saturating_sub(PICK_ROWS / 2);
                    app.pick = Some(Pick { kind: PickKind::Tree, title: "session tree".into(), rows, idx, top });
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
        "/tree" => { let _ = job_tx.send(Job::Tree); }
        "/plan" => {
            let on = !ai_core::plan_mode();
            ai_core::set_plan(on);
            app.lines.push(match on {
                true => "  ℹ plan mode on — reads and searches only; /plan again to allow changes".into(),
                false => "  ℹ plan mode off".to_string(),
            });
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

/// Right of the status line: what this session is, and how full it is.
pub fn footer_right(plan: bool, session: &str, model: &str, branch: &str, sess_tok: u64, ctx: u64, limit: u64) -> String {
    let mut parts = Vec::new();
    if plan {
        parts.push("plan".to_string()); // first, so a narrow terminal truncates it last
    }
    if !session.is_empty() {
        parts.push(session.to_string());
    }
    parts.push(model.to_string());
    if !branch.is_empty() {
        parts.push(format!("⎇ {branch}"));
    }
    if sess_tok > 0 {
        parts.push(format!("{} tok", kilo(sess_tok)));
    }
    if ctx > 0 {
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

/// Keys while the picker is open: arrows move, Enter selects, Esc cancels,
/// everything else is swallowed.
fn picker_key(app: &mut App, code: KeyCode, job_tx: &Sender<Job>) {
    match code {
        KeyCode::Esc => {
            app.pick = None;
            return;
        }
        KeyCode::Enter => {
            let Some(p) = app.pick.take() else { return };
            match p.kind {
                PickKind::Session => { let _ = job_tx.send(Job::ResumePath(p.rows[p.idx].1.clone())); }
                PickKind::Model => switch_model(app, job_tx, &p.rows[p.idx].1),
                PickKind::Tree => { let _ = job_tx.send(Job::Select(p.rows[p.idx].1.clone())); }
            }
            return;
        }
        KeyCode::Up | KeyCode::Down => {}
        _ => return,
    }
    let Some(p) = app.pick.as_mut() else { return };
    let delta: isize = if code == KeyCode::Up { -1 } else { 1 };
    let (idx, top) = picker_nav(p.idx, p.top, p.rows.len(), delta);
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
    });
}
