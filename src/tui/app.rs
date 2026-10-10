// App state + event loop (pi-style), rendered on the alternate screen.
// Transcript scrolls inside the app (PageUp/PageDown); the shell's primary
// buffer is untouched while the TUI runs.
// Always-visible input field; model name in bottom status line.
// Keyboard: Esc interrupts, Ctrl+C clears (twice quits), Ctrl+D quits when the
// input is empty, Ctrl+O/Ctrl+T show or hide tool output/thinking, Enter submits, arrows edit input, Up/Down recall history,
// PageUp/PageDown scroll transcript, @ opens a file picker, Tab completes a path.
// Left-drag over the transcript selects and copies (double-click a word, triple a line).
// Slash: /use /model /new /resume /rename /tree /reload /quit, plus prompt
// templates, /skill:name and /server:prompt (see commands()).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};

use crate::ai_core;
use crate::resources;
use super::render::{self, RenderState};
use super::Job;

/// One row of the slash-command menu. `soon` marks a command that is queued in
/// harness/open-work.md but not built yet, so the menu stays honest.
#[derive(Clone, Debug, PartialEq)]
pub struct Cmd {
    pub name: String,
    pub desc: String,
    pub soon: bool,
    /// Where it comes from, shown beside it: empty for a built-in, u yours,
    /// p this project's, mcp:<server> a server's prompt.
    pub tag: String,
}

/// The built-in commands: (name, description, soon).
const BUILTINS: &[(&str, &str, bool)] = &[
    ("/model", "pick from saved profiles, or /model <name> to switch", false),
    ("/use", "/use <name> - switch to a saved profile", false),
    ("/resume", "list sessions and switch: /resume, /resume <name>, /resume <n>", false),
    ("/rename", "/rename <new-name> - rename the active session", false),
    ("/new", "/new [name] - save this session and start a new one", false),
    ("/tree", "browse the session tree and branch from an earlier entry", false),
    ("/reload", "rebuild from source (or restart) and relaunch", false),
    ("/quit", "exit rusti (same as ctrl+c twice)", false),
    ("/undo", "put back the files the last turn changed and rewind to before it", false),
    ("/commit", "stage the work and commit it with a drafted message", false),
    ("/plan", "toggle plan mode: read and propose, change nothing (shift+tab cycles all modes)", false),
    ("/settings", "choose which segments the status line shows", false),
    ("/themes", "pick the colour palette, or /themes <name>", false),
    ("/mcp", "list MCP servers and switch them on or off", false),
    ("/test", "/test <cmd> - loop until it exits 0", true),
    ("/export", "/export [file.md] - write the transcript out as markdown", false),
    ("/compact", "/compact [focus] - summarize the older history now to free context", false),
];

pub fn builtins() -> Vec<Cmd> {
    BUILTINS.iter().map(|(n, d, soon)| Cmd { name: n.to_string(), desc: d.to_string(), soon: *soon, tag: String::new() }).collect()
}

/// A built-in owns its name in any case, so /Commit.md never runs as /Commit.
fn is_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(name))
}

/// The / menu (Q16): built-ins, then prompt templates, then skills, then MCP
/// prompts, each tagged by where it came from. A built-in keeps its name
/// against a template, with a warning: a file must not take over /commit.
pub fn commands(warn: &mut Vec<String>) -> Vec<Cmd> {
    let ext = |name: String, hint: &str, desc: &str, tag: String| Cmd {
        name,
        desc: if hint.is_empty() { desc.to_string() } else { format!("{hint} - {desc}") },
        soon: false,
        tag,
    };
    let mut found: Vec<Cmd> = Vec::new();
    found.extend(resources::templates(warn).into_iter().map(|t| ext(format!("/{}", t.name), &t.hint, &t.desc, t.tag.into())));
    found.extend(resources::skills(warn).into_iter().map(|s| ext(format!("/skill:{}", s.name), &s.hint, &s.desc, s.tag.into())));
    found.extend(ai_core::mcp::prompts().into_iter().map(|(srv, p)| ext(format!("/{srv}:{}", p.name), &p.hint(), &p.desc, format!("mcp:{srv}"))));
    let mut v = builtins();
    for c in found {
        if is_builtin(&c.name) {
            warn.push(format!("{} [{}] has a built-in command's name; the built-in wins", c.name, c.tag));
        } else {
            v.push(c);
        }
    }
    v
}

/// Menu rows visible at once; up/down walks the whole filtered list.
pub const MENU_ROWS: usize = 5;

/// /commit is a prompt macro — the model already has git and the tools; the
/// user message already carries `git status --short`.
const COMMIT_TASK: &str = "Review the working tree with git status and git diff, then stage the files \
belonging to the work we just did and create one commit. Write a concise message saying why the change \
was made, not just what changed. Do not push.";

/// Picker rows visible at once; up/down scrolls the window for long lists.
pub const PICK_ROWS: usize = 8;

/// One row of the menu panel: a slash command, or a path to put in place of
/// the chars `start..cursor` of the input (an `@` reference or a Tab completion).
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Cmd(Cmd),
    Path { start: usize, insert: String, path: String, dir: bool },
}

/// Everything menu_items reads besides the pick/ask state, which forces a refresh.
type MenuKey = (String, usize, bool, Option<(String, usize)>, Option<String>);

/// `@` picker rows shown at most, and files indexed at most.
const FILE_ROWS: usize = 20;
const FILE_CAP: usize = 50_000;

/// What the panel offers for this input: the commands while it is a lone
/// "/word" (a space means the arguments have started, so the menu closes), the
/// list a Tab opened, or the files matching an `@` token at the cursor.
pub fn menu_items(app: &App) -> Vec<Item> {
    if app.pick.is_some() || app.ask.is_some() || app.menu_off.as_deref() == Some(app.input.as_str()) {
        return Vec::new();
    }
    let cmds = filter_cmds(&app.cmds, &app.input);
    if !cmds.is_empty() {
        return cmds.into_iter().map(Item::Cmd).collect();
    }
    if let Some((input, cursor, items)) = &app.tab {
        if *input == app.input && *cursor == app.cursor {
            return items.clone();
        }
    }
    match (at_token(&app.input, app.cursor), &app.files) {
        (Some((start, q)), Some(files)) => score_files(files, &q)
            .into_iter()
            .map(|(path, dir)| Item::Path { start, insert: at_insert(&path, dir), path, dir })
            .collect(),
        _ => Vec::new(),
    }
}

/// The `@` token the cursor is in: (char index of the `@`, query typed after
/// it). The `@` must start a token — line start, whitespace, `(`, `[` or a
/// quote before it — so "me@host" stays an email. `@"` opens a quoted path,
/// which may hold spaces; only the cursor's own line is looked at.
pub fn at_token(input: &str, cursor: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = input.chars().take(cursor).collect();
    let line = chars.iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
    for i in (line..chars.len()).rev() {
        if chars[i] != '@' || !(i == line || matches!(chars[i - 1], ' ' | '\t' | '(' | '[' | '"' | '\'' | '`')) {
            continue;
        }
        let rest: String = chars[i + 1..].iter().collect();
        return match rest.strip_prefix('"') {
            Some(q) if !q.contains('"') => Some((i, q.to_string())),
            None if !rest.contains(char::is_whitespace) && !rest.contains('"') => Some((i, rest)),
            _ => None, // the nearest @ is closed or behind a space: not a reference being typed
        };
    }
    None
}

