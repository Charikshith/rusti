// E1 hooks: your own commands, run at fixed points of a turn. Each gets the
// event as one JSON object on stdin and answers with one JSON object on
// stdout; nothing, or {}, means carry on. Out of process on purpose: any
// language, no plugin API to version, and a crash is a failed hook rather
// than a dead agent. Configured in config.json "hooks" (a project's
// model.json only once the folder is trusted), run in order, each seeing
// what the ones before it rewrote.
//
//   event          stdin adds                    a reply may set
//   user_shell     command, to_model             command (run this instead), output + exit (handled), block
//   pre_tool       tool, args                    args (rewrite), block. Runs after the permission gate,
//                                                so it can refuse what you allowed, never allow what you refused
//   post_tool      tool, args, ok, output        output (redact), ok
//   user_prompt    prompt                        prompt (rewrite), block
//   session_start  session, resumed              context (added to the first message)
//   pre_compact    session, messages             summary (used instead of asking the model), block (skip it)
//
// Every event also carries "event" and "cwd". A non-zero exit, a timeout or
// a reply that is not a JSON object is a failure: it blocks where the event
// can block, withholds post_tool's output (a redaction that failed must not
// leak what it was meant to hide), and is only reported for session_start.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::tools;
use crate::config::Hook;

pub const EVENTS: [&str; 6] = ["user_shell", "pre_tool", "post_tool", "user_prompt", "session_start", "pre_compact"];

/// Set once at startup from the config, like the shell: a test or a sub-run
/// that never sets it runs no hooks.
static HOOKS: OnceLock<BTreeMap<String, Vec<Hook>>> = OnceLock::new();

pub fn set(hooks: BTreeMap<String, Vec<Hook>>) {
    let _ = HOOKS.set(hooks);
}

/// A startup row per event name rusti does not have: a typo there would
/// otherwise be a hook that silently never runs.
pub fn check() -> Vec<String> {
    let names = HOOKS.get().map(|h| h.keys().collect::<Vec<_>>()).unwrap_or_default();
    names
        .into_iter()
        .filter(|k| !EVENTS.contains(&k.as_str()))
        .map(|k| format!("  ⚠ hooks: no event called {k} (events: {})", EVENTS.join(", ")))
        .collect()
}

/// The configured hooks for `event`; the helpers below take them as an
/// argument so a test can hand them its own.
pub fn of(event: &str) -> &'static [Hook] {
    HOOKS.get().and_then(|h| h.get(event)).map_or(&[], Vec::as_slice)
}

/// Run one hook: the event in on stdin, its reply object out.
fn call(h: &Hook, input: &Value) -> Result<Map<String, Value>, String> {
    let mut child = tools::own_group(&mut tools::shell(&h.command))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("did not start: {e}"))?;
    let tree = tools::Tree::of(&child);
    // a writer thread: a hook that never reads stdin must not block us on a full pipe
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let data = input.to_string();
    std::thread::spawn(move || {
        let _ = stdin.write_all(data.as_bytes()); // dropped here: EOF for the hook
    });
    let quiet: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(|_: &str| {});
    let out = tools::read_live(child.stdout.take().expect("stdout is piped"), quiet.clone());
    let err = tools::read_live(child.stderr.take().expect("stderr is piped"), quiet);
    let deadline = Instant::now() + Duration::from_secs(h.timeout.max(1));
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if Instant::now() >= deadline => {
                tree.kill(&mut child);
                return Err(format!("timed out after {}s", h.timeout.max(1)));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(format!("could not be waited on: {e}")),
        }
    };
    // a child the hook left running holds the pipes; its reply is already read
    let ([out, err], held) = tools::drain([out, err]);
    tree.done(held);
    let (out, err) = (String::from_utf8_lossy(&out).into_owned(), String::from_utf8_lossy(&err).into_owned());
    if !status.success() {
        let why = err.lines().rev().find(|l| !l.trim().is_empty()).map(|l| format!(": {}", l.trim())).unwrap_or_default();
        return Err(format!("exited {}{why}", status.code().unwrap_or(-1)));
    }
    match out.trim() {
        "" => Ok(Map::new()),
        s => match serde_json::from_str::<Value>(s) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(format!("replied with something other than a JSON object: {}", super::tools::truncate(s, 200))),
        },
    }
}

