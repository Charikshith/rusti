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
    /// Config's `defer`: tools stay out of requests until tool_search loads them.
    pub defer: bool,
    stdin: Mutex<ChildStdin>,
    rx: Mutex<Receiver<String>>,
    child: Mutex<Child>,
    errs: Arc<Mutex<VecDeque<String>>>,
    next_id: AtomicU64,
}

impl Server {
    /// The reader lock is held from send to reply, so two parallel calls to
    /// one server take turns; otherwise one could read and skip the other's
    /// reply. Different servers still overlap.
    fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let rx = self.rx.lock().map_err(|_| "mcp reader lock poisoned".to_string())?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.wait_for(&rx, id, timeout, method)
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
    fn wait_for(&self, rx: &Receiver<String>, id: u64, timeout: Duration, method: &str) -> Result<Value, String> {
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

pub(crate) static SERVERS: Mutex<Vec<Arc<Server>>> = Mutex::new(Vec::new());

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
        defer: cfg.defer,
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
                let deferred = if srv.defer { "deferred " } else { "" };
                out.push(format!("  ✓ mcp {name}: {} {deferred}tools", srv.tools.len()));
                if let Ok(mut list) = SERVERS.lock() {
                    list.push(Arc::new(srv));
                }
            }
            Err(e) => out.push(format!("  ✗ mcp {name}: {e}")),
        }
    }
    out
}

/// Every connected server's tools, as OpenAI-style function schemas, except
/// a deferred server's: those wait for tool_search.
pub fn schemas() -> Vec<Value> {
    let Ok(list) = SERVERS.lock() else { return Vec::new() };
    list.iter().filter(|s| !s.defer).flat_map(|s| s.tools.iter().map(|(_, _, sch)| sch.clone())).collect()
}

/// Deferred servers' tools as (server, schema), in registry order.
pub fn deferred() -> Vec<(String, Value)> {
    let Ok(list) = SERVERS.lock() else { return Vec::new() };
    list.iter()
        .filter(|s| s.defer)
        .flat_map(|s| s.tools.iter().map(|(_, _, sch)| (s.name.clone(), sch.clone())))
        .collect()
}

/// Most tools one tool_search loads; the default is fewer.
pub const SEARCH_MAX: usize = 8;

/// tool_search: rank the deferred tools not yet loaded against `query` and
/// load the best `limit`. Returns the result text and the names loaded.
pub fn search(query: &str, limit: usize, loaded: &[String]) -> (String, Vec<String>) {
    let pool: Vec<(String, Value)> = deferred()
        .into_iter()
        .filter(|(_, s)| !loaded.iter().any(|n| s["function"]["name"] == n.as_str()))
        .collect();
    let docs: Vec<String> = pool.iter().map(|(srv, s)| search_text(srv, s)).collect();
    let hits: Vec<&Value> = bm25(query, &docs).into_iter().take(limit.clamp(1, SEARCH_MAX)).map(|(i, _)| &pool[i].1).collect();
    if hits.is_empty() {
        return (format!("No deferred tool matches \"{query}\" ({} not loaded yet). Try other words.", pool.len()), vec![]);
    }
    let names: Vec<String> = hits.iter().map(|s| s["function"]["name"].as_str().unwrap_or("").to_string()).collect();
    let rows: Vec<String> = hits
        .iter()
        .map(|s| {
            let desc = s["function"]["description"].as_str().unwrap_or("").lines().next().unwrap_or("");
            format!("- {} — {desc}", s["function"]["name"].as_str().unwrap_or(""))
        })
        .collect();
    let n = names.len();
    (format!("Loaded {n} tool{}, available from your next call:\n{}", if n == 1 { "" } else { "s" }, rows.join("\n")), names)
}

/// What a deferred tool is searchable by: its server, its name (the tokenizer
/// splits `get_issue` and `getIssue` into words), description, and each
/// parameter's name and description.
fn search_text(server: &str, schema: &Value) -> String {
    let f = &schema["function"];
    let mut t = format!("{server} {} {}", f["name"].as_str().unwrap_or(""), f["description"].as_str().unwrap_or(""));
    if let Some(props) = f["parameters"]["properties"].as_object() {
        for (k, v) in props {
            t.push_str(&format!(" {k} {}", v["description"].as_str().unwrap_or("")));
        }
    }
    t
}

const STOP: &[&str] = &["a", "an", "and", "are", "by", "for", "from", "in", "is", "it", "of", "on", "or", "that", "the", "this", "to", "with"];

/// Lowercase words, split at anything not alphanumeric and at camelCase
/// humps, stop words dropped, naively stemmed (plural s, -ing) so "issues"
/// finds "issue". ponytail: no real stemmer; "created" misses "create".
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut w = String::new();
    let mut prev_lower = false;
    for c in text.chars().chain([' ']) {
        if !c.is_alphanumeric() || (c.is_uppercase() && prev_lower) {
            if !w.is_empty() && !STOP.contains(&w.as_str()) {
                out.push(stem(&w).to_string());
            }
            w.clear();
        }
        if c.is_alphanumeric() {
            w.extend(c.to_lowercase());
        }
        prev_lower = c.is_lowercase();
    }
    out
}

fn stem(w: &str) -> &str {
    if w.len() > 5 && w.ends_with("ing") {
        &w[..w.len() - 3]
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        &w[..w.len() - 1]
    } else {
        w
    }
}