/// The `@` references in a submitted message that name an image file, each
/// once, in order: what gets attached to it (F03). Only `@` counts — a path
/// merely mentioned ("not old.png") stays text — so Alt+V types its clips as
/// `@` references too. Read with at_token's own rules by asking it about each
/// place a token can end. ponytail: O(n²) over the draft's chars, fine for
/// anything typed; scan forward once if pasted megabytes ever submit slowly.
pub fn image_refs(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut refs: Vec<(usize, String)> = Vec::new();
    for end in 0..=chars.len() {
        if end < chars.len() && !chars[end].is_whitespace() && chars[end] != '"' {
            continue;
        }
        if let Some((start, path)) = at_token(input, end) {
            match refs.last_mut() {
                Some(last) if last.0 == start => last.1 = path, // a quoted path grows past its spaces
                _ => refs.push((start, path)),
            }
        }
    }
    let is_image = |p: &str| ai_core::tools::image_mime(p).is_some() && std::path::Path::new(p).is_file();
    let mut out: Vec<String> = Vec::new();
    for (_, p) in refs {
        // "what is in @shot.png?": sentence punctuation after a reference
        let p = if is_image(&p) { p } else { p.trim_end_matches(['?', '.', ',', ';', ':', '!', ')']).to_string() };
        if is_image(&p) && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// What accepting a path inserts for an `@` reference. A folder keeps the
/// picker open (no trailing space) so you can walk into it; a path with a
/// space is quoted, and a quoted folder stays open-quoted for the same reason.
pub fn at_insert(path: &str, dir: bool) -> String {
    match (path.contains(' '), dir) {
        (false, false) => format!("@{path} "),
        (false, true) => format!("@{path}/"),
        (true, false) => format!("@\"{path}\" "),
        (true, true) => format!("@\"{path}/"),
    }
}

/// The best `FILE_ROWS` files for an `@` query, Pi's scoring: name equal 100,
/// name prefix 80, name substring 50, path substring 30, +10 for a folder;
/// ties go to the shallower, then the shorter path. A query with a `/` is
/// scoped to that folder and scores the part after it.
pub fn score_files(files: &[(String, bool)], query: &str) -> Vec<(String, bool)> {
    let q = query.to_lowercase();
    let (scope, q) = match q.rsplit_once('/') {
        Some((d, rest)) => (format!("{d}/"), rest.to_string()),
        None => (String::new(), q),
    };
    let mut hits: Vec<(i32, usize, &String, bool)> = files
        .iter()
        .filter_map(|(path, dir)| {
            let lower = path.to_lowercase();
            let inside = lower.strip_prefix(&scope).filter(|r| !r.is_empty())?;
            let name = inside.rsplit('/').next().unwrap_or(inside);
            let score = if q.is_empty() || name == q {
                100
            } else if name.starts_with(&q) {
                80
            } else if name.contains(&q) {
                50
            } else if inside.contains(&q) {
                30
            } else {
                return None;
            };
            Some((score + if *dir { 10 } else { 0 }, path.matches('/').count(), path, *dir))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.len().cmp(&b.2.len())).then(a.2.cmp(b.2)));
    hits.into_iter().take(FILE_ROWS).map(|(_, _, p, d)| (p.clone(), d)).collect()
}

/// The path-like token before the cursor that Tab completes: (char index it
/// starts at, text with `\` shown as `/`). It runs back to whitespace on the
/// cursor's line. ponytail: no quoting, so a path with a space completes only
/// up to the space; `@"…"` covers those.
pub fn path_token(input: &str, cursor: usize) -> (usize, String) {
    let chars: Vec<char> = input.chars().take(cursor).collect();
    let start = chars.iter().rposition(|c| c.is_whitespace()).map_or(0, |i| i + 1);
    (start, chars[start..].iter().collect::<String>().replace('\\', "/"))
}

/// Entries of the token's folder whose name starts with its last part, any
/// case, folders first: (completed token, is_dir). `~` is the home folder.
pub fn path_matches(token: &str) -> Vec<(String, bool)> {
    let (dir, prefix) = match token.rsplit_once('/') {
        Some((d, p)) => (format!("{d}/"), p),
        None => (String::new(), token),
    };
    let home = || std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let read = match dir.as_str() {
        "" => ".".to_string(),
        d if d == "~/" || d.starts_with("~/") => format!("{}/{}", home(), &d[2..]),
        "/" => "/".to_string(),
        d => d.to_string(),
    };
    let Ok(rd) = std::fs::read_dir(&read) else { return Vec::new() };
    let p = prefix.to_lowercase();
    let mut out: Vec<(String, bool)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.to_lowercase().starts_with(&p).then(|| (format!("{dir}{name}"), e.path().is_dir()))
        })
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    out
}

/// The longest start every candidate shares, ignoring case; the first
/// candidate's spelling wins.
pub fn common_prefix(items: &[(String, bool)]) -> String {
    let Some((first, _)) = items.first() else { return String::new() };
    let mut n = first.chars().count();
    for (s, _) in &items[1..] {
        n = n.min(first.chars().zip(s.chars()).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count());
    }
    first.chars().take(n).collect()
}

/// Put `insert` in place of chars `start..cursor`; the cursor lands after it.
pub fn replace_span(input: &str, start: usize, cursor: usize, insert: &str) -> (String, usize) {
    let head: String = input.chars().take(start).collect();
    let tail: String = input.chars().skip(cursor).collect();
    (format!("{head}{insert}{tail}"), start + insert.chars().count())
}

/// Tab with no menu open: complete the path before the cursor. One match goes
/// in outright, several fill what they share and open the list, none says so.
fn tab_complete(app: &mut App) {
    let (start, token) = path_token(&app.input, app.cursor);
    let found = path_matches(&token);
    let done = |(p, dir): &(String, bool)| if *dir { format!("{p}/") } else { format!("{p} ") };
    match found.len() {
        0 => app.notice = Some(("no match".into(), std::time::Instant::now())),
        1 => (app.input, app.cursor) = replace_span(&app.input, start, app.cursor, &done(&found[0])),
        _ => {
            let shared = common_prefix(&found);
            let fill = if shared.chars().count() > token.chars().count() { shared } else { token };
            (app.input, app.cursor) = replace_span(&app.input, start, app.cursor, &fill);
            let items = found
                .iter()
                .map(|f| Item::Path { start, insert: done(f), path: f.0.clone(), dir: f.1 })
                .collect();
            app.tab = Some((app.input.clone(), app.cursor, items));
        }
    }
}

/// Accept a path row: it replaces its token, and a folder `@` reference keeps
/// the picker open on what is inside it.
fn accept_path(app: &mut App, start: usize, insert: &str) {
    (app.input, app.cursor) = replace_span(&app.input, start, app.cursor, insert);
    app.tab = None;
}

/// Commands whose name completes `input`, or none when `input` isn't a lone
/// "/word" (a space means the arguments have started).
pub fn filter_cmds(cmds: &[Cmd], input: &str) -> Vec<Cmd> {
    let Some(q) = input.strip_prefix('/') else { return Vec::new() };
    if q.contains(char::is_whitespace) {
        return Vec::new();
    }
    let q = q.to_lowercase();
    cmds.iter().filter(|c| c.name[1..].to_lowercase().starts_with(&q)).cloned().collect()
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
    /// Running "⠋" rows as (ToolStart id, line index, waited on the user's
    /// permission). Parallel calls settle in any order, so each keeps its own.
    pub tool_rows: Vec<(u64, usize, bool)>,
    pub ask_line: Option<usize>,  // index of the pending question's line
    pub fold: Option<(usize, Vec<String>)>, // the row a run of reads folds into, and its files
    pub retry_line: Option<usize>, // index of the counting "⚠ … retry n/3" line
    pub todo_line: Option<usize>,  // index of the todo list, which each update replaces
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
    pub turn_cached: Option<u64>, // prompt tokens read from the provider's cache this turn; None: never reported
    pub cache_pct: Option<u64>,   // share of the last call's prompt that was a cache hit
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
    /// Ctrl+T: fold each reasoning block to one row (the live one keeps a
    /// two-row preview). Saved as "hide_thinking" in the global config.
    pub hide_thinking: bool,
    /// Ctrl+Q while working: each runs as its own task after this one ends.
    /// Steers (Enter while working) live in ai_core, which delivers them.
    pub follow: std::collections::VecDeque<String>,
    /// The panel's rows, refreshed by sync_menu so a 50,000-file score runs
    /// once per edit, not once per caller per frame.
    pub menu: Vec<Item>,
    menu_key: MenuKey,
    /// Project files for the `@` picker, indexed once per draft on a thread
    /// (`index` until it reports) and dropped when a turn ends, since the
    /// agent may have created files.
    pub files: Option<Vec<(String, bool)>>,
    pub index: Option<Receiver<Vec<(String, bool)>>>,
    /// The list a Tab opened, valid while the input is still what it filled in.
    pub tab: Option<(String, usize, Vec<Item>)>,
    /// The / menu's commands, re-read from disk each time a lone "/" opens it.
    pub cmds: Vec<Cmd>,
    /// Mouse selection over the transcript as (anchor, head), each a
    /// (wrapped row, column); copied on release, cleared by a click or a key.
    pub sel: Option<((usize, usize), (usize, usize))>,
    /// The last left-press (when, column, row, clicks in a row), to count a
    /// double or triple click, and whether the button has dragged since.
    pub click: Option<(std::time::Instant, u16, u16, u8)>,
    pub dragged: bool,
}