/// Every hook for `event`, in order, on `input`. Each reply's fields are
/// merged in, so the next hook sees the rewrite; a `block` stops the chain
/// with its reason, and so does any failure. A user_shell reply carrying
/// `output` handled the command, so the hooks after it do not run.
pub fn run_with(hooks: &[Hook], event: &str, input: Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut v = input;
    if hooks.is_empty() {
        return Ok(v);
    }
    v.insert("event".into(), event.into());
    v.insert("cwd".into(), std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default().into());
    for h in hooks {
        let reply = call(h, &Value::Object(v.clone())).map_err(|e| format!("{event} hook `{}` {e}", h.command))?;
        match reply.get("block") {
            Some(Value::String(why)) => return Err(why.clone()),
            Some(Value::Bool(true)) => return Err(format!("blocked by {event} hook `{}`", h.command)),
            _ => {}
        }
        let handled = event == "user_shell" && reply.contains_key("output");
        for (k, x) in reply {
            if k != "event" && k != "block" {
                v.insert(k, x);
            }
        }
        if handled {
            break;
        }
    }
    Ok(v)
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    v.map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
}

/// What to do with a `!` command once its hooks have answered.
#[derive(Debug, PartialEq)]
pub enum Shell {
    /// Run this, which may not be what was typed.
    Run(String),
    /// A hook ran it itself; this is the result.
    Done { exit: i64, output: String },
}

/// F09: `!cmd` / `!!cmd` through the user_shell hooks. Err is the reason it was refused.
pub fn user_shell(hooks: &[Hook], cmd: &str, to_model: bool) -> Result<Shell, String> {
    let r = run_with(hooks, "user_shell", obj(json!({"command": cmd, "to_model": to_model})))?;
    if let Some(output) = text(r.get("output")) {
        return Ok(Shell::Done { exit: r.get("exit").and_then(Value::as_i64).unwrap_or(0), output });
    }
    Ok(Shell::Run(r.get("command").and_then(Value::as_str).unwrap_or(cmd).to_string()))
}

/// The arguments the tool should run with, or why it must not run.
pub fn pre_tool(hooks: &[Hook], tool: &str, args: &Value) -> Result<Value, String> {
    let r = run_with(hooks, "pre_tool", obj(json!({"tool": tool, "args": args})))?;
    Ok(r.get("args").cloned().unwrap_or_else(|| args.clone()))
}

/// The result the model sees. A failed hook withholds the output.
pub fn post_tool(hooks: &[Hook], tool: &str, args: &Value, ok: bool, output: String) -> (bool, String) {
    if hooks.is_empty() {
        return (ok, output);
    }
    match run_with(hooks, "post_tool", obj(json!({"tool": tool, "args": args, "ok": ok, "output": output}))) {
        Ok(r) => (r.get("ok").and_then(Value::as_bool).unwrap_or(ok), text(r.get("output")).unwrap_or_default()),
        Err(e) => (false, format!("[output withheld: {e}]")),
    }
}

/// The message as the hooks left it, or why it must not be sent.
pub fn user_prompt(hooks: &[Hook], prompt: &str) -> Result<String, String> {
    let r = run_with(hooks, "user_prompt", obj(json!({"prompt": prompt})))?;
    Ok(text(r.get("prompt")).unwrap_or_else(|| prompt.to_string()))
}

/// Extra context for the session's first message this run, if a hook gave any.
pub fn session_start(hooks: &[Hook], session: &str, resumed: bool) -> Result<Option<String>, String> {
    let r = run_with(hooks, "session_start", obj(json!({"session": session, "resumed": resumed})))?;
    Ok(text(r.get("context")).filter(|c| !c.trim().is_empty()))
}

