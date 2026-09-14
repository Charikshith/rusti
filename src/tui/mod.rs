// TUI module: pi-style renderer on the alternate screen with synchronized output.
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

/// Undo what the TUI did to the terminal. Shared by the normal exit and the
/// panic hook, and idempotent: leaving an alternate screen you are not on and
/// disabling raw mode that is already off are both no-ops.
pub fn restore_terminal() {
    let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
}

/// The UI loop runs on the main thread; every worker in this program is an
/// unnamed `thread::spawn`, so `name()` separates them. A panicking worker must
/// NOT restore the terminal — the TUI is still drawing on it, and tearing the
/// screen down under a live render is worse than the panic.
fn on_ui_thread() -> bool {
    std::thread::current().name() == Some("main")
}

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
    pub cli_args: Vec<String>, // this process's argv, for /reload's relaunch
}

/// Jobs the TUI sends to the agent thread: a user task, a model switch,
/// resuming a saved session file, browsing/branching the session tree, or
/// rebuilding + relaunching the binary in place.
pub enum Job {
    Task(String),
    Model { url: String, key: String, model: String },
    ResumePath(String), // load this session file and continue it
    Rename(String),     // rename the active session file
    Tree,               // send the selectable tree rows for the picker
    Select(String),     // move the active leaf to this entry (pi-style branch)
    Undo,               // put back the files the last turn changed, then rewind to before it
    Bash(String),       // "!cmd": run it here, show the output, and let the model see it
    Export(Option<String>), // write the active path out as markdown
    Reload,
}

/// Absolute path to rusti's own Cargo.toml, baked in at build time — so
/// /reload rebuilds rusti's source even when the agent's cwd is some other
/// project it's coding on.
const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// True only when both paths exist and name the same file. Both must exist:
/// canonicalize() fails for a missing path, and treating two errors as equal
/// would call any two non-existent paths the same file.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Cargo uplifts the binary to `target/release/rusti`, and if that file is this
/// very process the write fails — Windows locks a running exe, Unix returns
/// ETXTBSY. That is the whole reason /reload used to build into its own target
/// tree, at the cost of a second copy of every dependency. Renaming a running
/// binary IS allowed on both platforms, so move it aside instead and let cargo
/// have the name; it lands beside the staged copies, where the sweep in
/// stage_reload_exe deletes it on a later reload. Cargo re-links the uplifted
/// binary whenever it is missing, so nothing needs rebuilding for this.
fn free_the_output_path(target_dir: &str) {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    let built = std::path::Path::new(target_dir).join("release").join(format!("rusti{ext}"));
    let Ok(me) = std::env::current_exe() else { return };
    if !same_file(&me, &built) {
        return; // already running a staged copy: cargo can overwrite freely
    }
    let aside = std::path::Path::new(target_dir).join(format!("rusti-old-{}{ext}", now_ms()));
    let _ = std::fs::rename(&built, aside);
}

/// Copy the freshly built binary to a unique name and return its path. We run
/// copies, never `target/release/rusti` itself, so cargo can always overwrite
/// that file: Windows locks a running exe, and on Windows every previous
/// generation stays alive as a thin wrapper (see run()), so a fixed pair of
/// names would run out on the third reload. Stale copies from finished chains
/// are swept here — including the `rusti-old-*` that free_the_output_path moved
/// aside; in-use ones simply fail to delete and go on the next sweep.
fn stage_reload_exe(target_dir: &str) -> io::Result<String> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    if let Ok(rd) = std::fs::read_dir(target_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("rusti-") && name.ends_with(ext) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let exe = format!("{target_dir}/rusti-{}{ext}", now_ms());
    std::fs::copy(format!("{target_dir}/release/rusti{ext}"), &exe)?;
    Ok(exe)
}

/// Rebuilds the relaunch argv: drops stale --url/--key/--model/--session/
/// --resume/--tree, then re-adds the live model (so a mid-session /use switch
/// survives the restart even if model.json disagrees), the live session name
/// (so a /rename survives too) and --resume when there is a session to resume. --tui is assumed already present (reload only runs in
/// TUI mode).
fn build_relaunch_args(
    cli_args: &[String],
    url: &str,
    key: &str,
    model: &str,
    session: Option<&str>,
    resume: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    let mut it = cli_args.iter();
    while let Some(a) = it.next() {
        if a == "--url" || a == "--key" || a == "--model" || a == "--session" {
            it.next();
            continue;
        }
        if a == "--resume" || a == "--tree" {
            continue;
        }
        args.push(a.clone());
    }
    args.push("--url".into());
    args.push(url.into());
    args.push("--key".into());
    args.push(key.into());
    args.push("--model".into());
    args.push(model.into());
    if let Some(name) = session {
        args.push("--session".into());
        args.push(name.into());
    }
    if resume {
        args.push("--resume".into());
    }
    args
}