/// Prefix on a transcript row that is present but not drawn until Ctrl+O.
/// A marker char beats a parallel Vec<bool>: app.lines is pushed to from a
/// dozen places, and two vectors that must stay in step is the bug.
pub const HIDDEN: &str = "\u{1}";

/// How long a first Ctrl+C stays armed for the second one.
const ARM_WINDOW: Duration = Duration::from_secs(2);
/// How long a transient confirmation stays on the status line.
const NOTICE_TTL: Duration = Duration::from_secs(3);
/// How close two presses must be to count as a double (or triple) click.
const MULTI_CLICK: Duration = Duration::from_millis(400);

impl App {
    pub fn new(model: String, session: String, lines: Vec<String>, history: Vec<String>, msg_num: usize,
               footer: crate::config::Footer) -> App {
        App {
            lines, current: String::new(),
            ask: None, input: String::new(), cursor: 0, done: true, model, session,
            msg_num, spinner: 0, scroll_up: 0,
            history, hist_idx: None, tool_rows: Vec::new(), ask_line: None, fold: None, retry_line: None, todo_line: None,
            notice: None, exit_armed: None,
            footer,
            turn_t0: std::time::Instant::now(), turn_tok: 0, turn_ctx: 0, turn_gen_ms: 0, turn_est: false,
            turn_cached: None, cache_pct: None,
            sess_tok: 0, branch: String::new(),
            thinking: false,
            menu_idx: 0, menu_top: 0, menu_for: String::new(), menu_off: None, fresh: true,
            pick: None, expand: false, hide_thinking: false, follow: Default::default(),
            menu: Vec::new(), menu_key: Default::default(), files: None, index: None, tab: None,
            cmds: builtins(),
            sel: None, click: None, dragged: false,
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
        stats_row(self.turn_tok, self.turn_ctx, self.turn_cached, self.turn_est, self.turn_gen_ms, self.turn_t0.elapsed().as_secs_f64())
    }

    /// Reset the selection when the filter text changed, then keep the
    /// selected row inside the visible window.
    pub fn sync_menu(&mut self) {
        if self.menu_for != self.input {
            self.menu_for = self.input.clone();
            self.menu_idx = 0;
            self.menu_top = 0;
            // the menu just opened: pick up templates and skills added since
            if self.input == "/" {
                self.cmds = commands(&mut Vec::new());
            }
        }
        if let Some(Ok(files)) = self.index.as_ref().map(|rx| rx.try_recv()) {
            self.files = Some(files);
            self.index = None;
        }
        if self.files.is_none() && self.index.is_none() && at_token(&self.input, self.cursor).is_some() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || { let _ = tx.send(ai_core::tools::project_files(FILE_CAP)); });
            self.index = Some(rx);
        }
        let key = (self.input.clone(), self.cursor, self.files.is_some(),
                   self.tab.as_ref().map(|t| (t.0.clone(), t.1)), self.menu_off.clone());
        if key != self.menu_key || self.pick.is_some() || self.ask.is_some() {
            self.menu = menu_items(self);
            self.menu_key = key;
        }
        let n = self.menu.len();
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

pub fn stats_row(tok: u64, ctx: u64, cached: Option<u64>, est: bool, gen_ms: u128, wall_s: f64) -> Option<String> {
    if tok == 0 {
        return None;
    }
    let tps = if gen_ms > 0 { tok as f64 * 1000.0 / gen_ms as f64 } else { 0.0 };
    let e = if est { "~" } else { "" };
    let done = clock(crate::session::epoch_secs(), utc_offset_min());
    // cache reads summed over the turn's calls: what the provider bills at the cheap rate
    let cached = cached.map(|c| format!(" · cached {}", kilo(c))).unwrap_or_default();
    Some(format!(
        "  · worked {} · done {done} · {e}{tok} tok · {tps:.1} tps · ctx {e}{}{cached}",
        human_dur(wall_s), kilo(ctx)))
}

/// Put the selection on the clipboard and say so on the status line.
fn copy_sel(app: &mut App, state: &RenderState) {
    let Some(sel) = app.sel else { return };
    let text = state.selected(sel);
    if text.trim().is_empty() {
        app.sel = None;
        return;
    }
    // OSC 52 asks the terminal itself (works over SSH); the local tool is the
    // fallback for a terminal that ignores it. Neither can see the other fail.
    let _ = io::Write::write_all(&mut io::stdout(), osc52(&text).as_bytes());
    let _ = io::Write::flush(&mut io::stdout());
    let n = text.lines().count();
    let s = if n == 1 { "" } else { "s" };
    let msg = match ai_core::tools::copy_text(&text) {
        Ok(()) => format!("copied {n} line{s}"),
        Err(e) => format!("copied {n} line{s} via the terminal only; {e}"),
    };
    app.notice = Some((msg, std::time::Instant::now()));
}

/// OSC 52: set the clipboard through the terminal (Windows Terminal, iTerm2,
/// kitty, WezTerm, and over SSH); a terminal without it ignores the sequence.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", ai_core::tools::b64(text.as_bytes()))
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
    // theme files first, so a saved name can be one of them
    let mut warn = super::theme::load(&resources::dirs("themes"));
    // a saved palette that is no longer in the table keeps the default and says
    // so below, rather than leaving the user wondering why /themes did nothing
    let bad_theme = cfg.theme.as_deref().filter(|n| !super::theme::set(n)).map(String::from);
    super::theme::set_light(cfg.light);
    let mut app = App::new(model, session, seed_lines, seed_history, seed_msg_num, cfg.footer.clone());
    app.hide_thinking = cfg.hide_thinking;
    // stderr is invisible under the alternate screen, so this goes in the
    // transcript — and stays there, a warning you must act on can't expire
    if let Some(e) = &cfg.err {
        app.lines.push(format!("  ⚠ {e}"));
        app.lines.push("  ⚠ no saved models, permissions or MCP servers loaded; saving is off".into());
    }
    if let Some(name) = bad_theme {
        app.lines.push(format!("  ⚠ no theme called {name}; using {}", super::theme::name()));
    }
    // a template, skill or theme file that did not load says why, once
    app.cmds = commands(&mut warn);
    app.lines.extend(warn.into_iter().map(|w| format!("  ⚠ {w}")));
    let mut state = RenderState::new();

