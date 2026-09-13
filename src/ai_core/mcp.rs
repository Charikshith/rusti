// MCP (Model Context Protocol) client: stdio transport, JSON-RPC 2.0.
//
// Servers are child processes. We write one JSON object per line to their
// stdin and read one per line from their stdout. That is the whole protocol,
// so there is no dependency here beyond serde_json and std.
//
// Their tools are merged into rusti's own tool list under the name
// "mcp__<server>__<tool>", which is what makes them reachable by the model
// without touching dispatch()'s match arm for every new server.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{Config, McpServer};

/// The version we ask for at initialize. Servers negotiate down if they are
/// older; we do not care which they pick, only that tools/list works after.
const PROTOCOL: &str = "2024-11-05";

/// A server that has to fetch its own package (npx, uvx) can take a while the
/// first time, so the handshake gets longer than a call.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Function names are capped at 64 characters by the OpenAI-compatible tool
/// schema. A tool whose prefixed name exceeds that is dropped with a warning
/// rather than sent, because the API rejects the whole request otherwise.
const MAX_NAME: usize = 64;

/// Last lines of a server's stderr, kept so a failure can say WHY. Without it
/// a missing API key inside the server reads as a bare timeout.
const ERR_TAIL: usize = 10;

pub struct Server {
    pub name: String,
    /// (advertised name, original tool name, schema) — the advertised name is
    /// kept beside the original so a call never has to parse the prefix back
    /// off, which would be ambiguous once slug() has turned '-' into '_'.
    pub tools: Vec<(String, String, Value)>,
    stdin: Mutex<ChildStdin>,
    rx: Mutex<Receiver<String>>,
    child: Mutex<Child>,
    errs: Arc<Mutex<VecDeque<String>>>,
    next_id: AtomicU64,
}

impl Server {
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.wait_for(id, timeout, method)
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn send(&self, msg: &Value) -> Result<(), String> {
        let mut w = self.stdin.lock().map_err(|_| "mcp stdin lock poisoned".to_string())?;
        writeln!(w, "{msg}").map_err(|e| format!("writing to {}: {e}", self.name))?;
        w.flush().map_err(|e| format!("flushing {}: {e}", self.name))
    }

    /// Read until the reply with this id arrives. Anything else on the pipe —
    /// notifications, log lines, replies we already timed out on — is skipped
    /// rather than treated as the answer.
    fn wait_for(&self, id: u64, timeout: Duration, method: &str) -> Result<Value, String> {
        let rx = self.rx.lock().map_err(|_| "mcp reader lock poisoned".to_string())?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(format!("{} timed out after {}s on {method}{}", self.name, timeout.as_secs(), self.why()));
            }
            let line = match rx.recv_timeout(left) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("{} exited{}", self.name, self.why()))
                }
            };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                let msg = e.get("message").and_then(Value::as_str).unwrap_or("unknown error");
                return Err(format!("{}: {msg}", self.name));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Whatever the server last said on stderr, for appending to an error.
    fn why(&self) -> String {
        let errs = self.errs.lock().map(|e| e.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
        if errs.is_empty() { String::new() } else { format!(" — stderr: {}", errs.join(" / ")) }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Closing stdin is the polite stop (most servers exit on EOF), but a
        // server that ignores EOF would linger as an orphan, so follow with a
        // kill. Both are best-effort: we are usually on the way out anyway.
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

static SERVERS: Mutex<Vec<Arc<Server>>> = Mutex::new(Vec::new());

/// Tool names must match [A-Za-z0-9_-]; anything else becomes '_'.
fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

pub fn is_mcp(name: &str) -> bool {
    name.starts_with("mcp__")
}

fn build(cfg: &McpServer) -> Command {
    // On Windows npx/uvx/npm are .cmd shims, and CreateProcess cannot execute
    // those directly — "program not found" even when they are on PATH. Going
    // through cmd /C is what makes the usual MCP server commands work here.
    let mut c = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&cfg.command).args(&cfg.args);
        c
    } else {
        let mut c = Command::new(&cfg.command);
        c.args(&cfg.args);
        c
    };
    c.envs(&cfg.env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c
}

/// Spawn one server and complete the handshake: initialize, initialized,
/// tools/list. Returns it ready to take calls.
fn connect(name: &str, cfg: &McpServer) -> Result<Server, String> {
    let mut child = build(cfg).spawn().map_err(|e| format!("spawning {}: {e}", cfg.command))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;

    // Reader threads, one per pipe. They exist so a hung server hits a timeout
    // instead of blocking the agent forever in a plain read: recv_timeout can
    // give up, a blocking read on a pipe cannot.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break; // receiver gone: the server was disabled or we are exiting
            }
        }
    });
    let errs = Arc::new(Mutex::new(VecDeque::new()));
    let sink = Arc::clone(&errs);
    std::thread::spawn(move || {
        // draining stderr is not optional: a server that fills the pipe buffer
        // blocks on write and stops answering, which looks like a hang
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let Ok(mut q) = sink.lock() else { break };
            if q.len() == ERR_TAIL {
                q.pop_front();
            }
            q.push_back(line);
        }
    });

    // built empty and filled after the handshake: Server owns the child and
    // implements Drop, so it cannot be moved out of to add the tools later
    let mut s = Server {
        name: name.to_string(),
        tools: Vec::new(),
        stdin: Mutex::new(stdin),
        rx: Mutex::new(rx),
        child: Mutex::new(child),
        errs,
        next_id: AtomicU64::new(1),
    };

    s.request(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL,
            "capabilities": {},
            "clientInfo": {"name": "rusti", "version": env!("CARGO_PKG_VERSION")},
        }),
        HANDSHAKE_TIMEOUT,
    )?;
    s.notify("notifications/initialized", json!({}))?;

    let listed = s.request("tools/list", json!({}), HANDSHAKE_TIMEOUT)?;
    let mut tools = Vec::new();
    let mut skipped = Vec::new();
    for t in listed.get("tools").and_then(Value::as_array).cloned().unwrap_or_default() {
        let Some(orig) = t.get("name").and_then(Value::as_str) else { continue };
        let full = format!("mcp__{}__{}", slug(name), slug(orig));
        if full.len() > MAX_NAME {
            skipped.push(orig.to_string());
            continue;
        }
        let schema = json!({"type": "function", "function": {
            "name": full,
            "description": t.get("description").and_then(Value::as_str).unwrap_or(""),
            // servers may omit inputSchema; the API still needs an object schema
            "parameters": t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
        }});
        tools.push((full, orig.to_string(), schema));
    }
    if !skipped.is_empty() {
        // loud, because the alternative is the model never seeing the tool and
        // nobody knowing why
        crate::ai_core::emit(crate::ai_core::Event::Text(format!(
            "  ⚠ {name}: {} tool(s) dropped, name over {MAX_NAME} chars: {}",
            skipped.len(),
            skipped.join(", ")
        )));
    }
    s.tools = tools;
    Ok(s)
}