/// Move the active session to a new name under DIR. Returns the note to show,
/// or the reason it can't be done. A session that hasn't saved yet has no file
/// to move, so pointing `path` at the new name is the whole rename.
fn rename_session(session: &mut Session, name: &str) -> Result<String, String> {
    if !crate::session::valid_name(name) {
        return Err("letters, digits, . _ - only (max 40)".into());
    }
    let old = session.path.clone();
    let new_path = crate::session::path_for(name);
    if new_path == old {
        return Err(format!("already named '{name}'"));
    }
    if std::path::Path::new(&new_path).exists() {
        return Err(format!("a session named '{name}' already exists"));
    }
    if std::path::Path::new(&old).exists() {
        std::fs::create_dir_all(crate::session::DIR)
            .and_then(|_| std::fs::rename(&old, &new_path))
            .map_err(|e| format!("rename failed: {e}"))?;
    }
    let was = crate::session::name_of(&old);
    session.path = new_path;
    Ok(format!("renamed {was} → {name}"))
}

/// Reconstruct visible transcript lines from a resumed session's active path,
/// so /reload (and plain --resume) don't present a blank screen despite prior
/// history — the agent's own message list already has it, the TUI doesn't.
fn render_history(session: &Session) -> (Vec<String>, Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut history = Vec::new();
    let mut n = 0;
    // Tool results are chained after the assistant entry that called them, in
    // call order (see feat-026), so a queue pairs each result with its summary.
    let mut pending: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    for e in session.path() {
        match e.role.as_str() {
            "user" => {
                n += 1;
                lines.push(format!("{n}› {}", e.content));
                history.push(e.content.clone());
            }
            "assistant" => {
                let calls = e
                    .tool_calls
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if !e.content.is_empty() {
                    lines.push(e.content.clone());
                    if calls.is_empty() {
                        lines.push(String::new()); // the turn ended here
                    }
                }
                for c in calls {
                    let name = c["function"]["name"].as_str().unwrap_or("?");
                    let args: serde_json::Value = c["function"]["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::Value::Null);
                    pending.push_back(ai_core::tool_summary(name, &args));
                }
            }
            // without the tool lines a resumed turn looks like the agent did
            // nothing between the question and the answer
            "tool" => {
                let summary = pending.pop_front().unwrap_or_else(|| "tool".into());
                let mark = match e.ok {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "·", // written before `ok` was recorded
                };
                lines.push(format!("  {mark} {summary}"));
                // the result is right here in the entry, so Ctrl+O works on a
                // resumed turn too; failures stay visible, as they were live
                let hide = e.ok != Some(false);
                lines.extend(ai_core::fail_tail(&e.content).into_iter()
                    .map(|r| if hide { format!("{}{r}", app::HIDDEN) } else { r }));
            }
            _ => {} // the system prompt is not transcript
        }
    }
    (lines, history, n)
}

/// Move the leaf to `id` pi-style and rebuild the transcript: a user entry
/// rewinds to its parent and offers its text for editing, an assistant entry
/// continues right after it.
fn branch_at(session: &mut Session, id: &str, event_tx: &mpsc::Sender<ai_core::Event>) {
    let is_user = session.entries.iter().any(|e| e.id == id && e.role == "user");
    match session.select(id) {
        None => { let _ = event_tx.send(ai_core::Event::Text(format!("  ✗ no entry {id}"))); }
        Some(text) => {
            let _ = session.save();
            let (lines, history, msg_num) = render_history(session);
            let _ = event_tx.send(ai_core::Event::Resumed { lines, history, msg_num });
            let _ = event_tx.send(ai_core::Event::Notice(format!("branched at {id}; continuing from here")));
            if is_user {
                let _ = event_tx.send(ai_core::Event::Prefill(text));
            }
        }
    }
}

/// Entry point: spawns agent in background thread, renders TUI or plain stream.
pub fn run(cfg: TuiConfig) -> io::Result<()> {
    let TuiConfig { client, session, model, cli_args } = cfg;
    let (mut seed_lines, seed_history, seed_msg_num) = render_history(&session);
    // MCP servers connect before the first frame, not in the agent thread: the
    // model's tool list is built per turn, and a half-connected server would
    // advertise nothing on the turn you just typed
    seed_lines.extend(ai_core::mcp::connect_all());
    let seed_name = crate::session::name_of(&session.path);

    let (event_tx, event_rx) = mpsc::channel();
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    ai_core::set_event_sink(event_tx.clone());
    let cancel = Arc::new(AtomicBool::new(false)); // Esc sets it; agent thread checks it
    let cancel_agent = Arc::clone(&cancel);
    let mut agent = AgentHandle(Some(std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut session = session;
        let mut client = client;
        let branch = |tx: &mpsc::Sender<ai_core::Event>| {
            let _ = tx.send(ai_core::Event::Git(ai_core::git_branch().unwrap_or_default()));
        };
        branch(&event_tx); // queued before the first frame
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
                    // no transcript note: the bottom status line already shows the model
                }
                Ok(Job::ResumePath(path)) => {
                    let loaded = Session::load_from(&path);
                    if loaded.is_empty() {
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ✗ nothing to resume in {path}")));
                    } else {
                        let n = loaded.entries.len();
                        let leaf = loaded.active.clone().unwrap_or_default();
                        let (lines, history, msg_num) = render_history(&loaded);
                        session = loaded;
                        let _ = event_tx.send(ai_core::Event::Resumed { lines, history, msg_num });
                        let _ = event_tx.send(ai_core::Event::SessionName(crate::session::name_of(&path)));
                        let _ = event_tx.send(ai_core::Event::Notice(format!("resumed {path} ({n} entries, leaf {leaf})")));
                    }
                }
                Ok(Job::Reload) => {
                    let _ = event_tx.send(ai_core::Event::ToolStart("cargo build --release".into()));
                    let manifest = format!("{MANIFEST_DIR}/Cargo.toml");
                    // rusti's own target dir, warm from ordinary cargo builds: a
                    // private one meant a second copy of every dependency and a
                    // cold first build. free_the_output_path handles the reason
                    // it was private — the running exe holding its own path.
                    let target_dir = format!("{MANIFEST_DIR}/target");
                    free_the_output_path(&target_dir);
                    let t0 = std::time::Instant::now();
                    let out = std::process::Command::new("cargo")
                        .args(["build", "--release", "--manifest-path", &manifest, "--target-dir", &target_dir])
                        .output();
                    let ms = t0.elapsed().as_millis();
                    let fail = |event_tx: &mpsc::Sender<ai_core::Event>, detail: String| {
                        let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: false, ms, output: String::new() });
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ✗ reload failed:\n{detail}")));
                        let _ = event_tx.send(ai_core::Event::TaskEnd { ok: true, error: None });
                    };
                    match out {
                        Ok(o) if o.status.success() => match stage_reload_exe(&target_dir) {
                            Ok(exe) => {
                                let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: true, ms, output: String::new() });
                                // a turn that failed before its first save leaves entries only in
                                // memory; persist now so --resume finds them (and doesn't exit 1)
                                let resume = !session.is_empty() && session.save().is_ok();
                                // an unnamed session lives at the root session.json: no flag
                                let named = (session.path != crate::session::PATH)
                                    .then(|| crate::session::name_of(&session.path));
                                let args = build_relaunch_args(&cli_args, &client.url, &client.key,
                                    &client.model, named.as_deref(), resume);
                                let _ = event_tx.send(ai_core::Event::Reload { exe, args });
                            }
                            Err(e) => fail(&event_tx, format!("built OK but couldn't stage the binary: {e}")),
                        },
                        Ok(o) => {
                            let mut msg = String::from_utf8_lossy(&o.stderr).into_owned();
                            if msg.trim().is_empty() {
                                msg = String::from_utf8_lossy(&o.stdout).into_owned();
                            }
                            let tail: Vec<&str> = msg.lines().rev().take(25).collect();
                            fail(&event_tx, tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
                        }
                        Err(e) => fail(&event_tx, format!("failed to run cargo: {e}")),
                    }
                }
                Ok(Job::Rename(name)) => {
                    // a rename is bookkeeping; a failed one is something to read
                    let msg = match rename_session(&mut session, &name) {
                        Ok(note) => {
                            let _ = event_tx.send(ai_core::Event::SessionName(name.clone()));
                            ai_core::Event::Notice(note)
                        }
                        Err(e) => ai_core::Event::Text(format!("  ✗ {e}")),
                    };
                    let _ = event_tx.send(msg);
                }
                Ok(Job::Tree) => {
                    let rows: Vec<(String, String)> = crate::tree::rows(&session)
                        .into_iter()
                        .filter(|(_, selectable, _)| *selectable)
                        .map(|(id, _, label)| (label, id))
                        .collect();
                    if rows.is_empty() {
                        let _ = event_tx.send(ai_core::Event::Text("  ✗ no session yet".into()));
                    } else {
                        let _ = event_tx.send(ai_core::Event::Tree(rows));
                    }
                }
                Ok(Job::Select(id)) => branch_at(&mut session, &id, &event_tx),
                Ok(Job::Export(to)) => {
                    let file = to.unwrap_or_else(|| format!("{}.md", crate::session::name_of(&session.path)));
                    let ev = match std::fs::write(&file, session.export_markdown()) {
                        Ok(()) => ai_core::Event::Notice(format!("exported to {file}")),
                        Err(e) => ai_core::Event::Text(format!("  ✗ could not write {file}: {e}")),
                    };
                    let _ = event_tx.send(ev);
                }
                Ok(Job::Bash(cmd)) => {
                    let summary = format!("$ {cmd}");
                    let _ = event_tx.send(ai_core::Event::ToolStart(summary.clone()));
                    let t0 = std::time::Instant::now();
                    let (ok, out) = ai_core::tools::run_command(&cmd, 0);
                    // output on success too: seeing it is the whole point of "!"
                    let _ = event_tx.send(ai_core::Event::ToolEnd {
                        summary, ok, ms: t0.elapsed().as_millis(), output: out.clone(),
                    });
                    if !session.is_empty() {
                        session.add(crate::session::Entry::new("user", format!("I ran `{cmd}` myself:\n{out}")), session.active.clone());
                        let _ = session.save();
                    }
                    let _ = event_tx.send(ai_core::Event::TaskEnd { ok: true, error: None });
                }
                Ok(Job::Undo) => {
                    for l in ai_core::tools::undo_turn() {
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ↶ {l}")));
                    }
                    let last_user = session.path().iter().rev().find(|e| e.role == "user").map(|e| e.id.clone());
                    match last_user {
                        Some(id) => branch_at(&mut session, &id, &event_tx),
                        None => { let _ = event_tx.send(ai_core::Event::Text("  ✗ nothing to undo".into())); }
                    }
                }
                Err(_) => break, // TUI exited
            }
            branch(&event_tx); // a turn (or a !command) may have switched or committed
        }
    })));

    if is_terminal::is_terminal(std::io::stdin()) {
        // A panic unwinds past the restore below, so without this the shell is
        // left in raw mode on the alternate screen: no echo, no prompt, and the
        // panic message painted on a buffer that is about to vanish. Restore
        // first, then delegate, so the message lands on the primary buffer.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if on_ui_thread() {
                restore_terminal();
            }
            prev(info);
        }));
        terminal::enable_raw_mode()?;
        // Alternate screen: the TUI gets a fresh canvas every run (no stale
        // transcript from the previous one), and leaving restores the shell's
        // primary buffer exactly — history intact, no gap, no leftovers.
        // Transcripts persist via session.json + /resume, not the scrollback.
        execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        let res = app::ui_loop(&job_tx, event_rx, model, seed_name, &cancel, seed_lines, seed_history, seed_msg_num);
        restore_terminal();
        // job_tx must drop before the join — the agent thread blocks in
        // job_rx.recv() until every Sender is gone, otherwise join() hangs.
        drop(job_tx);
        agent.0.take().map(|h| h.join());
        match res? {
            app::Exit::Quit => {
                // primary buffer is restored; the farewell lands right under
                // the launch line — no clear sequences, no gap, history intact
                println!("Come back again, boss");
                Ok(())
            }
            app::Exit::Reload { exe, args } => {
                let mut cmd = std::process::Command::new(exe);
                cmd.args(args);
                #[cfg(unix)]
                {
                    use std::os::unix::process::CommandExt;
                    let e = cmd.exec(); // only returns on failure
                    Err(io::Error::new(io::ErrorKind::Other, format!("relaunch failed: {e}")))
                }
                #[cfg(not(unix))]
                {
                    // no exec on Windows. If we exited after spawn(), the shell would
                    // take its prompt back and fight the child for console input — so
                    // stay alive as a thin wrapper and exit with the child's status.
                    let status = cmd
                        .status()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("relaunch failed: {e}")))?;
                    std::process::exit(status.code().unwrap_or(1));
                }
            }
        }
    } else {
        plain::run(event_rx)
    }
}

