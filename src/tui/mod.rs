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
    pub cli_args: Vec<String>, // this process's argv, for /reload's relaunch
}

/// Jobs the TUI sends to the agent thread: a user task, a model switch,
/// resuming session.json, dumping the current session tree, or rebuilding +
/// relaunching the binary in place.
pub enum Job {
    Task(String),
    Model { url: String, key: String, model: String },
    Resume,
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

/// Rebuilds the relaunch argv: drops stale --url/--key/--model/--resume/--tree,
/// then re-adds the live model (so a mid-session /use switch survives the
/// restart even if model.json disagrees) and --resume when there's a saved
/// session to resume. --tui is assumed already present (reload only runs in
/// TUI mode).
fn build_relaunch_args(cli_args: &[String], url: &str, key: &str, model: &str, resume: bool) -> Vec<String> {
    let mut args = Vec::new();
    let mut it = cli_args.iter();
    while let Some(a) = it.next() {
        if a == "--url" || a == "--key" || a == "--model" {
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
    if resume {
        args.push("--resume".into());
    }
    args
}

/// Reconstruct visible transcript lines from a resumed session's active path,
/// so /reload (and plain --resume) don't present a blank screen despite prior
/// history — the agent's own message list already has it, the TUI doesn't.
fn render_history(session: &Session) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut n = 0;
    for e in session.path() {
        match e.role.as_str() {
            "user" => {
                n += 1;
                lines.push(format!("{n}› {}", e.content));
            }
            "assistant" if !e.content.is_empty() => {
                lines.push(e.content.clone());
                lines.push(String::new());
            }
            _ => {} // system / tool / tool-call-only assistant entries: not shown
        }
    }
    (lines, n)
}

/// Entry point: spawns agent in background thread, renders TUI or plain stream.
pub fn run(cfg: TuiConfig) -> io::Result<()> {
    let TuiConfig { client, session, model, cli_args } = cfg;
    let (seed_lines, seed_msg_num) = render_history(&session);

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
                Ok(Job::Reload) => {
                    let _ = event_tx.send(ai_core::Event::ToolStart("cargo build --release".into()));
                    let manifest = format!("{MANIFEST_DIR}/Cargo.toml");
                    // own dir, not target/release: the user usually launched from there,
                    // and Windows won't let cargo overwrite that running exe
                    let target_dir = format!("{MANIFEST_DIR}/target/reload");
                    let out = std::process::Command::new("cargo")
                        .args(["build", "--release", "--manifest-path", &manifest, "--target-dir", &target_dir])
                        .output();
                    let fail = |event_tx: &mpsc::Sender<ai_core::Event>, detail: String| {
                        let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: false });
                        let _ = event_tx.send(ai_core::Event::Text(format!("  ✗ reload failed:\n{detail}")));
                        let _ = event_tx.send(ai_core::Event::TaskEnd { ok: true, error: None });
                    };
                    match out {
                        Ok(o) if o.status.success() => match stage_reload_exe(&target_dir) {
                            Ok(exe) => {
                                let _ = event_tx.send(ai_core::Event::ToolEnd { summary: "cargo build --release".into(), ok: true });
                                // a turn that failed before its first save leaves entries only in
                                // memory; persist now so --resume finds them (and doesn't exit 1)
                                let resume = !session.is_empty() && session.save().is_ok();
                                let args = build_relaunch_args(&cli_args, &client.url, &client.key, &client.model, resume);
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
        let res = app::ui_loop(&job_tx, event_rx, model, &cancel, seed_lines, seed_msg_num);
        let _ = execute!(stdout(), cursor::Show);
        let _ = terminal::disable_raw_mode();
        // job_tx must drop before the join — the agent thread blocks in
        // job_rx.recv() until every Sender is gone, otherwise join() hangs.
        drop(job_tx);
        agent.0.take().map(|h| h.join());
        match res? {
            app::Exit::Quit => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Entry;

    #[test]
    fn render_history_reconstructs_transcript_skipping_system_and_tool_calls() {
        let mut s = Session::new("m".into());
        let sys = s.add(Entry::new("system", "sys prompt".into()), None);
        let u1 = s.add(Entry::new("user", "first task".into()), Some(sys));
        let a1 = s.add(Entry::new("assistant", String::new()), Some(u1)); // tool-call turn, no text
        let t1 = s.add(Entry::new("tool", "tool output".into()), Some(a1));
        s.add(Entry::new("assistant", "done".into()), Some(t1));

        let (lines, n) = render_history(&s);
        assert_eq!(n, 1);
        assert_eq!(lines, vec!["1› first task".to_string(), "done".to_string(), String::new()]);
    }

    #[test]
    fn relaunch_args_replace_model_and_dedupe_resume() {
        let cli: Vec<String> = ["--tui", "--url", "old", "--resume", "--tree"].iter().map(|s| s.to_string()).collect();
        let args = build_relaunch_args(&cli, "new-url", "new-key", "new-model", true);
        assert_eq!(args, vec!["--tui", "--url", "new-url", "--key", "new-key", "--model", "new-model", "--resume"]);

        let no_resume = build_relaunch_args(&["--tui".to_string()], "u", "k", "m", false);
        assert!(!no_resume.contains(&"--resume".to_string()));
    }
}