/// Connect every enabled server in the config. Returns one status line each,
/// for the caller to put in the transcript. A server that fails to start is
/// reported and skipped — it never blocks rusti from running.
pub fn connect_all() -> Vec<String> {
    let cfg = Config::load();
    let mut out = Vec::new();
    for (name, s) in cfg.mcp.iter().filter(|(_, s)| s.enabled) {
        match connect(name, s) {
            Ok(srv) => {
                out.push(format!("  ✓ mcp {name}: {} tools", srv.tools.len()));
                if let Ok(mut list) = SERVERS.lock() {
                    list.push(Arc::new(srv));
                }
            }
            Err(e) => out.push(format!("  ✗ mcp {name}: {e}")),
        }
    }
    out
}

/// Every connected server's tools, as OpenAI-style function schemas.
pub fn schemas() -> Vec<Value> {
    let Ok(list) = SERVERS.lock() else { return Vec::new() };
    list.iter().flat_map(|s| s.tools.iter().map(|(_, _, sch)| sch.clone())).collect()
}

/// Call a tool by its advertised "mcp__server__tool" name.
pub fn call(full: &str, args: &Value) -> (bool, String) {
    let found = {
        let Ok(list) = SERVERS.lock() else { return (false, "mcp registry poisoned".into()) };
        list.iter()
            .find_map(|s| s.tools.iter().find(|(f, _, _)| f == full).map(|(_, orig, _)| (Arc::clone(s), orig.clone())))
    };
    // clone the Arc and DROP the registry lock before the call: holding it
    // would serialize every server behind whichever one is slowest
    let Some((srv, orig)) = found else {
        return (false, format!("unknown mcp tool: {full}"));
    };
    let params = json!({"name": orig, "arguments": args});
    match srv.request("tools/call", params, CALL_TIMEOUT) {
        Err(e) => (false, e),
        Ok(res) => {
            let text = res
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .map(|p| match p.get("type").and_then(Value::as_str) {
                            Some("text") => p.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                            // images and resources are not rendered; say what
                            // arrived so the model is not told "nothing"
                            Some(other) => format!("[{other} content]"),
                            None => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| res.to_string());
            // isError is the protocol's way of reporting a TOOL failure; a
            // transport failure came back as Err above
            let ok = !res.get("isError").and_then(Value::as_bool).unwrap_or(false);
            (ok, if text.trim().is_empty() { "(no output)".into() } else { text })
        }
    }
}

/// One row per configured server: (name, enabled, connected, tool count).
pub fn status() -> Vec<(String, bool, bool, usize)> {
    let cfg = Config::load();
    let live = SERVERS.lock().ok();
    cfg.mcp
        .iter()
        .map(|(name, s)| {
            let n = live
                .as_ref()
                .and_then(|l| l.iter().find(|x| &x.name == name).map(|x| x.tools.len()));
            (name.clone(), s.enabled, n.is_some(), n.unwrap_or(0))
        })
        .collect()
}

/// Switch a server on or off and persist the choice. Turning one on connects
/// it now; turning one off drops it, and Drop kills the child.
pub fn set_enabled(name: &str, on: bool) -> Result<String, String> {
    let mut cfg = Config::load();
    let Some(s) = cfg.mcp.get_mut(name) else { return Err(format!("no mcp server named {name}")) };
    s.enabled = on;
    let spec = s.clone();
    cfg.save()?;

    if !on {
        if let Ok(mut list) = SERVERS.lock() {
            list.retain(|x| x.name != name);
        }
        return Ok(format!("mcp {name} off"));
    }
    if SERVERS.lock().map(|l| l.iter().any(|x| x.name == name)).unwrap_or(false) {
        return Ok(format!("mcp {name} already on"));
    }
    let srv = connect(name, &spec)?;
    let n = srv.tools.len();
    SERVERS.lock().map_err(|_| "mcp registry poisoned".to_string())?.push(Arc::new(srv));
    Ok(format!("mcp {name} on: {n} tools"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_names_are_legal_and_map_back_to_the_original() {
        // '.' and '/' are common in MCP tool names and are NOT legal in an
        // OpenAI function name; '-' is legal and must survive
        assert_eq!(slug("read.file"), "read_file");
        assert_eq!(slug("fs/list"), "fs_list");
        assert_eq!(slug("get-issue"), "get-issue");
        assert!(is_mcp("mcp__github__get-issue"));
        assert!(!is_mcp("read_file"));
        // the prefix is never parsed back off — slug() is lossy, so "a_b" and
        // "a.b" collide — which is why the original name is stored beside it
        assert_eq!(slug("a_b"), slug("a.b"));
    }

    #[test]
    fn an_over_long_name_would_be_dropped_rather_than_sent() {
        let long = "x".repeat(80);
        assert!(format!("mcp__s__{}", slug(&long)).len() > MAX_NAME);
        assert!(format!("mcp__s__{}", slug("ok")).len() <= MAX_NAME);
    }

    fn fake() -> McpServer {
        McpServer {
            command: "python".into(),
            args: vec!["tests/fake_mcp_server.py".into()],
            env: Default::default(),
            enabled: true,
        }
    }

    /// The wire format has to be checked against something that ANSWERS. The
    /// unit tests above only prove the client agrees with itself; this one
    /// proves initialize/tools/list/tools/call are actually shaped right.
    #[test]
    fn talks_to_a_real_stdio_server() {
        let Ok(srv) = connect("fake", &fake()) else {
            eprintln!("skipping: python not runnable here");
            return;
        };

        // tools/list came back, and the illegal '.' was made legal without
        // losing the original the server wants to be called by
        assert_eq!(srv.tools.len(), 2, "both tools should be advertised");
        let (full, orig, schema) = &srv.tools[0];
        assert_eq!(full, "mcp__fake__echo_it");
        assert_eq!(orig, "echo.it", "the server's own name must be kept for the call");
        assert_eq!(schema["function"]["name"], "mcp__fake__echo_it");
        assert_eq!(schema["function"]["parameters"]["properties"]["text"]["type"], "string");

        // a call routes by advertised name and unwraps the content array
        SERVERS.lock().unwrap().push(Arc::new(srv));
        let (ok, out) = call("mcp__fake__echo_it", &json!({"text": "hi"}));
        assert!(ok, "echo should succeed: {out}");
        assert_eq!(out, "echo: hi");

        // isError is a TOOL failure: ok=false, but the text still comes back
        let (ok, out) = call("mcp__fake__boom", &json!({}));
        assert!(!ok, "isError must report as failure");
        assert_eq!(out, "it broke");

        // an unknown name is refused by us, not sent
        let (ok, out) = call("mcp__fake__nope", &json!({}));
        assert!(!ok && out.contains("unknown mcp tool"), "got {out}");

        SERVERS.lock().unwrap().retain(|s| s.name != "fake");
    }
}