    loop {
        app.sync_menu();
        render::draw(&app, &mut state)?;
        app.spinner = app.spinner.wrapping_add(1);
        // about once a second: an edit to the active theme's file applies live
        if app.spinner.is_multiple_of(20) {
            if let Some(e) = super::theme::reload_if_changed() {
                app.notice = Some((e, std::time::Instant::now()));
            }
        }

        if event::poll(Duration::from_millis(50))? {
            let ev = event::read()?;
            // The wheel only arrives as a Mouse event because the TUI captures
            // the mouse. Without capture the terminal turns it into Up/Down key
            // presses in the alternate screen, which this app reads as input
            // history — that is the "scrolling jumps to an old message" bug.
            // Capture also takes the terminal's own click-and-drag selection, so
            // selecting is done here; Shift+drag still reaches the terminal's.
            if let CEvent::Mouse(m) = ev {
                use event::{MouseButton::Left, MouseEventKind as M};
                match m.kind {
                    M::ScrollUp => {
                        app.scroll_up = (app.scroll_up + 3).min(state.max_scroll);
                    }
                    M::ScrollDown => {
                        app.scroll_up = app.scroll_up.saturating_sub(3);
                    }
                    M::Down(Left) => {
                        let now = std::time::Instant::now();
                        let n = match app.click {
                            Some((t, x, y, n)) if now - t < MULTI_CLICK && (x, y) == (m.column, m.row) => n % 3 + 1,
                            _ => 1,
                        };
                        app.click = Some((now, m.column, m.row, n));
                        app.dragged = false;
                        // a press below the transcript (input, status) just clears
                        app.sel = state.cell(m.column, m.row, app.scroll_up)
                            .filter(|_| (m.row as usize) < state.view_h)
                            .map(|c| if n > 1 { state.expand(c, n) } else { (c, c) });
                        if n > 1 {
                            copy_sel(&mut app, &state);
                        }
                    }
                    M::Drag(Left) => {
                        if let Some((a, _)) = app.sel {
                            // past the top or bottom edge: scroll a row and keep selecting
                            if m.row == 0 {
                                app.scroll_up = (app.scroll_up + 1).min(state.max_scroll);
                            } else if m.row as usize >= state.view_h {
                                app.scroll_up = app.scroll_up.saturating_sub(1);
                            }
                            app.sel = state.cell(m.column, m.row, app.scroll_up).map(|h| (a, h));
                            app.dragged = true;
                        }
                    }
                    M::Up(Left) => {
                        if app.dragged {
                            copy_sel(&mut app, &state);
                        } else if app.click.is_some_and(|c| c.3 == 1) {
                            app.sel = None; // a plain click
                        }
                        app.dragged = false;
                    }
                    _ => {}
                }
                continue;
            }
            // rows rewrap to the new width, so the selection no longer points at its text
            if let CEvent::Resize(..) = ev {
                app.sel = None;
            }
            if let CEvent::Key(k) = ev {
                if k.kind == KeyEventKind::Press {
                    app.sel = None;
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
                        // Alt+Up (Alt+Q where the terminal takes Alt+Up): queued
                        // messages back into the input; with none queued it is plain Up
                        (KeyCode::Up, m) | (KeyCode::Char('q'), m) if m.contains(KeyModifiers::ALT) && dequeue(&mut app) => {}
                        // ── slash-command menu (open while input is a lone "/word") ──
                        (KeyCode::Up, _) if !app.menu.is_empty() => {
                            app.menu_idx = app.menu_idx.saturating_sub(1);
                        }
                        (KeyCode::Down, _) if !app.menu.is_empty() => {
                            app.menu_idx += 1; // sync_menu clamps to the item count
                        }
                        // a path row: Tab and Enter both accept it, Enter never submits
                        (KeyCode::Tab | KeyCode::Enter, _) if matches!(app.menu.get(app.menu_idx), Some(Item::Path { .. })) => {
                            if let Some(Item::Path { start, insert, .. }) = app.menu.get(app.menu_idx).cloned() {
                                accept_path(&mut app, start, &insert);
                            }
                        }
                        (KeyCode::Tab, _) if !app.menu.is_empty() => {
                            if let Some(Item::Cmd(c)) = app.menu.get(app.menu_idx) {
                                app.input = format!("{} ", c.name);
                                app.cursor = app.input.chars().count();
                            }
                        }
                        (KeyCode::Esc, _) if !app.menu.is_empty() => {
                            app.menu_off = Some(app.input.clone());
                            app.tab = None;
                        }
                        (KeyCode::Enter, _) if !app.menu.is_empty() => {
                            let Some(Item::Cmd(c)) = app.menu.get(app.menu_idx) else { continue };
                            let cmd = c.name.to_string();
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
                                dequeue(&mut app); // pi: an abort hands the queue back
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
                        // .rusti/clips and an @ reference to it is typed into the
                        // input, which submit attaches (F03); copied image files
                        // come in as one reference each. Both chords, because
                        // Windows Terminal binds ctrl+v to its own text paste and
                        // usually swallows it — alt+v is the one that always arrives.
                        (KeyCode::Char('v'), m)
                            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::ALT) =>
                        {
                            match ai_core::tools::clipboard_image() {
                                Ok(paths) => {
                                    let text: String = paths.iter().map(|p| at_insert(p, false)).collect();
                                    let byte = app.input.char_indices().nth(app.cursor)
                                        .map(|(i, _)| i).unwrap_or(app.input.len());
                                    app.input.insert_str(byte, &text);
                                    app.cursor += text.chars().count();
                                    app.notice = Some((format!("pasted {}", paths.join(", ")), std::time::Instant::now()));
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
                        // Ctrl+Q: a follow-up, run as its own task when this one ends
                        // (Alt+Enter is the newline fallback, so it cannot be this key)
                        (KeyCode::Char('q'), KeyModifiers::CONTROL) if app.ask.is_none() => {
                            let raw = app.input.trim().to_string();
                            if raw.starts_with('/') || raw.starts_with('!') {
                                app.notice = Some(("only a message can be queued".into(), std::time::Instant::now()));
                            } else if !raw.is_empty() {
                                app.fresh = false;
                                app.input.clear();
                                app.cursor = 0;
                                if app.done {
                                    start_task(&mut app, job_tx, raw);
                                } else {
                                    app.follow.push_back(raw);
                                }
                            }
                        }
                        // Tab with no menu open completes the path before the cursor
                        (KeyCode::Tab, _) => tab_complete(&mut app),
                        // Shift+Tab cycles the permission mode: manual → plan → auto → devil.
                        // The status line names it in place, so cycling leaves the transcript alone.
                        (KeyCode::BackTab, _) => ai_core::set_mode(ai_core::mode().next()),
                        // Ctrl+T folds reasoning blocks; like Ctrl+O it is only a redraw
                        (KeyCode::Char('t'), KeyModifiers::CONTROL) => toggle_thinking(&mut app),
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
                                // "!!cmd": the same, but the output is for the user only
                                let (cmd, to_model) = match cmd.strip_prefix('!') {
                                    Some(c) => (c.trim().to_string(), false),
                                    None => (cmd.trim().to_string(), true),
                                };
                                if cmd.is_empty() {
                                    app.lines.push("  ✗ usage: !<shell command> (!!<shell command> keeps it from the model)".into());
                                } else if !app.done {
                                    app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
                                } else {
                                    app.flush();
                                    app.scroll_up = 0;
                                    app.done = false;
                                    if app.history.last().map(|h| h != &raw).unwrap_or(true) {
                                        app.history.push(raw.clone());
                                    }
                                    let _ = job_tx.send(Job::Bash { cmd, to_model });
                                }
                            } else if !app.done {
                                // a steer: shown as pending until the agent takes it
                                ai_core::steer(raw.clone(), image_refs(&raw));
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
                ai_core::Event::Text(t) => { app.flush(); push_text(&mut app, t); }
                ai_core::Event::ToolStart { id, summary } => {
                    narrate(&mut app);
                    app.flush();
                    app.thinking = false;
                    app.lines.push(format!("  ⠋ {summary}"));
                    app.tool_rows.push((id, app.lines.len() - 1, false));
                }
                ai_core::Event::ToolEnd { id, summary, ok, ms, output } => tool_end(&mut app, id, summary, ok, ms, &output),
                ai_core::Event::SessionName(name) => app.session = name,
                ai_core::Event::Git(b) => app.branch = b,
                ai_core::Event::Notice(t) => {
                    app.notice = Some((t, std::time::Instant::now()));
                    // an /mcp toggle finished off-thread: show the server's new state
                    if app.pick.as_ref().is_some_and(|p| p.kind == PickKind::Mcp) {
                        pick_mcp(&mut app);
                    }
                }
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
                    app.cache_pct = None;
                }
                ai_core::Event::Usage { tokens, prompt, est, gen_ms, cached } => {
                    app.turn_tok += tokens;
                    app.sess_tok += tokens;
                    app.turn_ctx = prompt;
                    app.turn_gen_ms += gen_ms;
                    app.turn_est |= est;
                    if let Some(c) = cached {
                        app.turn_cached = Some(app.turn_cached.unwrap_or(0) + c);
                    }
                    // kept across turns, unlike ctx: it describes the last call, which is still the last call
                    app.cache_pct = cached.map(|c| c * 100 / prompt.max(1));
                }
                ai_core::Event::Ask { question, choices, reply } => ask(&mut app, question, choices, reply),
                ai_core::Event::Reload { exe, args } => return Ok(Exit::Reload { exe, args }),
                ai_core::Event::CompactStart { reason, messages, .. } => {
                    app.flush();
                    let why = if reason == "overflow" { " (the provider said the context was full)" } else { "" };
                    app.lines.push(format!("  ⟳ compacting context: {messages} messages → summary{why}"));
                }
                // the summary stays behind Ctrl+O: it is for the model, and
                // streaming it into the transcript buried the actual work
                ai_core::Event::CompactEnd { summary, tokens } => {
                    app.flush();
                    app.lines.push(format!("  ⟳ compacted: ~{}k tokens of context now  (ctrl+o)", (tokens + 500) / 1000));
                    app.lines.extend(summary.lines().map(|l| format!("{HIDDEN}  · {l}")));
                }
                ai_core::Event::Delivered(t) => {
                    app.flush();
                    app.msg_num += 1;
                    app.lines.push(format!("{}› {t}", app.msg_num));
                    if app.history.last() != Some(&t) {
                        app.history.push(t);
                    }
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
                    app.thinking = false;
                    app.fold = None;
                    (app.files, app.index) = (None, None); // the turn may have made or removed files
                    if let Some(l) = app.turn_stats() {
                        app.lines.push(l);
                    }
                    app.done = true;
                    app.scroll_up = 0;
                    if !app.lines.is_empty() {
                        app.lines.push(String::new());
                    }
                    next_queued(&mut app, ok, job_tx);
                }
            }
        }
    }
}

/// Slash commands: /use <name> switches model, /model (no arg) lists saved
/// profiles or (with a name) switches like /use, /resume loads a saved session,
/// /new starts an empty one,
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
        "/new" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                let name = (!arg.is_empty()).then(|| arg.to_string());
                let _ = job_tx.send(Job::New { name, history: app.history.clone() });
            }
        }
        "/settings" => pick_settings(app),
        // /themes <name> switches outright; bare /themes opens the list. Both
        // re-read the theme files, so one added since startup is there too
        "/themes" => {
            warn_once(app, super::theme::load(&resources::dirs("themes")));
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
            // the status line carries the state and what it means
            ai_core::set_plan(on);
        }
        "/commit" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                start_task(app, job_tx, COMMIT_TASK.into());
            }
        }
        "/compact" => {
            if !app.done {
                app.lines.push("  ✗ finish or Esc-interrupt the current task first".into());
            } else {
                app.done = false; // the summary is a model call: show it working
                let _ = job_tx.send(Job::Compact((!arg.is_empty()).then(|| arg.to_string())));
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
        _ if BUILTINS.iter().any(|(n, _, soon)| *n == cmd && *soon) => {
            app.lines.push(format!("  ℹ {cmd} is queued, not built yet — see harness/open-work.md"))
        }
        _ if run_extension(app, job_tx, raw, cmd, arg) => {}
        _ => app.lines.push(format!("  ✗ unknown command: {cmd}")),
    }
    false
}

