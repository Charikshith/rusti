// Tools the agent can call. Each returns a string the LLM sees as the result.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MAX_RESULT: usize = 20_000; // cap tool output so the conversation stays small
const CMD_TIMEOUT_SECS: u64 = 120;
// ponytail: fixed skip list instead of .gitignore parsing; rg (when present) honours .gitignore itself
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", "dist", "build", "__pycache__", ".venv", "venv"];

/// offset = 1-based first line (0/1 = start), limit = max lines (0 = all).
/// Whole-file reads return raw content; ranged reads prefix line numbers.
pub fn read_file(path: &str, offset: usize, limit: usize) -> (bool, String) {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            if offset <= 1 && limit == 0 {
                return (true, truncate(&s, MAX_RESULT));
            }
            let start = offset.saturating_sub(1);
            let take = if limit == 0 { usize::MAX } else { limit };
            let mut out = String::new();
            for (i, l) in s.lines().skip(start).take(take).enumerate() {
                out.push_str(&format!("{}: {l}\n", start + i + 1));
            }
            if out.is_empty() {
                out = format!("(no lines at offset {offset}; file has {} lines)", s.lines().count());
            }
            (true, truncate(&out, MAX_RESULT))
        }
        Err(e) => (false, format!("error reading {path}: {e}")),
    }
}

// ---- undo ------------------------------------------------------------------
// ponytail: one level — the files the *last turn* touched. A stack of frames if
// multi-turn undo is ever wanted.

/// Before-images for the current turn, oldest first. None = the file did not exist.
static UNDO: Mutex<Vec<(String, Option<Vec<u8>>)>> = Mutex::new(Vec::new());

pub fn undo_begin_turn() {
    UNDO.lock().unwrap().clear();
}

/// Remember `path` as it is right now, once per turn (the first before-image wins).
fn snapshot(path: &str) {
    let mut u = UNDO.lock().unwrap();
    if u.iter().any(|(p, _)| p == path) {
        return;
    }
    let before = if Path::new(path).exists() {
        match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(_) => return, // unreadable: better no undo than deleting it on undo
        }
    } else {
        None
    };
    u.push((path.to_string(), before));
}

/// Put every file the last turn touched back; returns one line per file.
pub fn undo_turn() -> Vec<String> {
    let snaps = std::mem::take(&mut *UNDO.lock().unwrap());
    let mut out = Vec::new();
    for (path, before) in snaps.into_iter().rev() {
        let r = match &before {
            Some(b) => std::fs::write(&path, b).map(|_| format!("restored {path}")),
            None if Path::new(&path).exists() => std::fs::remove_file(&path).map(|_| format!("removed {path}")),
            None => continue,
        };
        out.push(r.unwrap_or_else(|e| format!("could not undo {path}: {e}")));
    }
    out
}

pub fn write_file(path: &str, content: &str) -> (bool, String) {
    if let Err(e) = guard(path) {
        return (false, e);
    }
    snapshot(path);
    match std::fs::write(path, content) {
        Ok(()) => (true, format!("wrote {} bytes to {path}", content.len())),
        Err(e) => (false, format!("error writing {path}: {e}")),
    }
}

/// timeout_secs = 0 -> CMD_TIMEOUT_SECS. The process is killed at the deadline.
pub fn run_command(cmd: &str, timeout_secs: u64) -> (bool, String) {
    use std::io::Read;
    let mut child = match shell(cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("failed to run command: {e}")),
    };
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || { let mut v = Vec::new(); let _ = so.read_to_end(&mut v); v });
    let err_t = std::thread::spawn(move || { let mut v = Vec::new(); let _ = se.read_to_end(&mut v); v });
    let secs = if timeout_secs == 0 { CMD_TIMEOUT_SECS } else { timeout_secs };
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // ponytail: reader threads are not joined — a grandchild (cmd /C spawns one)
                // may still hold the pipe; they exit when it does. Use a job object if it matters.
                return (false, format!("[timed out after {secs}s; process killed]"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return (false, format!("failed to wait for command: {e}")),
        }
    };
    let code = status.code().unwrap_or(-1);
    let mut s = format!("[exit {code}]\n");
    s.push_str(&String::from_utf8_lossy(&out_t.join().unwrap_or_default()));
    s.push_str(&String::from_utf8_lossy(&err_t.join().unwrap_or_default()));
    (code == 0, truncate(&s, MAX_RESULT))
}

pub fn edit_file(path: &str, old_text: &str, new_text: &str) -> (bool, String) {
    multi_edit(path, &[(old_text.to_string(), new_text.to_string())])
}