/// First line of an entry, capped — for /tree output.
/// Word wrap on byte length. Ponytail: naive — fine for ASCII.
pub fn word_wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 { return vec![String::new()]; }
    let mut out = Vec::new();
    for src in s.split('\n') {
        // Leading spaces are the row's marker indent ("  ✓ ", "  │ ") and
        // colorize_row matches on them, so they must survive the split: an
        // empty token would otherwise land in the `line.is_empty()` arm and
        // be dropped, leaving every status row uncolored.
        let body = src.trim_start_matches(' ');
        let indent = &src[..src.len() - body.len()];
        let w = width.saturating_sub(indent.len()).max(1);
        let first = out.len();
        let mut line = String::new();
        for word in body.split(' ') {
            if line.is_empty() {
                line = word.to_string();
            } else if line.len() + 1 + word.len() > w {
                out.push(line);
                line = word.to_string();
            } else {
                line.push(' ');
                line.push_str(word);
            }
        }
        out.push(line);
        out[first].insert_str(0, indent);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Entry;

    #[test]
    fn word_wrap_keeps_the_marker_indent_that_colorize_row_matches_on() {
        // the indent IS the colour key: drop it and ✓/✗/⠋/│/· all render plain
        assert_eq!(word_wrap("  ✓ Cargo.toml  0ms", 80), vec!["  ✓ Cargo.toml  0ms"]);
        assert_eq!(word_wrap("  │ thinking out loud", 80), vec!["  │ thinking out loud"]);
        assert_eq!(word_wrap("1› hello", 80), vec!["1› hello"]);
        // wrapping still happens, and the indent is charged against the width
        // so the first row + its indent still fits (truncate_str would cut it)
        let rows = word_wrap("  ✓ aaaa bbbb cccc", 10);
        assert_eq!(rows[0], "  ✓ aaaa");
        assert!(rows.iter().all(|r| r.len() <= 10), "{rows:?}");
        assert_eq!(rows.concat().replace(' ', ""), "✓aaaabbbbcccc");
        // blank and all-space lines survive unchanged
        assert_eq!(word_wrap("", 80), vec![""]);
        assert_eq!(word_wrap("  ", 80), vec!["  "]);
    }

    #[test]
    fn render_history_replays_prose_and_tool_lines_but_not_the_system_prompt() {
        let mut s = Session::new("m".into());
        let sys = s.add(Entry::new("system", "sys prompt".into()), None);
        let u1 = s.add(Entry::new("user", "first task".into()), Some(sys));
        let mut a1 = Entry::new("assistant", "Let me look.".into());
        a1.tool_calls = Some(serde_json::json!([
            {"function": {"name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}"}},
            {"function": {"name": "run_command", "arguments": "{\"command\":\"cargo test\"}"}},
        ]));
        let a1 = s.add(a1, Some(u1));
        let mut t1 = Entry::new("tool", "fn main…".into());
        t1.ok = Some(true);
        let t1 = s.add(t1, Some(a1));
        let mut t2 = Entry::new("tool", "[exit 101]".into());
        t2.ok = Some(false);
        let t2 = s.add(t2, Some(t1));
        s.add(Entry::new("assistant", "done".into()), Some(t2));

        let (lines, history, n) = render_history(&s);
        assert_eq!(n, 1);
        // the tool lines are the body of the turn: without them a resumed
        // session looks like the agent answered without doing anything
        assert_eq!(
            lines,
            vec![
                "1› first task".to_string(),
                "Let me look.".to_string(),
                "  ✓ src/main.rs".to_string(),   // summary + outcome, paired in call order
                // a resumed turn carries its output too, so Ctrl+O works on it:
                // the success tail marked hidden, the failure tail plainly visible
                format!("{}  · fn main…", app::HIDDEN),
                "  ✗ cargo test".to_string(),
                "  · [exit 101]".to_string(),
                "done".to_string(),
                String::new(), // the turn ended on prose
            ]
        );
        assert_eq!(history, vec!["first task".to_string()]);

        // an entry written before `ok` was recorded renders neutrally, not as a failure
        let mut old = Session::new("m".into());
        let u = old.add(Entry::new("user", "q".into()), None);
        let mut a = Entry::new("assistant", String::new()); // tool-call turn, no prose
        a.tool_calls = Some(serde_json::json!([{"function": {"name": "glob", "arguments": "{\"pattern\":\"*.rs\"}"}}]));
        let a = old.add(a, Some(u));
        old.add(Entry::new("tool", "x".into()), Some(a));
        // and its output hides like a success: an unknown outcome is not a
        // failure, so it does not get to push its tail on screen unasked
        assert_eq!(
            render_history(&old).0,
            vec!["1› q".to_string(), "  · *.rs".to_string(), format!("{}  · x", app::HIDDEN)]
        );
    }

    #[test]
    fn selecting_a_user_entry_branches_at_its_parent_and_offers_its_text() {
        let mut s = Session::new("m".into());
        let sys = s.add(Entry::new("system", "sys".into()), None);
        let u1 = s.add(Entry::new("user", "first".into()), Some(sys));
        let a1 = s.add(Entry::new("assistant", "ok".into()), Some(u1));
        let u2 = s.add(Entry::new("user", "second".into()), Some(a1.clone()));
        s.add(Entry::new("assistant", "bad turn".into()), Some(u2.clone()));

        // only user/assistant rows are offered; the leaf is marked
        let rows: Vec<_> = crate::tree::rows(&s).into_iter().filter(|r| r.1).collect();
        assert_eq!(rows.len(), 4);
        assert!(rows[3].2.ends_with(" ◀"));

        assert_eq!(s.select(&u2), Some("second".into())); // user: leaf moves to its parent
        let (lines, _, n) = render_history(&s);
        assert_eq!(n, 1);
        assert_eq!(lines, vec!["1› first".to_string(), "ok".to_string(), String::new()]);

        assert_eq!(s.select(&a1), Some("ok".into())); // assistant: continue right after it
        assert_eq!(s.active.as_deref(), Some(a1.as_str()));
    }

    #[test]
    fn rename_moves_the_session_file_and_refuses_bad_or_taken_names() {
        // cwd-relative, like the real session paths: fs::rename can't cross volumes
        let src = std::path::PathBuf::from("_rename_src_test.json");
        let mut s = Session::with_path("m".into(), &src.to_string_lossy());
        s.add(Entry::new("user", "hi".into()), None);
        s.save().unwrap();

        assert!(rename_session(&mut s, "bad name").is_err());
        let name = "rusti_rename_test";
        assert!(rename_session(&mut s, name).unwrap().ends_with(name));
        assert_eq!(s.path, crate::session::path_for(name));
        assert!(std::path::Path::new(&s.path).exists());
        assert!(!src.exists());
        assert!(rename_session(&mut s, name).is_err()); // already named that

        let mut other = Session::with_path("m".into(), "other.json");
        assert!(rename_session(&mut other, name).is_err()); // name taken
        std::fs::remove_file(&s.path).ok();
    }

    #[test]
    fn slash_menu_filters_on_prefix_and_closes_once_args_start() {
        assert_eq!(app::filter_cmds("/").len(), app::CMDS.len());
        let re: Vec<&str> = app::filter_cmds("/re").iter().map(|c| c.name).collect();
        assert_eq!(re, vec!["/resume", "/rename", "/reload"]);
        assert!(app::filter_cmds("/use x").is_empty()); // args started
        assert!(app::filter_cmds("hello").is_empty());
        assert!(app::filter_cmds("/zz").is_empty());
    }

    #[test]
    fn transcript_sits_on_the_input_instead_of_floating_at_the_top() {
        // short transcript: the gap goes ABOVE it, so the first message is
        // just over the prompt rather than stranded at row 0
        assert_eq!(render::transcript_window(3, 10, 0), (7, 0, 3));
        assert_eq!(render::transcript_window(0, 10, 0), (10, 0, 0)); // empty
        assert_eq!(render::transcript_window(10, 10, 0), (0, 0, 10)); // exactly full
        assert_eq!(render::transcript_window(100, 10, 0), (0, 90, 100)); // follows the bottom
        assert_eq!(render::transcript_window(100, 10, 5), (0, 85, 95)); // scrolled up 5
        assert_eq!(render::transcript_window(100, 10, 999), (0, 0, 10)); // can't pass the first line
        assert_eq!(render::transcript_window(3, 10, 5), (7, 0, 3)); // nothing to scroll to
        assert_eq!(render::transcript_window(5, 0, 0), (0, 5, 5)); // no room at all
    }

    #[test]
    fn shift_enter_drafts_move_the_cursor_by_line_not_through_history() {
        //  "ab\ncde\nf" — char indices: a0 b1 \n2 c3 d4 e5 \n6 f7
        let s = "ab\ncde\nf";
        assert_eq!(render::caret_at(s, 0), (0, 0));
        assert_eq!(render::caret_at(s, 2), (0, 2)); // end of line 0, before the \n
        assert_eq!(render::caret_at(s, 3), (1, 0)); // start of line 1
        assert_eq!(render::caret_at(s, 7), (2, 0));
        assert_eq!(render::caret_at(s, 8), (2, 1)); // end of the draft

        assert_eq!(app::move_line(s, 4, -1), 1); // col 1 of line 1 -> col 1 of line 0
        assert_eq!(app::move_line(s, 1, -1), 1); // first line: stays put
        assert_eq!(app::move_line(s, 1, 1), 4); // down keeps the column
        assert_eq!(app::move_line(s, 5, 1), 8); // col 2 -> line 2 is shorter, clamps to its end
        assert_eq!(app::move_line(s, 8, 1), 8); // last line: stays put
        assert_eq!(app::move_line("one line", 3, -1), 3); // nothing to move to
        assert_eq!(app::move_line("", 0, 1), 0);
    }

    #[test]
    fn picker_filter_matches_anywhere_in_the_row_ignoring_case() {
        let pick = |filter: &str| app::Pick {
            kind: app::PickKind::Model,
            title: "models".into(),
            rows: vec![
                ("qwen3.6-plus  cmc/Qwen/Qwen3.6-Plus".into(), "qwen3.6-plus".into()),
                ("glm-5.1  cmc/zai-org/GLM-5.1".into(), "glm-5.1".into()),
                ("kimi-k2.6  cmc/moonshotai/Kimi-K2.6".into(), "kimi-k2.6".into()),
            ],
            idx: 0,
            top: 0,
            filter: filter.into(),
        };
        let names = |f: &str| -> Vec<String> { pick(f).visible().iter().map(|(_, v)| v.clone()).collect() };
        assert_eq!(names("").len(), 3);
        assert_eq!(names("qwen"), vec!["qwen3.6-plus"]); // matches the lower-cased name
        assert_eq!(names("QWEN"), vec!["qwen3.6-plus"]); // and ignores the case typed
        assert_eq!(names("zai-org"), vec!["glm-5.1"]); // matches the model id, not just the name
        assert!(names("nope").is_empty());
    }

    #[test]
    fn footer_shows_only_the_parts_that_exist() {
        let on = crate::config::Footer::default();
        assert_eq!(
            app::footer_right(&on, false, "main", "mimo", "master", 4321, 25_000, 100_000),
            "main · mimo · ⎇ master · 4.3k tok · ctx 25%"
        );
        // fresh session: no name, no branch, nothing generated — but ctx is
        // reported at 0% rather than hidden, since it resets every turn
        assert_eq!(app::footer_right(&on, false, "", "mimo", "", 0, 0, 100_000), "mimo · ctx 0%");
        // plan mode leads, so a narrow terminal cuts it last
        assert_eq!(app::footer_right(&on, true, "", "mimo", "", 0, 0, 100_000), "plan · mimo · ctx 0%");
    }

    #[test]
    fn settings_hide_footer_segments_but_never_plan_mode() {
        let off = crate::config::Footer {
            session: false, model: false, branch: false, tokens: false, context: false,
        };
        // everything off still announces plan mode: it is why writes get refused
        assert_eq!(app::footer_right(&off, true, "main", "mimo", "master", 4321, 25_000, 100_000), "plan");
        assert_eq!(app::footer_right(&off, false, "main", "mimo", "master", 4321, 25_000, 100_000), "");
        let ctx_only = crate::config::Footer { context: true, ..off };
        assert_eq!(
            app::footer_right(&ctx_only, false, "main", "mimo", "master", 4321, 25_000, 100_000),
            "ctx 25%"
        );
    }

    #[test]
    fn turn_stats_row_reports_tps_over_generation_time_only() {
        // 120 tokens generated in 3s, turn took 8s wall (5s of it tool waits)
        assert_eq!(app::stats_row(120, 4321, false, 3000, 8.0).unwrap(), "  · 120 tok · 40.0 tps · 8.0s · ctx 4.3k");
        assert_eq!(app::stats_row(9, 950, true, 0, 1.0).unwrap(), "  · ~9 tok · 0.0 tps · 1.0s · ctx ~950");
        assert!(app::stats_row(0, 0, false, 100, 1.0).is_none());
    }

    #[test]
    fn picker_nav_walks_and_scrolls_the_visible_window() {
        // a 12-row list with 8 visible: window scrolls only when the cursor
        // passes its bottom edge, and never shows a hole at the top
        let (i, t) = app::picker_nav(0, 0, 12, 1);
        assert_eq!((i, t), (1, 0));
        let (i, t) = app::picker_nav(7, 0, 12, 1);
        assert_eq!((i, t), (8, 1)); // 8 == top + PICK_ROWS, so scroll
        let (i, t) = app::picker_nav(8, 1, 12, -1);
        assert_eq!((i, t), (7, 1)); // walk up within the window
        let (i, t) = app::picker_nav(0, 0, 12, -1);
        assert_eq!((i, t), (0, 0)); // clamped at the top
        // a short list never scrolls and clamps at the bottom
        let (i, t) = app::picker_nav(2, 0, 3, 1);
        assert_eq!((i, t), (2, 0));
    }

    #[test]
    fn relaunch_args_replace_model_and_dedupe_resume() {
        let cli: Vec<String> = ["--tui", "--url", "old", "--session", "stale", "--resume", "--tree"]
            .iter().map(|s| s.to_string()).collect();
        let args = build_relaunch_args(&cli, "new-url", "new-key", "new-model", Some("renamed"), true);
        assert_eq!(args, vec!["--tui", "--url", "new-url", "--key", "new-key", "--model", "new-model",
                              "--session", "renamed", "--resume"]);

        let no_resume = build_relaunch_args(&["--tui".to_string()], "u", "k", "m", None, false);
        assert!(!no_resume.contains(&"--resume".to_string()));
        assert!(!no_resume.contains(&"--session".to_string()));
    }

    /// The panic hook restores the terminal only from the UI thread. A worker
    /// panic must delegate straight to the previous hook, or it would tear the
    /// screen down while the TUI is still rendering on it.
    #[test]
    fn only_the_ui_thread_restores_the_terminal() {
        // the test harness runs each test on its own named thread, which is
        // exactly the "not main" case a panicking worker hits
        assert!(!on_ui_thread(), "a worker thread must not restore");
        let named = std::thread::Builder::new()
            .name("main".into())
            .spawn(|| on_ui_thread())
            .unwrap();
        assert!(named.join().unwrap(), "the UI thread must restore");
    }

    /// The trap in free_the_output_path: canonicalize() errors for a missing
    /// path, so treating (Err, Err) as equal would move a file aside that cargo
    /// was never going to write - or match two unrelated missing paths.
    #[test]
    fn same_file_needs_both_paths_to_exist() {
        let dir = std::env::temp_dir();
        let f = dir.join("rusti_same_file_test.bin");
        std::fs::write(&f, b"x").unwrap();
        let missing_a = dir.join("rusti_no_such_file_a.bin");
        let missing_b = dir.join("rusti_no_such_file_b.bin");

        assert!(same_file(&f, &f), "a file is itself");
        assert!(!same_file(&f, &missing_a), "an existing file is not a missing one");
        assert!(!same_file(&missing_a, &missing_b), "two missing paths are not the same file");
        // the same file reached by a different spelling still matches
        assert!(same_file(&f, &dir.join(".").join("rusti_same_file_test.bin")));
        let _ = std::fs::remove_file(&f);
    }
}