/// `/name args` for a prompt template, `/skill:name args` for a skill, and
/// `/server:prompt args` for an MCP prompt: each becomes the message sent,
/// while the transcript shows what you typed. Templates and skills are read
/// from disk now, so an edit applies without a restart. False when nothing
/// has that name.
fn run_extension(app: &mut App, job_tx: &Sender<Job>, raw: &str, cmd: &str, arg: &str) -> bool {
    if is_builtin(cmd) {
        return false;
    }
    let name = &cmd[1..];
    let job = if let Some(skill) = name.strip_prefix("skill:") {
        let Some(s) = resources::skills(&mut Vec::new()).into_iter().find(|s| s.name == skill) else { return false };
        resources::skill_message(&s, arg).map(Job::Task)
    } else if let Some(t) = resources::templates(&mut Vec::new()).into_iter().find(|t| t.name == name) {
        Ok(Job::Task(resources::expand(&t.body, &resources::split_args(arg))))
    } else if let Some((server, p)) = ai_core::mcp::prompts().into_iter().find(|(s, p)| format!("{s}:{}", p.name) == name) {
        ai_core::mcp::prompt_args(&p, resources::split_args(arg))
            .map(|args| Job::McpPrompt { server, name: p.name.clone(), args })
            .map_err(|e| format!("{e} — usage: /{name} {}", p.hint()))
    } else {
        return false;
    };
    match job {
        Err(e) => app.lines.push(format!("  ✗ {e}")),
        Ok(_) if !app.done => app.lines.push("  ✗ finish or Esc-interrupt the current task first".into()),
        Ok(job) => {
            begin_task(app, raw.to_string());
            let _ = job_tx.send(job);
        }
    }
    true
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

/// A question for the user: a permission prompt or one the model asks.
pub fn ask(app: &mut App, question: String, choices: Vec<(String, String)>, reply: tokio::sync::oneshot::Sender<String>) {
    app.flush();
    // A permission prompt belongs to the tool row it gates: that row
    // turns into the question and back, leaving nothing behind. A
    // question the model asks (no fixed choices) keeps its own row.
    // permissions are asked right after each ToolStart, so it is the newest row
    let tool = app.tool_rows.last().map(|r| r.1).filter(|&i| !choices.is_empty() && i < app.lines.len());
    match tool.and_then(|i| app.lines[i].strip_prefix("  ⠋ ").map(|t| (i, t.to_string()))) {
        Some((i, t)) => {
            app.lines[i] = format!("  ? {t}  allow?");
            app.ask_line = Some(i);
            if let Some(r) = app.tool_rows.last_mut() {
                r.2 = true;
            }
        }
        None => {
            app.lines.push(format!("  ℹ {question}"));
            app.ask_line = Some(app.lines.len() - 1);
        }
    }
    app.ask = Some((question.clone(), reply));
    // a fixed set of answers is a choice, not a sentence to type:
    // it opens the picker that /model and /resume already use.
    // A typed answer needs the prompt: a list left open (/themes,
    // /resume…) would take the keys into its filter instead
    if choices.is_empty() {
        app.pick = None;
    } else {
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
pub fn tool_end(app: &mut App, id: u64, summary: String, ok: bool, ms: u128, output: &str) {
    let time = if ms >= 100 { format!("  {}", ai_core::took(ms)) } else { String::new() };
    let pos = app.tool_rows.iter().position(|r| r.0 == id);
    let (row, hand) = pos.map(|p| app.tool_rows.remove(p)).map_or((None, false), |r| (Some(r.1), r.2));
    let hand = if hand { "  ✋" } else { "" };
    let tail: Vec<String> = ai_core::fail_tail(output).into_iter().map(|r| format!("{HIDDEN}{r}")).collect();
    let line = if ok {
        format!("  ✓ {summary}{time}{hand}")
    } else {
        let why = ai_core::fail_reason(output);
        let more = if tail.len() > 1 { "  (ctrl+o)" } else { "" };
        format!("  ✗ {summary}  {why}{time}{hand}{more}")
    };
    let row = row.filter(|&i| i < app.lines.len() && app.lines[i].starts_with("  ⠋ "));
    let read = summary.strip_prefix("read ").filter(|_| ok).map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string());
    let fold = app.fold.take();
    match (row, read) {
        // this read follows the last one with nothing visible between them
        (Some(i), Some(file)) if fold.as_ref().is_some_and(|(j, _)| *j < i && app.lines[j + 1..i].iter().all(|l| l.starts_with(HIDDEN))) => {
            let (j, mut files) = fold.unwrap();
            files.push(file);
            app.lines[j] = format!("  ✓ read {} files  {}", files.len(), files.join(" · "));
            app.lines.remove(i);
            shift_rows(app, i, -1);
            app.fold = Some((j, files));
            insert_rows(app, i, tail);
        }
        (Some(i), read) => {
            app.lines[i] = line;
            app.fold = read.map(|f| (i, vec![f]));
            insert_rows(app, i + 1, tail);
        }
        (None, _) => {
            app.lines.push(line);
            app.lines.extend(tail);
        }
    }
}

/// Warnings that a re-read can repeat (a theme file that still fails): each
/// is said once, not again on every /themes.
/// ponytail: scans the transcript; a seen-set if that ever shows up
fn warn_once(app: &mut App, warns: Vec<String>) {
    for w in warns {
        let row = format!("  ⚠ {w}");
        if !app.lines.contains(&row) {
            app.lines.push(row);
        }
    }
}

/// The todo tool's list, or what is left when it is cleared.
fn is_todo(t: &str) -> bool {
    t == "  todo list cleared" || ["  ☑ ", "  ◐ ", "  ☐ "].iter().any(|g| t.starts_with(g))
}

/// A whole line of text into the transcript. The todo list is state, so a
/// new version replaces the last one where it stands: a plan updated ten
/// times left ten copies of itself.
pub fn push_text(app: &mut App, t: String) {
    let last = app.todo_line.filter(|&i| is_todo(&t) && app.lines.get(i).is_some_and(|l| is_todo(l)));
    match last {
        Some(i) => app.lines[i] = t,
        None => {
            if is_todo(&t) {
                app.todo_line = Some(app.lines.len());
            }
            app.lines.push(t);
        }
    }
}

/// The output rows go right under their own tool row, not at the end, where
/// a parallel call's row could still be running below it.
fn insert_rows(app: &mut App, at: usize, rows: Vec<String>) {
    let n = rows.len() as isize;
    app.lines.splice(at..at, rows);
    shift_rows(app, at, n);
}

/// Every live row index at or past `at` moves by `by`, after rows there were
/// inserted (+) or removed (-).
fn shift_rows(app: &mut App, at: usize, by: isize) {
    let live = app.tool_rows.iter_mut().map(|r| &mut r.1);
    let others = [&mut app.ask_line, &mut app.retry_line, &mut app.todo_line].into_iter().flatten();
    for i in live.chain(others).chain(app.fold.as_mut().map(|f| &mut f.0)) {
        if *i >= at {
            *i = i.saturating_add_signed(by);
        }
    }
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

/// Alt+Up / Esc: every queued message, steers first, back into the input
/// above the draft, blank-line separated. False when nothing was queued.
fn dequeue(app: &mut App) -> bool {
    let mut q = ai_core::take_steers();
    q.extend(app.follow.drain(..));
    if q.is_empty() {
        return false;
    }
    if !app.input.trim().is_empty() {
        q.push(std::mem::take(&mut app.input));
    }
    app.input = q.join("\n\n");
    app.cursor = app.input.chars().count();
    true
}

/// TaskEnd: a steer that arrived as the task finished missed its delivery
/// point, so it goes to the front of the follow-ups; after a success the
/// first follow-up starts. A failure leaves the queue showing for Alt+Up.
fn next_queued(app: &mut App, ok: bool, job_tx: &Sender<Job>) {
    for s in ai_core::take_steers().into_iter().rev() {
        app.follow.push_front(s);
    }
    if ok {
        if let Some(t) = app.follow.pop_front() {
            start_task(app, job_tx, t);
        }
    }
}

/// Show the message, reset the per-turn counters, hand it to the agent thread.
fn start_task(app: &mut App, job_tx: &Sender<Job>, raw: String) {
    begin_task(app, raw.clone());
    let _ = job_tx.send(Job::Task(raw));
}

/// start_task without the send: what a turn looks like on screen as it starts.
fn begin_task(app: &mut App, raw: String) {
    app.flush();
    drop_mcp_ok(app);
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
    app.turn_cached = None;
    if app.history.last().map(|h| h != &raw).unwrap_or(true) {
        app.history.push(raw);
    }
}

/// The "✓ mcp x: N tools" rows are a startup notice: once you have typed, you
/// know what loaded. A ✗ row stays, since a server that failed needs acting on.
/// They are seeded before any row an index points at, so every live index
/// shifts by the number removed.
fn drop_mcp_ok(app: &mut App) {
    let before = app.lines.len();
    // a server with prompts adds ", N prompts as /x:…" after "tools"
    app.lines.retain(|l| !l.starts_with("  ✓ mcp "));
    shift_rows(app, 0, -((before - app.lines.len()) as isize));
}

/// The mode's status-line name and its one-line meaning.
pub fn mode_label(m: ai_core::Mode) -> (&'static str, &'static str) {
    match m {
        ai_core::Mode::Manual => ("manual", "every write, command and fetch asks"),
        ai_core::Mode::Plan => ("plan", "read-only, writes and commands refused"),
        ai_core::Mode::Auto => ("auto", "project edits run unasked, risky ones ask"),
        ai_core::Mode::Devil => ("DEVIL", "no prompts, root guard off"),
    }
}

/// The status line: the permission mode, then what this session is and how
/// full it is, fitted to `w` columns. Every segment except the mode is
/// toggleable via /settings — the mode is what you are IN, not decoration, so
/// hiding it would hide why a write is refused, or why it ran unasked. The
/// mode's description gives way first on a narrow terminal, its name never.
pub fn footer_right(
    f: &crate::config::Footer, mode: ai_core::Mode, session: &str, model: &str, branch: &str,
    sess_tok: u64, ctx: u64, limit: u64, cache_pct: Option<u64>, w: usize,
) -> String {
    let mut parts = Vec::new();
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
    // a server that never reports cache hits gets no segment, not a false 0%
    if let (true, Some(p)) = (f.cache, cache_pct) {
        parts.push(format!("cache {p}%"));
    }
    let (name, desc) = mode_label(mode);
    let rest: String = parts.iter().map(|p| format!(" · {p}")).collect();
    let room = w.saturating_sub(name.len() + 2 + super::columns(&rest));
    let desc = match super::columns(desc) {
        n if n <= room => desc.to_string(),
        // ponytail: under ~8 columns a cut description is noise, so it goes
        _ if room >= 8 => format!("{}…", super::truncate_str(desc, room - 1)),
        _ => return super::truncate_str(&format!("{name}{rest}"), w),
    };
    format!("{name}: {desc}{rest}")
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
                        "{:<18}{:<9}{:<13}{:<11}{}", // fits an auto name with its -N suffix
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
            ai_core::tools::set_vision(p.vision != Some(false));
            ai_core::use_profile_context(p.context);
            let _ = job_tx.send(Job::Model {
                url: p.url.clone(),
                key: p.key.clone(),
                model: p.model.clone(),
                cache: p.cache,
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
    let rows: Vec<(String, String)> = super::theme::all()
        .into_iter()
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
        row("cache", f.cache, "share of the last prompt read from the provider's cache"),
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

/// Enter on an /mcp row. Turning a server ON connects it, which can take up
/// to a minute per handshake step and can fail, so it runs on its own thread:
/// the screen keeps drawing and Esc still works. The outcome arrives as a
/// Notice, which also refreshes this list.
fn toggle_mcp(app: &mut App, name: &str) {
    let on = crate::ai_core::mcp::status()
        .into_iter()
        .find(|(n, ..)| n == name)
        .map(|(_, enabled, ..)| enabled)
        .unwrap_or(false);
    let msg = format!("mcp {name}: {}", if on { "switching off…" } else { "connecting…" });
    app.notice = Some((msg, std::time::Instant::now()));
    let name = name.to_string();
    std::thread::spawn(move || {
        let msg = match crate::ai_core::mcp::set_enabled(&name, !on) {
            Ok(m) => m,
            Err(e) => format!("mcp {name}: {e}"),
        };
        ai_core::emit(ai_core::Event::Notice(msg));
    });
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
        "cache" => f.cache = !f.cache,
        _ => return,
    }
    let mut cfg = crate::config::Config::load();
    cfg.footer = app.footer.clone();
    if let Err(e) = cfg.save() {
        app.notice = Some((format!("settings not saved: {e}"), std::time::Instant::now()));
    }
    pick_settings(app);
}

/// Flip hide_thinking and save it, the same load-then-save as toggle_footer.
fn toggle_thinking(app: &mut App) {
    app.hide_thinking = !app.hide_thinking;
    let what = if app.hide_thinking { "hidden" } else { "shown" };
    let mut cfg = crate::config::Config::load();
    cfg.hide_thinking = app.hide_thinking;
    let msg = match cfg.save() {
        Ok(()) => format!("thinking {what}"),
        Err(e) => format!("thinking {what}; not saved: {e}"),
    };
    app.notice = Some((msg, std::time::Instant::now()));
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

    /// A mode change lives on the status line only: toggling /plan back and
    /// forth adds nothing to the transcript and raises no notice.
    #[test]
    fn plan_toggle_leaves_the_transcript_alone() {
        let mut a = app();
        let (tx, _rx) = std::sync::mpsc::channel();
        for _ in 0..4 {
            handle_command("/plan", &mut a, &tx);
        }
        assert!(a.lines.is_empty() && a.notice.is_none());
        assert_eq!(ai_core::mode(), ai_core::Mode::Manual, "an even number of toggles is back where it started");
    }

    /// /mcp on a server that never answers its handshake ran up to three
    /// 60 s waits on the UI thread: no frame drawn, no key read. The toggle
    /// must hand the connect off and return at once with a progress notice.
    #[test]
    fn mcp_toggle_does_not_wait_for_the_handshake() {
        // RUSTI_HOME is set only in a child run of this one test, so the other
        // tests in this process never see the temp home or its connect thread
        if std::env::var_os("RUSTI_HOME").is_none() {
            let home = std::env::temp_dir().join(format!("rusti_mcp_toggle_{}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            // sort reads stdin to EOF and prints nothing until then: a silent server
            std::fs::write(home.join("config.json"), r#"{"mcp":{"silent":{"command":"sort","enabled":false}}}"#).unwrap();
            let test = concat!(module_path!(), "::mcp_toggle_does_not_wait_for_the_handshake");
            let test = test.split_once("::").unwrap().1;
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([test, "--exact", "--nocapture"])
                .env("RUSTI_HOME", &home)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
            return;
        }
        let mut a = app();
        let t = std::time::Instant::now();
        toggle_mcp(&mut a, "silent");
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "blocked {:?}", t.elapsed());
        assert_eq!(a.notice.as_ref().map(|(m, _)| m.as_str()), Some("mcp silent: connecting…"));
    }

    fn app() -> App {
        App::new("m".into(), "s".into(), vec![], vec![], 0, Default::default())
    }
    fn start(app: &mut App, t: &str) {
        app.lines.push(format!("  ⠋ {t}"));
        app.tool_rows.push((0, app.lines.len() - 1, false));
    }
    fn shown(app: &App) -> Vec<&str> {
        app.lines.iter().filter(|l| !l.starts_with(HIDDEN)).map(String::as_str).collect()
    }

    /// F14: state that is re-sent is not re-printed. Each todo update replaces
    /// the list where it stands, even after rows were inserted above it, and
    /// a theme file that still fails warns once however often /themes opens.
    #[test]
    fn todo_updates_and_repeated_warnings_do_not_stack() {
        let mut a = app();
        push_text(&mut a, "  ☐ read\n  ☐ edit".into());
        start(&mut a, "todo (2 items)");
        insert_rows(&mut a, 0, vec!["1› plan it".into()]);
        push_text(&mut a, "  ☑ read\n  ◐ edit".into());
        push_text(&mut a, "  · not a todo".into());
        push_text(&mut a, "  todo list cleared".into());
        assert_eq!(a.lines, ["1› plan it", "  todo list cleared", "  ⠋ todo (2 items)", "  · not a todo"]);
        // a transcript replaced under it (a resume) gets a fresh list, not an overwrite
        a.lines = vec!["other session".into(), "x".into()];
        push_text(&mut a, "  ☐ again".into());
        assert_eq!(a.lines, ["other session", "x", "  ☐ again"]);

        let mut a = app();
        for _ in 0..3 {
            warn_once(&mut a, vec!["theme broken.json: expected value".into()]);
        }
        warn_once(&mut a, vec!["theme other.json: expected value".into()]);
        assert_eq!(a.lines, ["  ⚠ theme broken.json: expected value", "  ⚠ theme other.json: expected value"]);
    }

    /// The first message clears the startup "✓ mcp" rows but keeps a failure.
    #[test]
    fn first_message_clears_mcp_startup_rows() {
        let mut a = App::new("m".into(), "s".into(),
            vec!["  ✓ mcp mem: 15 tools".into(), "  ✗ mcp gh: spawn failed".into()], vec![], 0, Default::default());
        start(&mut a, "run pwd");
        let (tx, _rx) = std::sync::mpsc::channel();
        start_task(&mut a, &tx, "hi".into());
        assert_eq!(a.lines, vec!["  ✗ mcp gh: spawn failed", "  ⠋ run pwd", "1› hi"]);
        assert_eq!(a.tool_rows, [(0, 1, false)], "a live index follows its row");
    }

    /// Design D's tool rows: a run of reads is one row, a failure is one row
    /// carrying its reason with the output behind Ctrl+O, a permission prompt
    /// is the tool's own row and leaves nothing behind, and the time shown is
    /// the command's, not the user's thinking time.
    #[test]
    fn tool_rows_fold_reads_and_carry_their_own_outcome() {
        let mut a = app();
        start(&mut a, "read harness/feature_list.json");
        tool_end(&mut a, 0, "read harness/feature_list.json".into(), true, 13, "{...}");
        start(&mut a, "read harness/progress.md");
        tool_end(&mut a, 0, "read harness/progress.md".into(), true, 11, "# Progress");
        start(&mut a, "read AGENTS.md");
        tool_end(&mut a, 0, "read AGENTS.md".into(), true, 9, "x");
        assert_eq!(shown(&a), vec!["  ✓ read 3 files  feature_list.json · progress.md · AGENTS.md"]);
        assert_eq!(a.lines.iter().filter(|l| l.starts_with(HIDDEN)).count(), 3, "each read's output stays behind Ctrl+O");

        // a visible row between two reads ends the fold
        a.lines.push("  ┆ now the log".into());
        start(&mut a, "read x.md");
        tool_end(&mut a, 0, "read x.md".into(), true, 5, "x");
        assert_eq!(shown(&a).last(), Some(&"  ✓ read x.md"), "under 100ms shows no time");

        // permission: the row becomes the question, then settles in place
        let mut a = app();
        start(&mut a, "run pwd");
        let (tx, _rx) = tokio::sync::oneshot::channel();
        a.lines[0] = "  ? run pwd  allow?".into();
        a.ask_line = Some(0);
        a.ask = Some(("allow run_command run pwd?".into(), tx));
        a.tool_rows[0].2 = true;
        close_ask(&mut a, "yes");
        assert_eq!(a.lines, vec!["  ⠋ run pwd"], "answering restores the running row, no ℹ line left behind");
        tool_end(&mut a, 0, "run pwd".into(), false, 16,
            "[exit 1]\n'pwd' is not recognized as an internal or external command,\noperable program or batch file.");
        assert_eq!(shown(&a), vec!["  ✗ run pwd  'pwd' is not recognized as an internal or external command,  ✋  (ctrl+o)"]);
        assert!(a.lines.len() > 1, "the full output is kept for Ctrl+O");

        // time only when it is worth reading
        let mut a = app();
        start(&mut a, "run cargo test");
        tool_end(&mut a, 0, "run cargo test".into(), true, 4200, "ok");
        assert_eq!(shown(&a), vec!["  ✓ run cargo test  4.2s"]);
    }

    /// Parallel calls: every row starts before any ends, and each ToolEnd
    /// settles its own row by id, whatever order they finish in; a failure's
    /// hidden output goes under its own row, and the rows below keep their
    /// place (F07).
    #[test]
    fn parallel_rows_settle_by_id_in_any_order() {
        let mut a = app();
        for (id, t) in [(7, "run slow"), (8, "run fast"), (9, "read a.rs")] {
            a.lines.push(format!("  ⠋ {t}"));
            a.tool_rows.push((id, a.lines.len() - 1, false));
        }
        tool_end(&mut a, 8, "run fast".into(), false, 5, "[exit 1]\nboom\nmore");
        tool_end(&mut a, 9, "read a.rs".into(), true, 5, "x");
        assert_eq!(shown(&a), ["  ⠋ run slow", "  ✗ run fast  boom  (ctrl+o)", "  ✓ read a.rs"]);
        assert!(a.lines[2].starts_with(HIDDEN), "the failure's output sits under its own row");
        assert_eq!(a.tool_rows, [(7, 0, false)]);
        tool_end(&mut a, 7, "run slow".into(), true, 3000, "done");
        assert_eq!(shown(&a), ["  ✓ run slow  3.0s", "  ✗ run fast  boom  (ctrl+o)", "  ✓ read a.rs"]);
        assert!(a.tool_rows.is_empty());
    }

    /// `@` opens only at a token boundary, on the cursor's own line, and a
    /// quoted reference may hold spaces until its quote closes.
    #[test]
    fn at_token_starts_only_at_a_word_boundary() {
        let at = |s: &str| at_token(s, s.chars().count());
        assert_eq!(at("@"), Some((0, String::new())));
        assert_eq!(at("look at @src/ma"), Some((8, "src/ma".into())));
        assert_eq!(at("see (@foo"), Some((5, "foo".into())));
        assert_eq!(at("mail me@host.com"), None, "an email is not a reference");
        assert_eq!(at("@foo bar"), None, "a space ends the reference");
        assert_eq!(at("first\n@x"), Some((6, "x".into())), "only the cursor's line");
        assert_eq!(at("@\"my di"), Some((0, "my di".into())));
        assert_eq!(at("@\"my dir/a.txt\" "), None, "a closed quote is a finished reference");
        assert_eq!(at_token("@abc tail", 2), Some((0, "a".into())), "the query stops at the cursor");
    }

    /// Pi's ranking: exact name, then prefix, then substring, then path
    /// substring, folders a step up; ties go shallower then shorter, and a
    /// query with a `/` searches inside that folder.
    #[test]
    fn at_picker_ranks_files_and_scopes_to_a_folder() {
        let f = |p: &str, d: bool| (p.to_string(), d);
        let files = vec![
            f("src", true), f("src/tui", true), f("src/main.rs", false), f("src/tui/app.rs", false),
            f("docs/main-notes.md", false), f("domain.rs", false), f("readme.md", false),
        ];
        let paths = |q: &str| score_files(&files, q).into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        assert_eq!(paths("main"), vec!["src/main.rs", "docs/main-notes.md", "domain.rs"]);
        assert_eq!(paths("MAIN.RS")[0], "src/main.rs", "case does not matter");
        assert_eq!(paths("tui"), vec!["src/tui", "src/tui/app.rs"], "a folder outranks a path hit");
        assert_eq!(paths("src/"), vec!["src/tui", "src/main.rs", "src/tui/app.rs"], "scoped, shallow first, folder first");
        assert_eq!(paths("src/tui/a"), vec!["src/tui/app.rs"]);
        assert!(paths("zzz").is_empty());
        let many: Vec<_> = (0..40).map(|i| f(&format!("f{i}.rs"), false)).collect();
        assert_eq!(score_files(&many, "").len(), FILE_ROWS);
    }

    /// Accepting a row replaces the `@` token; a folder keeps the picker
    /// open, and a path with a space is quoted.
    #[test]
    fn accepting_an_at_row_inserts_the_reference() {
        assert_eq!(at_insert("src/main.rs", false), "@src/main.rs ");
        assert_eq!(at_insert("src", true), "@src/");
        assert_eq!(at_insert("my dir/a b.txt", false), "@\"my dir/a b.txt\" ");
        assert_eq!(at_insert("my dir", true), "@\"my dir/");

        let mut a = app();
        a.files = Some(vec![("src".into(), true), ("src/main.rs".into(), false), ("me@host".into(), false)]);
        a.input = "read @sr please".into();
        a.cursor = 8; // after "@sr"
        a.sync_menu();
        assert_eq!(a.menu.len(), 2, "{:?}", a.menu);
        let Item::Path { start, insert, .. } = a.menu[0].clone() else { panic!("{:?}", a.menu) };
        accept_path(&mut a, start, &insert);
        assert_eq!((a.input.as_str(), a.cursor), ("read @src/ please", 10));
        a.sync_menu();
        assert_eq!(a.menu.len(), 1, "the folder's picker stays open on what is inside it");
        let Item::Path { start, insert, .. } = a.menu[0].clone() else { panic!() };
        accept_path(&mut a, start, &insert);
        assert_eq!(a.input, "read @src/main.rs  please");
        a.sync_menu();
        assert!(a.menu.is_empty(), "a file closes the picker");

        a.input = "/re".into();
        a.cursor = 3;
        a.sync_menu();
        assert!(matches!(a.menu[0], Item::Cmd(_)), "the slash menu is unchanged");
        assert_eq!(a.menu.len(), 3, "/resume /rename /reload");
        a.menu_off = Some(a.input.clone());
        a.sync_menu();
        assert!(a.menu.is_empty(), "Esc closes it");
    }

    /// Tab with no menu: one match is inserted, several fill their shared
    /// start and open the list (folders first), none says so; `\` reads as `/`.
    #[test]
    fn tab_completes_the_path_before_the_cursor() {
        let dir = std::env::temp_dir().join(format!("rusti_tab_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Notes")).unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        std::fs::write(dir.join("only.md"), "").unwrap();
        let base = dir.to_string_lossy().replace('\\', "/");

        let mut a = app();
        a.input = format!("cat {base}/on");
        a.cursor = a.input.chars().count();
        tab_complete(&mut a);
        assert_eq!(a.input, format!("cat {base}/only.md "));
        assert!(a.tab.is_none());

        a.input = format!("cat {}\\no", dir.to_string_lossy());
        a.cursor = a.input.chars().count();
        tab_complete(&mut a);
        assert_eq!(a.input, format!("cat {base}/Notes"), "shared start, any case (first row spells it), / for \\");
        a.sync_menu();
        let rows: Vec<String> = a.menu.iter().map(|i| match i { Item::Path { insert, .. } => insert.clone(), _ => panic!() }).collect();
        assert_eq!(rows, vec![format!("{base}/Notes/"), format!("{base}/notes.txt ")], "folders first");
        let end = a.cursor;
        a.cursor = 0;
        a.sync_menu();
        assert!(a.menu.is_empty(), "moving the cursor closes the list, so it cannot splice at a stale start");
        a.cursor = end;
        a.sync_menu();
        let Item::Path { start, insert, .. } = a.menu[1].clone() else { panic!() };
        accept_path(&mut a, start, &insert);
        assert_eq!(a.input, format!("cat {base}/notes.txt "));

        a.input = format!("{base}/zz");
        a.cursor = a.input.chars().count();
        tab_complete(&mut a);
        assert_eq!(a.notice(), Some("no match"));
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(path_token("a b\\c", 5), (2, "b/c".into()));
        assert_eq!(common_prefix(&[("Abc".into(), false), ("abd".into(), false)]), "Ab");
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

    /// A steer still queued when its task ends missed its delivery point: it
    /// starts next, ahead of the Ctrl+Q follow-ups, so nothing typed is lost.
    /// A failure starts nothing, and Alt+Up / Esc hand the whole queue back.
    #[test]
    fn a_late_steer_becomes_the_next_follow_up() {
        let _q = ai_core::STEER_TEST.lock().unwrap_or_else(|e| e.into_inner());
        ai_core::take_steers();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut a = app();
        a.follow.push_back("then commit".into());
        ai_core::steer("use tabs".into(), vec![]); // Enter pressed as the task finished
        next_queued(&mut a, true, &tx);
        assert!(matches!(rx.try_recv(), Ok(Job::Task(t)) if t == "use tabs"));
        assert_eq!(a.lines.last().map(String::as_str), Some("1› use tabs"));
        assert!(!a.done && ai_core::steers().is_empty());
        assert_eq!(a.follow, ["then commit"]);

        next_queued(&mut a, false, &tx);
        assert!(rx.try_recv().is_err(), "a failed task does not run the queue");

        ai_core::steer("no, spaces".into(), vec![]);
        a.input = "draft".into();
        assert!(dequeue(&mut a));
        assert_eq!(a.input, "no, spaces\n\nthen commit\n\ndraft");
        assert!(a.follow.is_empty() && ai_core::steers().is_empty());
        assert!(!dequeue(&mut a), "nothing queued: Alt+Up stays history recall");
    }

    /// Q16: the / menu lists built-ins, then templates, then skills, then
    /// MCP prompts, tagged by source; a template named like a built-in loses.
    /// A template runs as its expansion while the transcript shows what you typed.
    #[test]
    fn extensions_join_the_menu_and_run_as_their_text() {
        let mut a = app();
        a.cmds.push(Cmd { name: "/review".into(), desc: "<file> - Review a file".into(), soon: false, tag: "u".into() });
        a.cmds.push(Cmd { name: "/skill:pdf".into(), desc: "PDFs".into(), soon: false, tag: "p".into() });
        let names: Vec<String> = filter_cmds(&a.cmds, "/re").into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["/resume", "/rename", "/reload", "/review"], "built-ins first");
        assert_eq!(filter_cmds(&a.cmds, "/skill:")[0].tag, "p");

        let (tx, rx) = std::sync::mpsc::channel();
        assert!(!run_extension(&mut a, &tx, "/nope x", "/nope", "x"), "nothing by that name");
        assert!(is_builtin("/Commit") && is_builtin("/commit") && !is_builtin("/review"), "built-ins match in any case");
        assert!(!run_extension(&mut a, &tx, "/Commit x", "/Commit", "x"), "a built-in's name never runs an extension");
        begin_task(&mut a, "/review src/a.rs".into());
        assert_eq!(a.lines.last().map(String::as_str), Some("1› /review src/a.rs"), "the transcript shows what was typed");
        assert_eq!(a.history.last().map(String::as_str), Some("/review src/a.rs"), "and Up recalls it");
        assert!(rx.try_recv().is_err(), "begin_task sends nothing itself");
    }

    /// Submit attaches the `@` references that name an image file, quoted
    /// ones too, each once, and never a path that is only mentioned (F03).
    #[test]
    fn image_refs_take_only_at_references_to_image_files() {
        let dir = std::env::temp_dir().join(format!("rusti_refs_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a b")).unwrap();
        let files = [dir.join("shot.png"), dir.join("a b/x.JPG"), dir.join("notes.txt")];
        for f in &files {
            std::fs::write(f, "").unwrap();
        }
        let [shot, spaced, text] = files.map(|f| f.to_string_lossy().into_owned());
        let msg = format!("compare @{shot} with @\"{spaced}\" and @{shot} again,\nnot {shot}, nor @{text} or @missing.png");
        assert_eq!(image_refs(&msg), vec![shot.clone(), spaced.clone()]);
        assert!(image_refs(&format!("look at {shot}")).is_empty(), "mentioned, not referenced");
        assert_eq!(image_refs(&at_insert(&spaced, false)), vec![spaced.clone()], "what Alt+V types is what submit takes");
        assert_eq!(image_refs(&format!("what is in @{shot}?")), vec![shot.clone()], "trailing punctuation");
        assert_eq!(image_refs(&format!("see @{shot}.")), vec![shot.clone()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