/// Apply every (old,new) pair in order, all-or-nothing. Each old must appear exactly once
/// in the content as it stands after the previous edits.
pub fn multi_edit(path: &str, edits: &[(String, String)]) -> (bool, String) {
    if edits.is_empty() {
        return (false, "edit failed: no edits given".into());
    }
    if let Err(e) = guard(path) {
        return (false, e);
    }
    snapshot(path);
    let mut content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (false, format!("error reading {path}: {e}")),
    };
    for (i, (old, new)) in edits.iter().enumerate() {
        let count = content.matches(old.as_str()).count();
        let which = if edits.len() > 1 { format!(" (edit {})", i + 1) } else { String::new() };
        if count == 0 {
            return (false, format!("edit failed{which}: old text not found in {path}; nothing written"));
        }
        if count > 1 {
            return (false, format!("edit failed{which}: old text appears {count} times in {path}; make it unique; nothing written"));
        }
        content = content.replacen(old.as_str(), new, 1);
    }
    match std::fs::write(path, &content) {
        Ok(()) => {
            let (o, n): (usize, usize) = edits.iter().fold((0, 0), |(o, n), (a, b)| (o + a.len(), n + b.len()));
            (true, format!("edited {path}: {} edit(s), {o} chars -> {n} chars", edits.len()))
        }
        Err(e) => (false, format!("error writing {path}: {e}")),
    }
}

fn have_rg() -> bool {
    static RG: OnceLock<bool> = OnceLock::new();
    *RG.get_or_init(|| {
        Command::new("rg").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
    })
}

/// Regex search via ripgrep when installed; plain substring search (std only) otherwise.
pub fn grep(pattern: &str, path: &str, glob: &str) -> (bool, String) {
    let path = if path.is_empty() { "." } else { path };
    if have_rg() {
        let mut c = Command::new("rg");
        c.args(["-n", "--no-heading", "--color", "never", "--max-columns", "300", "-e", pattern]);
        if !glob.is_empty() {
            c.args(["-g", glob]);
        }
        c.arg(path);
        return match c.output() {
            Ok(o) => match o.status.code().unwrap_or(-1) {
                0 => (true, truncate(&String::from_utf8_lossy(&o.stdout), MAX_RESULT)),
                1 => (true, "no matches".into()),
                _ => (false, format!("rg error: {}", String::from_utf8_lossy(&o.stderr).trim())),
            },
            Err(e) => (false, format!("rg failed: {e}")),
        };
    }
    let root = Path::new(path);
    let files: Vec<String> = if root.is_file() {
        vec![path.to_string()]
    } else {
        let mut v = Vec::new();
        walk(root, root, usize::MAX, 0, &mut v);
        v.into_iter().filter(|(_, d)| !d).map(|(p, _)| p).collect()
    };
    let mut out = String::new();
    'outer: for rel in files {
        if !glob.is_empty() && !glob_match(glob, &rel) {
            continue;
        }
        let Ok(s) = std::fs::read_to_string(root.join(&rel)) else { continue }; // skips binaries
        for (i, l) in s.lines().enumerate() {
            if l.contains(pattern) {
                out.push_str(&format!("{rel}:{}:{}\n", i + 1, l.chars().take(300).collect::<String>()));
                if out.len() > MAX_RESULT {
                    break 'outer;
                }
            }
        }
    }
    if out.is_empty() {
        (true, "no matches (substring search; install ripgrep for regex)".into())
    } else {
        (true, truncate(&out, MAX_RESULT))
    }
}