/// Okapi BM25 (k1 1.2, b 0.75): (doc index, score) for every doc sharing a
/// word with the query, best first, ties in doc order. ponytail: rebuilt per
/// search, which is nothing next to the network call around it.
fn bm25(query: &str, docs: &[String]) -> Vec<(usize, f64)> {
    let (k1, b) = (1.2, 0.75);
    let docs: Vec<Vec<String>> = docs.iter().map(|d| words(d)).collect();
    let n = docs.len() as f64;
    let avg = docs.iter().map(Vec::len).sum::<usize>() as f64 / n.max(1.0);
    let mut q = words(query);
    q.sort();
    q.dedup();
    let mut scored: Vec<(usize, f64)> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let score = q
                .iter()
                .map(|t| {
                    let tf = d.iter().filter(|w| *w == t).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let df = docs.iter().filter(|d| d.contains(t)).count() as f64;
                    let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                    idf * tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * d.len() as f64 / avg.max(1.0)))
                })
                .sum::<f64>();
            (i, score)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
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
/// A Server whose child has already exited, for registry tests that never
/// call it.
pub(crate) fn dead_server(name: &str, defer: bool, tools: Vec<(String, String, Value)>) -> Server {
    let mut child = Command::new(if cfg!(windows) { "cmd" } else { "true" })
        .args(if cfg!(windows) { &["/C", "exit"][..] } else { &[][..] })
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    Server {
        name: name.into(),
        tools,
        defer,
        stdin: Mutex::new(stdin),
        rx: Mutex::new(mpsc::channel().1),
        child: Mutex::new(child),
        errs: Arc::new(Mutex::new(VecDeque::new())),
        next_id: AtomicU64::new(1),
    }
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
            defer: false,
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

    /// BM25 ranks the tool that matches the most query words, rarer words
    /// weighing more; plurals and camelCase still match, and a query sharing
    /// no word with any tool loads nothing (F08).
    #[test]
    fn bm25_ranks_deferred_tools_by_relevance() {
        let docs = vec![
            "github create_pull_request Open a pull request from a branch".to_string(),
            "github getIssue Fetch one issue by number number The issue number".to_string(),
            "github list_issues List issues in a repository state open or closed".to_string(),
            "slack post_message Send a message to a channel".to_string(),
        ];
        let order = |q: &str| bm25(q, &docs).into_iter().map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(order("pull request"), [0]);
        assert_eq!(order("get issue")[0], 1, "camelCase splits: getIssue answers 'get issue'");
        assert_eq!(order("issues")[..2], [1, 2], "a plural matches the singular");
        assert_eq!(order("list open issues")[0], 2, "more matched words outrank one");
        assert_eq!(order("send slack message"), [3]);
        assert!(order("the of and").is_empty(), "stop words alone match nothing");
        assert!(order("kubernetes").is_empty());
        assert_eq!(words("getIssue list_issues"), ["get", "issue", "list", "issue"]);

        // search() over the live registry: loaded tools are not offered again
        let schema = |n: &str, d: &str| json!({"type": "function", "function": {"name": n, "description": d,
            "parameters": {"type": "object", "properties": {"repo": {"type": "string", "description": "owner/name"}}}}});
        let srv = dead_server("bm25test", true, vec![
            ("mcp__bm25test__get_issue".into(), "get_issue".into(), schema("mcp__bm25test__get_issue", "Fetch one issue\nmore text")),
            ("mcp__bm25test__send".into(), "send".into(), schema("mcp__bm25test__send", "Send a chat message")),
        ]);
        SERVERS.lock().unwrap().push(Arc::new(srv));
        assert!(!schemas().iter().any(|s| s["function"]["name"] == "mcp__bm25test__get_issue"), "deferred: not declared");
        let (text, names) = search("issue", 5, &[]);
        assert_eq!(names, ["mcp__bm25test__get_issue"]);
        assert_eq!(text, "Loaded 1 tool, available from your next call:\n- mcp__bm25test__get_issue — Fetch one issue");
        let (text, names) = search("issue", 5, &names);
        assert!(names.is_empty() && text.starts_with("No deferred tool matches"), "{text}");
        assert_eq!(search("owner name", 1, &[]).1.len(), 1, "parameter descriptions are searched; limit caps");
        // declared: tool_search while something is deferred, and a load appends at the very end
        let declared = |loaded: &[String]| super::super::tool_schemas(loaded).iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let plain = declared(&[]);
        assert!(plain.contains(&"tool_search".to_string()) && !plain.contains(&"mcp__bm25test__get_issue".to_string()));
        assert_eq!(declared(&["mcp__bm25test__get_issue".to_string()]).last().unwrap(), "mcp__bm25test__get_issue");
        SERVERS.lock().unwrap().retain(|s| s.name != "bm25test");
    }

    /// edit_stat reads back the "+N -M" the edit tools write. It must not fire
    /// on any other tool's output, or an unrelated result ending in two
    /// number-ish words would grow a fake diff stat on its ✓ row.
    #[test]
    fn edit_stat_only_matches_a_real_pair() {
        use crate::ai_core::edit_stat;
        assert_eq!(edit_stat("edited src/main.rs: 1 edit(s) +3 -1\n  · - a"), Some("+3 -1"));
        assert_eq!(edit_stat("wrote 42 bytes to x.txt +2 -0"), Some("+2 -0"));
        assert_eq!(edit_stat("read 30 lines"), None);
        assert_eq!(edit_stat("+3"), None, "both halves or nothing");
        assert_eq!(edit_stat("weird +x -y"), None, "digits, not any word");
        assert_eq!(edit_stat(""), None);
    }
}