/// A summary to use instead of asking the model (Some), carry on (None), or
/// skip this compaction (Err, with the reason).
pub fn pre_compact(hooks: &[Hook], session: &str, messages: &[Value]) -> Result<Option<String>, String> {
    let r = run_with(hooks, "pre_compact", obj(json!({"session": session, "messages": messages})))?;
    Ok(text(r.get("summary")).filter(|s| !s.trim().is_empty()))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A hook command, portable between the shells rusti may run in.
    pub(in crate::ai_core) fn reply(dir: &std::path::Path, name: &str, json: &str) -> Hook {
        let f = dir.join(name);
        std::fs::write(&f, json).unwrap();
        let f = f.to_string_lossy().replace('\\', "/");
        let cmd = if cmd_shell() { format!("type \"{}\"", f.replace('/', "\\")) } else { format!("cat '{f}'") };
        Hook { command: cmd, timeout: 10 }
    }
    fn cmd_shell() -> bool {
        super::super::tools::current_shell().flag == "/C"
    }
    fn hook(command: &str) -> Hook {
        Hook { command: command.into(), timeout: 10 }
    }
    /// Echoes stdin back: proves what the hook was given.
    fn echo() -> Hook {
        hook(if cmd_shell() { "findstr \"^\"" } else { "cat" })
    }

    #[test]
    fn hook_decisions_chain_block_and_fail_safe() {
        let dir = std::env::temp_dir().join(format!("rusti_hooks_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let none: &[Hook] = &[];
        assert_eq!(user_shell(none, "ls", true), Ok(Shell::Run("ls".into())), "no hooks: as typed");

        // a rewrite reaches the next hook, which echoes it back unchanged
        let ssh = reply(&dir, "ssh.json", r#"{"command":"ssh box 'cd /srv && cargo test'"}"#);
        assert_eq!(user_shell(&[ssh.clone(), echo()], "cargo test", true), Ok(Shell::Run("ssh box 'cd /srv && cargo test'".into())));
        let seen = run_with(&[echo()], "user_shell", obj(json!({"command": "x", "to_model": false}))).unwrap();
        assert_eq!((seen["event"].as_str(), seen["to_model"].as_bool()), (Some("user_shell"), Some(false)), "the event arrives on stdin");
        assert!(seen["cwd"].as_str().is_some_and(|c| !c.is_empty()));

        // handled: the result is the hook's, and the hooks after it do not run
        let done = reply(&dir, "done.json", r#"{"output":"ran remotely","exit":3}"#);
        assert_eq!(user_shell(&[done, ssh.clone()], "make", true), Ok(Shell::Done { exit: 3, output: "ran remotely".into() }));

        // block, empty reply, failures
        let block = reply(&dir, "block.json", r#"{"block":"no rm here"}"#);
        assert_eq!(user_shell(&[block.clone(), ssh.clone()], "rm -rf x", true), Err("no rm here".into()));
        assert_eq!(user_shell(&[reply(&dir, "empty.json", "  \n")], "ls", true), Ok(Shell::Run("ls".into())));
        let exit = user_shell(&[hook("exit 3")], "ls", true).unwrap_err();
        assert!(exit.starts_with("user_shell hook `exit 3` exited 3"), "a failing hook blocks: {exit}");
        let junk = user_shell(&[reply(&dir, "junk.json", "[1,2]")], "ls", true).unwrap_err();
        assert!(junk.contains("something other than a JSON object"), "{junk}");
        let slow = if cmd_shell() { "ping -n 6 127.0.0.1 >nul" } else { "sleep 5" };
        let t0 = Instant::now();
        let late = user_shell(&[Hook { command: slow.into(), timeout: 1 }], "ls", true).unwrap_err();
        assert!(late.contains("timed out after 1s") && t0.elapsed() < Duration::from_secs(4), "{late}");

        // pre_tool rewrites args or refuses; post_tool redacts, and a failed redaction withholds
        let args = json!({"command": "rm -rf /"});
        assert_eq!(pre_tool(&[], "run_command", &args), Ok(args.clone()));
        let safer = reply(&dir, "safer.json", r#"{"args":{"command":"echo no"}}"#);
        assert_eq!(pre_tool(&[safer], "run_command", &args), Ok(json!({"command": "echo no"})));
        assert_eq!(pre_tool(&[block], "run_command", &args), Err("no rm here".into()));
        let redact = reply(&dir, "redact.json", r#"{"output":"[redacted]"}"#);
        assert_eq!(post_tool(&[redact], "read_file", &args, true, "API_KEY=sk-123".into()), (true, "[redacted]".into()));
        let (ok, out) = post_tool(&[hook("exit 1")], "read_file", &args, true, "API_KEY=sk-123".into());
        assert!(!ok && out.starts_with("[output withheld: post_tool hook") && !out.contains("sk-123"), "{out}");

        // user_prompt, session_start, pre_compact
        let polite = reply(&dir, "polite.json", r#"{"prompt":"please fix it"}"#);
        assert_eq!(user_prompt(&[polite], "fix it"), Ok("please fix it".into()));
        assert_eq!(user_prompt(&[echo()], "fix it"), Ok("fix it".into()));
        let ctx = reply(&dir, "ctx.json", r#"{"context":"on-call: alice"}"#);
        assert_eq!(session_start(&[ctx], "s-1", false), Ok(Some("on-call: alice".into())));
        assert_eq!(session_start(&[echo()], "s-1", true), Ok(None));
        let sum = reply(&dir, "sum.json", r#"{"summary":"we fixed the build"}"#);
        assert_eq!(pre_compact(&[sum], "s-1", &[json!({"role": "user", "content": "hi"})]), Ok(Some("we fixed the build".into())));
        assert_eq!(pre_compact(&[reply(&dir, "skip.json", r#"{"block":"not now"}"#)], "s-1", &[]), Err("not now".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hook that leaves a child running (a notifier fired with `&`) is done
    /// when its shell exits. Waiting for that child to close the pipes ignored
    /// the hook's own timeout.
    #[test]
    fn a_hook_that_backgrounds_a_child_answers_at_once() {
        if cmd_shell() {
            return;
        }
        let t0 = Instant::now();
        let r = user_shell(&[hook(r#"sleep 6 & echo '{"command":"ls -a"}'"#)], "ls", true);
        assert_eq!(r, Ok(Shell::Run("ls -a".into())), "the reply it wrote before exiting is read");
        assert!(t0.elapsed() < Duration::from_secs(3), "returned after {:?}", t0.elapsed());
    }
}