/// List files matching a glob (`*.rs`, `src/**/*.rs`). rg --files when installed, std walk otherwise.
pub fn glob(pattern: &str, path: &str) -> (bool, String) {
    let path = if path.is_empty() { "." } else { path };
    if have_rg() {
        let out = Command::new("rg").args(["--files", "-g", pattern, path]).output();
        return match out {
            Ok(o) if o.status.code() == Some(0) => (true, truncate(&String::from_utf8_lossy(&o.stdout).replace('\\', "/"), MAX_RESULT)),
            Ok(o) if o.status.code() == Some(1) => (true, "no files matched".into()),
            Ok(o) => (false, format!("rg error: {}", String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => (false, format!("rg failed: {e}")),
        };
    }
    let root = Path::new(path);
    let mut v = Vec::new();
    walk(root, root, usize::MAX, 0, &mut v);
    let out: String = v.into_iter().filter(|(p, d)| !d && glob_match(pattern, p)).map(|(p, _)| p + "\n").collect();
    if out.is_empty() { (true, "no files matched".into()) } else { (true, truncate(&out, MAX_RESULT)) }
}

/// Directory listing, depth 1 by default; directories end with '/'.
pub fn list_dir(path: &str, depth: usize) -> (bool, String) {
    let path = if path.is_empty() { "." } else { path };
    let root = Path::new(path);
    if !root.is_dir() {
        return (false, format!("not a directory: {path}"));
    }
    let mut v = Vec::new();
    walk(root, root, if depth == 0 { 1 } else { depth }, 0, &mut v);
    if v.is_empty() {
        return (true, "(empty)".into());
    }
    let out: String = v.into_iter().map(|(p, d)| if d { p + "/\n" } else { p + "\n" }).collect();
    (true, truncate(&out, MAX_RESULT))
}

/// Collect (relative path with '/', is_dir) under `dir`, sorted, skipping SKIP_DIRS.
fn walk(root: &Path, dir: &Path, max_depth: usize, depth: usize, out: &mut Vec<(String, bool)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let is_dir = p.is_dir();
        let name = e.file_name().to_string_lossy().into_owned();
        if is_dir && SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        out.push((rel, is_dir));
        if is_dir && depth + 1 < max_depth {
            walk(root, &p, max_depth, depth + 1, out);
        }
    }
}

/// gitignore-style: a pattern without '/' matches the file name at any depth;
/// with '/' it matches the whole relative path. Supports `*`, `?`, `**`.
/// ponytail: no `{a,b}` braces or `[..]` classes; backtracking `*` is fine for path segments.
pub fn glob_match(pat: &str, path: &str) -> bool {
    if !pat.contains('/') {
        return path.rsplit('/').next().map_or(false, |n| seg_match(pat, n));
    }
    let p: Vec<&str> = pat.trim_start_matches("./").split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    fn segs(p: &[&str], s: &[&str]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(&"**") => (0..=s.len()).any(|i| segs(&p[1..], &s[i..])),
            Some(seg) => !s.is_empty() && seg_match(seg, s[0]) && segs(&p[1..], &s[1..]),
        }
    }
    segs(&p, &s)
}

fn seg_match(pat: &str, s: &str) -> bool {
    fn m(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') => (0..=s.len()).any(|i| m(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && m(&p[1..], &s[1..]),
            Some(c) => !s.is_empty() && *c == s[0] && m(&p[1..], &s[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = s.chars().collect();
    m(&p, &s)
}

/// --yolo: no permission prompts and no project-root guard.
pub static YOLO: AtomicBool = AtomicBool::new(false);

/// Refuse writes outside the working directory (the project root). Resolves the
/// deepest existing ancestor so new files and `..` tricks are judged on the real path.
pub fn guard(path: &str) -> Result<(), String> {
    if YOLO.load(Ordering::Relaxed) {
        return Ok(());
    }
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    let root = ROOT.get_or_init(|| {
        std::env::current_dir().and_then(|d| d.canonicalize()).unwrap_or_default()
    });
    let mut existing = std::path::PathBuf::from(if path.is_empty() { "." } else { path });
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name().map(|n| n.to_owned()), existing.parent().map(|p| p.to_path_buf())) {
            (Some(n), Some(p)) => {
                rest.push(n);
                existing = if p.as_os_str().is_empty() { ".".into() } else { p };
            }
            _ => break,
        }
    }
    let mut full = existing.canonicalize().map_err(|e| format!("cannot resolve {path}: {e}"))?;
    for r in rest.iter().rev() {
        full.push(r);
    }
    if full.starts_with(root) {
        Ok(())
    } else {
        Err(format!("refused: {path} is outside the project root {} (run with --yolo to allow)", root.display()))
    }
}

pub fn delete_file(path: &str) -> (bool, String) {
    if let Err(e) = guard(path) {
        return (false, e);
    }
    snapshot(path);
    match std::fs::remove_file(path) {
        Ok(()) => (true, format!("deleted {path}")),
        Err(e) => (false, format!("error deleting {path}: {e}")),
    }
}

pub fn move_file(from: &str, to: &str) -> (bool, String) {
    if let Err(e) = guard(from).and_then(|_| guard(to)) {
        return (false, e);
    }
    if Path::new(to).exists() {
        return (false, format!("move failed: {to} already exists"));
    }
    snapshot(from);
    snapshot(to);
    match std::fs::rename(from, to) {
        Ok(()) => (true, format!("moved {from} -> {to}")),
        Err(e) => (false, format!("error moving {from} -> {to}: {e}")),
    }
}

// ---- background jobs -------------------------------------------------------
// ponytail: jobs outlive the agent process if not stopped; the system prompt tells
// the model to job_stop what it started. A Windows job object would auto-kill them.

struct Job {
    cmd: String,
    child: std::process::Child,
    out: Arc<Mutex<Vec<u8>>>,
}

static JOBS: Mutex<Vec<(u32, Job)>> = Mutex::new(Vec::new());
static NEXT_JOB: AtomicUsize = AtomicUsize::new(1);
const JOB_BUF: usize = 1 << 20; // keep the last 1 MB of output

fn shell(cmd: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    }
}

