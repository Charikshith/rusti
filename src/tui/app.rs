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
    Cmd { name: "/themes", desc: "pick the colour palette, or /themes <name>", soon: false },
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
    Theme,    // switch the palette; stays open, so you can walk the list and watch
    Ask,      // answer a pending question; Enter sends the row's value back
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
    pub hand: bool,               // the running tool waited on the user's permission
    pub fold: Option<(usize, Vec<String>)>, // the row a run of reads folds into, and its files
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
    /// Ctrl+O: show the tail of successful tool output too. Failures are
    /// always shown — a message you must read can't sit behind a keystroke.
    pub expand: bool,
}

/// Prefix on a transcript row that is present but not drawn until Ctrl+O.
/// A marker char beats a parallel Vec<bool>: app.lines is pushed to from a
/// dozen places, and two vectors that must stay in step is the bug.
pub const HIDDEN: &str = "\u{1}";

/// How long a first Ctrl+C stays armed for the second one.
const ARM_WINDOW: Duration = Duration::from_secs(2);
/// How long a transient confirmation stays on the status line.
const NOTICE_TTL: Duration = Duration::from_secs(3);

impl App {
    pub fn new(model: String, session: String, lines: Vec<String>, history: Vec<String>, msg_num: usize,
               footer: crate::config::Footer) -> App {
        App {
            lines, current: String::new(),
            ask: None, input: String::new(), cursor: 0, done: true, model, session,
            msg_num, spinner: 0, scroll_up: 0,
            history, hist_idx: None, tool_line: None, ask_line: None, hand: false, fold: None, retry_line: None,
            notice: None, exit_armed: None,
            footer,
            turn_t0: std::time::Instant::now(), turn_tok: 0, turn_ctx: 0, turn_gen_ms: 0, turn_est: false,
            sess_tok: 0, branch: String::new(),
            thinking: false,
            menu_idx: 0, menu_top: 0, menu_for: String::new(), menu_off: None, fresh: true,
            pick: None, expand: false,
        }
    }
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

/// A turn's wall time, read at a glance: seconds keep a decimal because a
/// short turn's tenths are the interesting part, minutes drop it because
/// "4m 32.4s" is noise once you are counting minutes.
pub fn human_dur(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 60.0 {
        return format!("{s:.1}s");
    }
    let (m, rest) = ((s / 60.0) as u64, (s % 60.0) as u64);
    if m < 60 {
        format!("{m}m {rest:02}s")
    } else {
        format!("{}h {:02}m", m / 60, m % 60)
    }
}

/// Local wall-clock "11:03 PM" from a unix timestamp and an offset in minutes.
/// Pure arithmetic on the time of day: no date, so no month lengths, no leap
/// years, nothing to get wrong beyond the offset itself.
pub fn clock(epoch_secs: u64, offset_min: i64) -> String {
    let local = epoch_secs as i64 + offset_min * 60;
    let day = local.rem_euclid(86_400);
    let (h24, min) = ((day / 3600) as u32, (day % 3600 / 60) as u32);
    let ampm = if h24 < 12 { "AM" } else { "PM" };
    let h12 = match h24 % 12 { 0 => 12, h => h };
    format!("{h12}:{min:02} {ampm}")
}

/// Hours and minutes out of a clock line. The LAST non-empty line, not the
/// whole output: a machine with a cmd AutoRun script prints a banner first, and
/// parsing that would silently answer "UTC" on a box that is nowhere near it.
pub fn parse_hm(text: &str) -> Option<(i64, i64)> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let mut parts = line.trim().split(':');
    let h = parts.next()?.trim().parse::<i64>().ok()?;
    let m = parts.next()?;
    let m = m.get(..2)?.parse::<i64>().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// Minutes east of UTC, asked once per process. std has no local time and this
/// is not worth a dependency: one cheap `echo %TIME%` / `date +%H:%M`, diffed
/// against the same instant in UTC, gives the offset for the whole session.
/// ponytail: a session running across a DST change keeps the old offset —
/// re-read it per turn if anyone ever notices.
pub fn utc_offset_min() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let out = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "echo %TIME%"]).output()
        } else {
            std::process::Command::new("date").args(["+%H:%M"]).output()
        };
        let Ok(out) = out else { return 0 };
        let text = String::from_utf8_lossy(&out.stdout);
        let Some((h, m)) = parse_hm(&text) else { return 0 };
        let utc = (crate::session::epoch_secs() % 86_400) as i64;
        let diff = (h * 60 + m) - utc / 60;
        // wrap to (-12h, +14h] and snap to a quarter hour: the two clocks are
        // read a few ms apart, so the raw difference is off by a minute at most
        let wrapped = (diff + 720).rem_euclid(1440) - 720;
        (wrapped as f64 / 15.0).round() as i64 * 15
    })
}

