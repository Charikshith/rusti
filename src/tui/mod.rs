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
/// resuming a saved session file, dumping the current session tree, or
/// rebuilding + relaunching the binary in place.
pub enum Job {
    Task(String),
    Model { url: String, key: String, model: String },
    ResumePath(String), // load this session file and continue it
    Rename(String),     // rename the active session file
    Tree,
    Reload,
}

/// Absolute path to rustypi's own Cargo.toml, baked in at build time — so
/// /reload rebuilds rustypi's source even when the agent's cwd is some other
/// project it's coding on.
const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// Copy the freshly built binary to a unique name and return its path. We run
/// copies, never `target/reload/release/rustypi` itself, so cargo can always
/// overwrite that file: Windows locks a running exe, and on Windows every
/// previous generation stays alive as a thin wrapper (see run()), so a fixed
/// pair of build dirs would run out on the third reload. Stale copies from
/// finished chains are swept here; in-use ones simply fail to delete.
fn stage_reload_exe(target_dir: &str) -> io::Result<String> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    if let Ok(rd) = std::fs::read_dir(target_dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("rustypi-") && name.ends_with(ext) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let exe = format!("{target_dir}/rustypi-{ts}{ext}");
    std::fs::copy(format!("{target_dir}/release/rustypi{ext}"), &exe)?;
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
    for e in session.path() {
        match e.role.as_str() {
            "user" => {
                n += 1;
                lines.push(format!("{n}› {}", e.content));
                history.push(e.content.clone());
            }
            "assistant" if !e.content.is_empty() => {
                lines.push(e.content.clone());
                lines.push(String::new());
            }
            _ => {} // system / tool / tool-call-only assistant entries: not shown
        }
    }
    (lines, history, n)
}

/// Entry point: spawns agent in background thread, renders TUI or plain stream.
pub fn run(cfg: TuiConfig) -> io::Result<()> {
    let TuiConfig { client, session, model, cli_args } = cfg;
    let (seed_lines, seed_history, seed_msg_num) = render_history(&session);
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
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ℹ resumed {path} ({n} entries, leaf {leaf})")));
                    }
                }
                Ok(Job::Reload) => {
                    let _ = event_tx.send(ai_core::Event::ToolStart("cargo build --release".into()));
                    let manifest = format!("{MANIFEST_DIR}/Cargo.toml");
                    // own dir, not target/release: the user usually launched from there,
                    // and Windows won't let cargo overwrite that running exe
                    let target_dir = format!("{MANIFEST_DIR}/target/reload");
                    let t0 = std::time::Instant::now();
                    let out = std::process::Command::new("cargo")
                        .args(["build", "--release", "--manifest-path", &manifest, "--target-dir", &target_dir])
                        .output();
                    let ms = t0.elapsed().as_millis();
                    let fail = |event_tx: &mpsc::Sender<ai_core::Event>, detail: String| {
                        let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: false, ms });
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ✗ reload failed:\n{detail}")));
                        let _ = event_tx.send(ai_core::Event::TaskEnd { ok: true, error: None });
                    };
                    match out {
                        Ok(o) if o.status.success() => match stage_reload_exe(&target_dir) {
                            Ok(exe) => {
                                let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: true, ms });
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
                    let msg = match rename_session(&mut session, &name) {
                        Ok(note) => {
                            let _ = event_tx.send(ai_core::Event::SessionName(name.clone()));
                            format!("  ℹ {note}")
                        }
                        Err(e) => format!("  ✗ {e}"),
                    };
                    let _ = event_tx.send(ai_core::Event::Text(msg));
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
        // Alternate screen: the TUI gets a fresh canvas every run (no stale
        // transcript from the previous one), and leaving restores the shell's
        // primary buffer exactly — history intact, no gap, no leftovers.
        // Transcripts persist via session.json + /resume, not the scrollback.
        execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide)?;
        let res = app::ui_loop(&job_tx, event_rx, model, seed_name, &cancel, seed_lines, seed_history, seed_msg_num);
        let _ = execute!(stdout(), cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
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
fn one_line(s: &str) -> String {
    s.replace('\n', " ").chars().take(80).collect()
}

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
    fn render_history_reconstructs_transcript_skipping_system_and_tool_calls() {
        let mut s = Session::new("m".into());
        let sys = s.add(Entry::new("system", "sys prompt".into()), None);
        let u1 = s.add(Entry::new("user", "first task".into()), Some(sys));
        let a1 = s.add(Entry::new("assistant", String::new()), Some(u1)); // tool-call turn, no text
        let t1 = s.add(Entry::new("tool", "tool output".into()), Some(a1));
        s.add(Entry::new("assistant", "done".into()), Some(t1));

        let (lines, history, n) = render_history(&s);
        assert_eq!(n, 1);
        assert_eq!(lines, vec!["1› first task".to_string(), "done".to_string(), String::new()]);
        assert_eq!(history, vec!["first task".to_string()]);
    }

    #[test]
    fn rename_moves_the_session_file_and_refuses_bad_or_taken_names() {
        // cwd-relative, like the real session paths: fs::rename can't cross volumes
        let src = std::path::PathBuf::from("_rename_src_test.json");
        let mut s = Session::with_path("m".into(), &src.to_string_lossy());
        s.add(Entry::new("user", "hi".into()), None);
        s.save().unwrap();

        assert!(rename_session(&mut s, "bad name").is_err());
        let name = "rustypi_rename_test";
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
    fn turn_stats_row_reports_tps_over_generation_time_only() {
        // 120 tokens generated in 3s, turn took 8s wall (5s of it tool waits)
        assert_eq!(app::stats_row(120, false, 3000, 8.0).unwrap(), "  · 120 tok · 40.0 tps · 8.0s");
        assert_eq!(app::stats_row(9, true, 0, 1.0).unwrap(), "  · ~9 tok · 0.0 tps · 1.0s");
        assert!(app::stats_row(0, false, 100, 1.0).is_none());
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
}