fn pump(mut r: impl std::io::Read + Send + 'static, buf: Arc<Mutex<Vec<u8>>>) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 { break; }
            let mut b = buf.lock().unwrap();
            b.extend_from_slice(&chunk[..n]);
            if b.len() > JOB_BUF {
                let cut = b.len() - JOB_BUF;
                b.drain(..cut);
            }
        }
    });
}

/// Start a long-running command (server, watcher); returns a job id for job_output/job_stop.
pub fn run_background(cmd: &str) -> (bool, String) {
    let mut child = match shell(cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("failed to start: {e}")),
    };
    let out = Arc::new(Mutex::new(Vec::new()));
    pump(child.stdout.take().unwrap(), out.clone());
    pump(child.stderr.take().unwrap(), out.clone());
    let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed) as u32;
    JOBS.lock().unwrap().push((id, Job { cmd: cmd.to_string(), child, out }));
    (true, format!("job {id} started: {cmd}"))
}

/// Output so far plus status. id 0 lists all jobs.
pub fn job_output(id: u32) -> (bool, String) {
    let mut jobs = JOBS.lock().unwrap();
    if id == 0 {
        if jobs.is_empty() {
            return (true, "no background jobs".into());
        }
        let s: String = jobs.iter_mut().map(|(i, j)| format!("job {i} [{}]: {}\n", status(&mut j.child), j.cmd)).collect();
        return (true, s);
    }
    match jobs.iter_mut().find(|(i, _)| *i == id) {
        Some((_, j)) => {
            let st = status(&mut j.child);
            let out = String::from_utf8_lossy(&j.out.lock().unwrap()).into_owned();
            let tail = if out.len() > MAX_RESULT { format!("…\n{}", &out[out.len() - MAX_RESULT..]) } else { out };
            (true, format!("[job {id} {st}]\n{tail}"))
        }
        None => (false, format!("no such job: {id}")),
    }
}

pub fn job_stop(id: u32) -> (bool, String) {
    let mut jobs = JOBS.lock().unwrap();
    let Some(pos) = jobs.iter().position(|(i, _)| *i == id) else {
        return (false, format!("no such job: {id}"));
    };
    let (_, mut j) = jobs.remove(pos);
    let was = status(&mut j.child);
    if cfg!(windows) {
        // cmd /C wraps the real process; taskkill /T takes the whole tree down
        let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &j.child.id().to_string()])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = j.child.kill();
    let _ = j.child.wait();
    (true, format!("job {id} stopped (was {was})"))
}

fn status(child: &mut std::process::Child) -> String {
    match child.try_wait() {
        Ok(Some(st)) => format!("exited {}", st.code().unwrap_or(-1)),
        Ok(None) => "running".into(),
        Err(e) => format!("unknown: {e}"),
    }
}

// ---- todo list ----------------------------------------------------------------

static TODOS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Replace the whole list; returns it rendered. status: pending | in_progress | done.
pub fn todo(items: Vec<(String, String)>) -> (bool, String) {
    let mut t = TODOS.lock().unwrap();
    *t = items;
    if t.is_empty() {
        return (true, "todo list cleared".into());
    }
    let s: String = t.iter().map(|(text, st)| {
        let mark = match st.as_str() { "done" => "☑", "in_progress" => "◐", _ => "☐" };
        format!("{mark} {text}\n")
    }).collect();
    (true, s)
}

pub async fn ask_user(question: &str) -> (bool, String) {
    use std::io::Write;
    // Front-end mode: send the question to the TUI and wait for its answer.
    if let Some(tx) = crate::ai_core::SINK.get() {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if tx
            .send(crate::ai_core::Event::Ask { question: question.to_string(), reply: reply_tx })
            .is_ok()
        {
            return (true, reply_rx.await.unwrap_or_else(|_| "no answer given".into()));
        }
    }
    eprint!("{question} > ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => {
            let a = line.trim().to_string();
            if a.is_empty() { (true, "no answer given".into()) } else { (true, a) }
        }
        Err(e) => (false, format!("error reading input: {e}")),
    }
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut cut = n;
        while !s.is_char_boundary(cut) { cut -= 1; }
        format!("{}\n…[truncated]", &s[..cut])
    }
}