pub fn stats_row(tok: u64, ctx: u64, est: bool, gen_ms: u128, wall_s: f64) -> Option<String> {
    if tok == 0 {
        return None;
    }
    let tps = if gen_ms > 0 { tok as f64 * 1000.0 / gen_ms as f64 } else { 0.0 };
    let e = if est { "~" } else { "" };
    let done = clock(crate::session::epoch_secs(), utc_offset_min());
    Some(format!(
        "  · worked {} · done {done} · {e}{tok} tok · {tps:.1} tps · ctx {e}{}",
        human_dur(wall_s), kilo(ctx)))
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
    // a saved palette that is no longer in the table keeps the default and says
    // so below, rather than leaving the user wondering why /themes did nothing
    let bad_theme = cfg.theme.as_deref().filter(|n| !super::theme::set(n)).map(String::from);
    super::theme::set_light(cfg.light);
    let mut app = App::new(model, session, seed_lines, seed_history, seed_msg_num, cfg.footer.clone());
    // stderr is invisible under the alternate screen, so this goes in the
    // transcript — and stays there, a warning you must act on can't expire
    if let Some(e) = &cfg.err {
        app.lines.push(format!("  ⚠ {e}"));
        app.lines.push("  ⚠ no saved models, permissions or MCP servers loaded; saving is off".into());
    }
    if let Some(name) = bad_theme {
        app.lines.push(format!("  ⚠ no theme called {name}; using {}", super::theme::name()));
    }
    let mut state = RenderState::new();

    loop {
        app.sync_menu();
        render::draw(&app, &mut state)?;
        app.spinner = app.spinner.wrapping_add(1);

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            // The wheel only arrives as a Mouse event because the TUI captures
            // the mouse. Without capture the terminal turns it into Up/Down key
            // presses in the alternate screen, which this app reads as input
            // history — that is the "scrolling jumps to an old message" bug.
            if let CEvent::Mouse(m) = ev {
                match m.kind {
                    event::MouseEventKind::ScrollUp => {
                        app.scroll_up = (app.scroll_up + 3).min(state.max_scroll);
                    }
                    event::MouseEventKind::ScrollDown => {
                        app.scroll_up = app.scroll_up.saturating_sub(3);
                    }
                    _ => {}
                }
                continue;
            }
            if let CEvent::Key(k) = ev {
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
                        // Ctrl+V / Alt+V paste the clipboard's image: it lands in
                        // .rusti/clips and its PATH is typed into the input, which
                        // read_file then attaches (feat-061). Both chords, because
                        // Windows Terminal binds ctrl+v to its own text paste and
                        // usually swallows it — alt+v is the one that always arrives.
                        (KeyCode::Char('v'), m)
                            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
                        {
                            match ai_core::tools::clipboard_image() {
                                Ok(path) => {
                                    let text = format!("{path} ");
                                    let byte = app.input.char_indices().nth(app.cursor)
                                        .map(|(i, _)| i).unwrap_or(app.input.len());
                                    app.input.insert_str(byte, &text);
                                    app.cursor += text.chars().count();
                                    app.notice = Some((format!("pasted {path}"), std::time::Instant::now()));
                                }
                                // a keypress that does nothing reads as a broken key,
                                // so say why on the status line either way
                                Err(e) => app.notice = Some((e, std::time::Instant::now())),
                            }
                        }
                        // Ctrl+O reveals the output of tools that SUCCEEDED; failures
                        // are on screen already. The rows sit in app.lines the whole
                        // time, marked hidden, so this is a redraw and not a rebuild.
                        (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                            app.expand = !app.expand;
                            let what = if app.expand { "shown" } else { "hidden" };
                            app.notice = Some((format!("tool output {what}"), std::time::Instant::now()));
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
                        // clamped to what the last frame could actually show:
                        // an unbounded scroll_up would sit far past the top and
                        // eat every scroll back down until it unwound
                        (KeyCode::PageUp, _) => { app.scroll_up = (app.scroll_up + 10).min(state.max_scroll); }
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
                    narrate(&mut app);
                    app.flush();
                    app.thinking = false;
                    app.hand = false;
                    app.lines.push(format!("  ⠋ {t}"));
                    app.tool_line = Some(app.lines.len() - 1);
                }
                ai_core::Event::ToolEnd { summary, ok, ms, output } => tool_end(&mut app, summary, ok, ms, &output),
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
                ai_core::Event::Ask { question, choices, reply } => {
                    app.flush();
                    // A permission prompt belongs to the tool row it gates: that row
                    // turns into the question and back, leaving nothing behind. A
                    // question the model asks (no fixed choices) keeps its own row.
                    let tool = app.tool_line.filter(|&i| !choices.is_empty() && i < app.lines.len());
                    match tool.and_then(|i| app.lines[i].strip_prefix("  ⠋ ").map(|t| (i, t.to_string()))) {
                        Some((i, t)) => {
                            app.lines[i] = format!("  ? {t}  allow?");
                            app.ask_line = Some(i);
                            app.hand = true;
                        }
                        None => {
                            app.lines.push(format!("  ℹ {question}"));
                            app.ask_line = Some(app.lines.len() - 1);
                        }
                    }
                    app.ask = Some((question.clone(), reply));
                    // a fixed set of answers is a choice, not a sentence to type:
                    // it opens the picker that /model and /resume already use
                    if !choices.is_empty() {
                        app.pick = Some(Pick {
                            kind: PickKind::Ask,
                            title: question,
                            rows: choices,
                            idx: 0,
                            top: 0,
                            filter: String::new(),
                        });
                    }
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
                    app.fold = None;
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
        // /themes <name> switches outright; bare /themes opens the list
        "/themes" => {
            if arg.is_empty() {
                pick_themes(app)
            } else {
                apply_theme(app, arg);
                app.pick = None; // named outright: no list to leave open
            }
        }
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

/// Answer the pending question from the picker: the panel closes, the question's
/// transcript line records what was chosen, and the tool waiting on the reply is
/// unblocked with the same string a typed answer would have sent.
fn answer_pick(app: &mut App, ans: &str) {
    app.pick = None;
    app.fresh = false;
    if let Some(reply) = close_ask(app, ans) {
        let _ = reply.send(ans.to_string());
    }
}

/// Close a pending question, recording the answer on the question's own line.
/// An answer to a permission prompt is not a turn in the conversation, so it
/// must not take a message number of its own.
fn close_ask(app: &mut App, ans: &str) -> Option<tokio::sync::oneshot::Sender<String>> {
    let (question, reply) = app.ask.take()?;
    if let Some(i) = app.ask_line.take() {
        if i < app.lines.len() {
            // a tool's own permission row goes back to running; ToolEnd settles it
            app.lines[i] = match app.lines[i].strip_prefix("  ? ") {
                Some(t) => format!("  ⠋ {}", t.strip_suffix("  allow?").unwrap_or(t)),
                None => format!("  ℹ {question} → {ans}"),
            };
        }
    }
    Some(reply)
}

/// Prose streamed before a tool call is the model narrating what it is about
/// to do, not its answer: it is set dim under a rail, so the answer is the only
/// prose at full brightness. Backticks are dropped, since narration is not run
/// through markdown.
fn narrate(app: &mut App) {
    if app.thinking || app.current.trim().is_empty() {
        return;
    }
    let text = std::mem::take(&mut app.current).replace('`', "");
    let rows: Vec<String> = text.trim().lines().map(|l| format!("  ┆ {l}")).collect();
    app.lines.push(rows.join("\n"));
}

/// Settle a tool's row: ✓ or ✗ with the reason, time only when it is worth
/// reading, ✋ when the user approved it. Output stays behind Ctrl+O. A run of
/// successful reads folds into one row, since each on its own says little.
pub fn tool_end(app: &mut App, summary: String, ok: bool, ms: u128, output: &str) {
    let time = if ms >= 100 { format!("  {}", ai_core::took(ms)) } else { String::new() };
    let hand = if std::mem::take(&mut app.hand) { "  ✋" } else { "" };
    let tail: Vec<String> = ai_core::fail_tail(output).into_iter().map(|r| format!("{HIDDEN}{r}")).collect();
    let line = if ok {
        format!("  ✓ {summary}{time}{hand}")
    } else {
        let why = ai_core::fail_reason(output);
        let more = if tail.len() > 1 { "  (ctrl+o)" } else { "" };
        format!("  ✗ {summary}  {why}{time}{hand}{more}")
    };
    let row = app.tool_line.take().filter(|&i| i < app.lines.len() && app.lines[i].starts_with("  ⠋ "));
    let read = summary.strip_prefix("read ").filter(|_| ok).map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string());
    let fold = app.fold.take();
    match (row, read) {
        // this read follows the last one with nothing visible between them
        (Some(i), Some(file)) if fold.as_ref().is_some_and(|(j, _)| *j < i && app.lines[j + 1..i].iter().all(|l| l.starts_with(HIDDEN))) => {
            let (j, mut files) = fold.unwrap();
            files.push(file);
            app.lines[j] = format!("  ✓ read {} files  {}", files.len(), files.join(" · "));
            app.lines.remove(i);
            app.fold = Some((j, files));
        }
        (Some(i), read) => {
            app.lines[i] = line;
            app.fold = read.map(|f| (i, vec![f]));
        }
        (None, _) => app.lines.push(line),
    }
    app.lines.extend(tail);
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
    // An answer picker is a chooser, not a list to filter: Esc means no rather
    // than "close the panel" (a pending question cannot just be dismissed —
    // something is blocked waiting on it), a digit takes its row outright, and
    // typed letters must not filter, or "y" would hide the answer you meant.
    if app.pick.as_ref().is_some_and(|p| p.kind == PickKind::Ask) {
        let row_value = |app: &App, i: usize| app.pick.as_ref().and_then(|p| p.rows.get(i)).map(|(_, v)| v.clone());
        match code {
            KeyCode::Esc => return answer_pick(app, "no"),
            KeyCode::Enter => {
                let i = app.pick.as_ref().map(|p| p.idx).unwrap_or(0);
                if let Some(v) = row_value(app, i) {
                    answer_pick(app, &v);
                }
                return;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Some(v) = c.to_digit(10).and_then(|n| (n as usize).checked_sub(1)).and_then(|i| row_value(app, i)) {
                    answer_pick(app, &v);
                }
                return;
            }
            KeyCode::Char(_) | KeyCode::Backspace => return,
            _ => {}
        }
    }
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
            // the whole screen is the preview, so the list stays open: Enter
            // applies, Esc closes, and walking the rows is how you compare
            if p.kind == PickKind::Theme {
                apply_theme(app, &value);
                return;
            }
            let Some(p) = app.pick.take() else { return };
            match p.kind {
                PickKind::Session => { let _ = job_tx.send(Job::ResumePath(value)); }
                PickKind::Model => switch_model(app, job_tx, &value),
                PickKind::Tree => { let _ = job_tx.send(Job::Select(value)); }
                PickKind::Settings | PickKind::Mcp | PickKind::Theme => {} // returned above; closing is Esc's job
                PickKind::Ask => {} // handled at the top: an answer is not a list action
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
                        "{:<15}{:<9}{:<13}{:<11}{}",
                        s.name,
                        crate::session::count(s.msgs, "msg", "msgs"),
                        // resuming shows one branch; say when there are others
                        if s.branches > 1 { crate::session::count(s.branches, "branch", "branches") } else { String::new() },
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
/// /themes with no argument. The rows carry the note from the table, so the
/// list explains itself rather than making you try all fifteen.
fn pick_themes(app: &mut App) {
    let here = super::theme::name();
    let rows: Vec<(String, String)> = super::theme::THEMES
        .iter()
        .map(|t| {
            let mark = if t.name == here { "•" } else { " " };
            (format!("{mark} {:<14} {}", t.name, t.note), t.name.to_string())
        })
        .collect();
    let (idx, top, filter) = match app.pick.take() {
        Some(p) if p.kind == PickKind::Theme => (p.idx, p.top, p.filter),
        // open on the row you are using, not on row 0
        _ => {
            let i = super::theme::index_of(here).unwrap_or(0);
            (i, i.saturating_sub(PICK_ROWS - 1), String::new())
        }
    };
    let mut p = Pick { kind: PickKind::Theme, title: "palette".into(), rows, idx, top, filter };
    p.idx = p.idx.min(p.visible().len().saturating_sub(1));
    app.pick = Some(p);
}

/// Switch and save. The redraw happens on the next frame anyway, so there is
/// nothing to repaint here — the whole screen is already the preview.
fn apply_theme(app: &mut App, name: &str) {
    if !super::theme::set(name) {
        app.notice = Some((format!("no theme called {name}"), std::time::Instant::now()));
        return;
    }
    let mut cfg = crate::config::Config::load();
    cfg.theme = Some(name.to_string());
    if let Err(e) = cfg.save() {
        app.notice = Some((format!("theme not saved: {e}"), std::time::Instant::now()));
    }
    pick_themes(app); // redraw the list so the • moves to the row you just picked
}

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
        app.lines.push("  ✗ no saved models (rusti --add)".into());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("m".into(), "s".into(), vec![], vec![], 0, Default::default())
    }
    fn start(app: &mut App, t: &str) {
        app.lines.push(format!("  ⠋ {t}"));
        app.tool_line = Some(app.lines.len() - 1);
    }
    fn shown(app: &App) -> Vec<&str> {
        app.lines.iter().filter(|l| !l.starts_with(HIDDEN)).map(String::as_str).collect()
    }

    /// Design D's tool rows: a run of reads is one row, a failure is one row
    /// carrying its reason with the output behind Ctrl+O, a permission prompt
    /// is the tool's own row and leaves nothing behind, and the time shown is
    /// the command's, not the user's thinking time.
    #[test]
    fn tool_rows_fold_reads_and_carry_their_own_outcome() {
        let mut a = app();
        start(&mut a, "read harness/feature_list.json");
        tool_end(&mut a, "read harness/feature_list.json".into(), true, 13, "{...}");
        start(&mut a, "read harness/progress.md");
        tool_end(&mut a, "read harness/progress.md".into(), true, 11, "# Progress");
        start(&mut a, "read AGENTS.md");
        tool_end(&mut a, "read AGENTS.md".into(), true, 9, "x");
        assert_eq!(shown(&a), vec!["  ✓ read 3 files  feature_list.json · progress.md · AGENTS.md"]);
        assert_eq!(a.lines.iter().filter(|l| l.starts_with(HIDDEN)).count(), 3, "each read's output stays behind Ctrl+O");

        // a visible row between two reads ends the fold
        a.lines.push("  ┆ now the log".into());
        start(&mut a, "read x.md");
        tool_end(&mut a, "read x.md".into(), true, 5, "x");
        assert_eq!(shown(&a).last(), Some(&"  ✓ read x.md"), "under 100ms shows no time");

        // permission: the row becomes the question, then settles in place
        let mut a = app();
        start(&mut a, "run pwd");
        let (tx, _rx) = tokio::sync::oneshot::channel();
        a.lines[0] = "  ? run pwd  allow?".into();
        a.ask_line = Some(0);
        a.ask = Some(("allow run_command run pwd?".into(), tx));
        a.hand = true;
        close_ask(&mut a, "yes");
        assert_eq!(a.lines, vec!["  ⠋ run pwd"], "answering restores the running row, no ℹ line left behind");
        tool_end(&mut a, "run pwd".into(), false, 16,
            "[exit 1]\n'pwd' is not recognized as an internal or external command,\noperable program or batch file.");
        assert_eq!(shown(&a), vec!["  ✗ run pwd  'pwd' is not recognized as an internal or external command,  ✋  (ctrl+o)"]);
        assert!(a.lines.len() > 1, "the full output is kept for Ctrl+O");

        // time only when it is worth reading
        let mut a = app();
        start(&mut a, "run cargo test");
        tool_end(&mut a, "run cargo test".into(), true, 4200, "ok");
        assert_eq!(shown(&a), vec!["  ✓ run cargo test  4.2s"]);
    }

    /// Prose streamed before a tool call is narration: it goes dim under the
    /// rail, and the answer after the last tool stays ordinary prose.
    #[test]
    fn prose_before_a_tool_is_narration() {
        let mut a = app();
        a.current = "Let me try `cd` instead.\nThen read the files.".into();
        narrate(&mut a);
        assert_eq!(a.lines, vec!["  ┆ Let me try cd instead.\n  ┆ Then read the files."]);
        assert!(a.current.is_empty());
        a.thinking = true;
        a.current = "  │ reasoning".into();
        narrate(&mut a);
        assert_eq!(a.lines.len(), 1, "reasoning is not narration; it keeps its own style");
    }
}
