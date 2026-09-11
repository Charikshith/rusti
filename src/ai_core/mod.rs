// ai_core: the agent engine — async LLM client, tools, and the agent loop.

pub mod llm;
pub mod tools;

use crate::session::{Entry, Session};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, OnceLock};

/// Events the agent emits, for a TUI (or any front end) to render.
pub enum Event {
    TextDelta(String),                         // a chunk of model text (streamed)
    ReasoningDelta(String),                    // a chunk of reasoning_content (thinking models)
    Text(String),                              // a complete line of text
    ToolStart(String),                         // tool about to run (short summary)
    ToolEnd { summary: String, ok: bool, ms: u128 }, // tool finished, with wall time
    Resumed { lines: Vec<String>, history: Vec<String>, msg_num: usize }, // session switched: transcript replaced
    SessionName(String),                       // active session's name, for the status line
    Usage { tokens: u64, est: bool, gen_ms: u128 }, // one LLM call's generation accounting
    Ask { question: String, reply: tokio::sync::oneshot::Sender<String> },
    TaskEnd { ok: bool, error: Option<String> }, // whole task finished
    Reload { exe: String, args: Vec<String> },   // TUI /reload: new binary built, ready to relaunch
}

/// Optional front-end sink. When set, the agent sends Events instead of
/// printing to stdout/stderr, and ask_user routes through the front end.
static SINK: OnceLock<mpsc::Sender<Event>> = OnceLock::new();

pub fn set_event_sink(tx: mpsc::Sender<Event>) {
    let _ = SINK.set(tx);
}

fn emit(ev: Event) {
    match SINK.get() {
        Some(tx) => {
            let _ = tx.send(ev);
        }
        None => match ev {
            Event::TextDelta(t) => {
                use std::io::Write;
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            Event::ReasoningDelta(t) => {
                use std::io::Write;
                eprint!("{t}");
                let _ = std::io::stderr().flush();
            }
            Event::Text(t) => println!("{t}"),
            Event::ToolStart(t) => eprintln!("  ⠋ {t}"),
            Event::ToolEnd { summary, ok, ms } => {
                eprintln!("  {} {summary}  {}", if ok { "✓" } else { "✗" }, took(ms))
            }
            Event::TaskEnd { ok, error } => {
                if !ok {
                    let line = match error {
                        Some(e) => format!("  ✗ Task failed: {e}"),
                        None => "  ✗ Task failed".into(),
                    };
                    eprintln!("{line}");
                }
            }
            Event::Ask { .. } => {}
            Event::Resumed { .. } => {} // TUI-only: replaces the on-screen transcript
            Event::SessionName(_) => {} // TUI-only: status-line label
            Event::Usage { .. } => {}  // per-turn stats are a TUI line
            Event::Reload { .. } => {} // TUI-only; no-op without a front end
        },
    }
}

const SYSTEM_PROMPT: &str = "You are a coding agent running in a terminal on the user's machine. \
Use the provided tools to inspect code, modify files, and run commands. Work step by step. \
Prefer grep, glob and list_dir over shell commands for finding code; use read_file with offset/limit for large files. \
Use todo to plan and track multi-step tasks. Writes and commands may need the user's approval; a denial is final \
for that call: explain or ask_user, do not retry it. Use run_background for servers and watchers, and job_stop \
what you started before finishing. Use delegate for a self-contained subtask whose details you do not need. \
Never invent file contents or command output — use tools to verify. \
When the task is done, reply with a concise summary of what you changed.";

/// Tool-call rounds per task. 50 fits a real read/edit/test/fix cycle; --max-iters overrides.
static MAX_ITERS: AtomicUsize = AtomicUsize::new(50);

pub fn set_max_iters(n: usize) {
    MAX_ITERS.store(n.max(1), Ordering::Relaxed);
}

/// Project instructions files, first hit wins. Read every turn so edits are live.
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "RUSTI.md", "CLAUDE.md"];
const MAX_INSTRUCTIONS: usize = 20_000;

fn instructions_from(dir: &std::path::Path) -> Option<(String, String)> {
    INSTRUCTION_FILES.iter().find_map(|name| {
        let s = std::fs::read_to_string(dir.join(name)).ok()?;
        let s = s.trim();
        if s.is_empty() { return None; }
        let body = if s.len() > MAX_INSTRUCTIONS {
            let mut cut = MAX_INSTRUCTIONS;
            while !s.is_char_boundary(cut) { cut -= 1; }
            format!("{}\n…[truncated]", &s[..cut])
        } else {
            s.to_string()
        };
        Some((name.to_string(), body))
    })
}

fn system_prompt() -> String {
    match instructions_from(std::path::Path::new(".")) {
        Some((name, body)) => format!("{SYSTEM_PROMPT}\n\n# Project instructions (from {name} in the working directory)\n{body}"),
        None => SYSTEM_PROMPT.to_string(),
    }
}

/// Tools that change state or run code; each call asks the user unless --yolo or "always" was given.
const GATED: &[&str] = &["write_file", "edit_file", "multi_edit", "run_command", "run_background", "delete_file", "move_file"];
static ALLOWED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

async fn permitted(name: &str, summary: &str) -> Result<(), String> {
    if tools::YOLO.load(Ordering::Relaxed) || !GATED.contains(&name) || ALLOWED.lock().unwrap().iter().any(|a| a == name) {
        return Ok(());
    }
    let (_, ans) = tools::ask_user(&format!("allow {name} {summary}? [y]es / [n]o / [a]lways for {name}")).await;
    if decide(name, &ans) { Ok(()) } else { Err(format!("user denied {name}: {}", ans.trim())) }
}

/// y/yes -> once, a/always -> this tool for the rest of the process, anything else -> deny.
fn decide(name: &str, answer: &str) -> bool {
    match answer.trim().to_ascii_lowercase().as_str() {
        "a" | "always" => { ALLOWED.lock().unwrap().push(name.to_string()); true }
        "y" | "yes" => true,
        _ => false,
    }
}

/// One level of delegation: a fresh session under .rusti/sessions/, same tools, same cap.
static DEPTH: AtomicUsize = AtomicUsize::new(0);

async fn delegate(client: &llm::Client, task: &str, cancel: &AtomicBool) -> (bool, String) {
    if DEPTH.fetch_add(1, Ordering::Relaxed) > 0 {
        DEPTH.fetch_sub(1, Ordering::Relaxed);
        return (false, "nested delegation is not allowed; do this part directly".into());
    }
    let path = format!("{}/sub-{}-{}.json", crate::session::DIR, std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let mut sub = Session::with_path(client.model.clone(), &path);
    let prompt = format!("You are a sub-agent given one scoped task by the main agent. Complete it, then reply with a \
concise report: what you found or changed, exact file paths, and anything the main agent must know.\n\nTask: {task}");
    // ponytail: the sub-agent streams into the same transcript as its parent; a nested block would need TUI work
    let r = Box::pin(run_agent(client, &mut sub, &prompt, cancel)).await;
    DEPTH.fetch_sub(1, Ordering::Relaxed);
    match r {
        Ok(t) => (true, tools::truncate(&format!("[sub-agent session: {path}]\n{t}"), tools::MAX_RESULT)),
        Err(e) => (false, format!("sub-agent failed: {e}")),
    }
}

pub async fn run_agent(
    client: &llm::Client,
    session: &mut Session,
    task: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let sys_prompt = system_prompt();
    if session.entries.is_empty() {
        session.add(Entry::new("system", sys_prompt), None);
    } else if let Some(sys) = session.entries.first_mut() {
        // a resumed session's system entry was frozen at creation time; keep it
        // live so editing SYSTEM_PROMPT / AGENTS.md + rebuilding (see tui /reload)
        // takes effect on the next turn instead of staying pinned to stale text.
        if sys.role == "system" {
            sys.content = sys_prompt;
        }
    }
    if !task.is_empty() {
        // branch from the current leaf (or root); resume/--tree set active
        session.add(Entry::new("user", task.into()), session.active.clone());
    }
    let tools = tool_schemas();

    for _ in 0..MAX_ITERS.load(Ordering::Relaxed) {
        if cancel.load(Ordering::Relaxed) {
            return Err("interrupted".into());
        }
        let res = client.chat_stream(&session.path_messages(), Some(&tools), cancel).await?;
        if res.finish_reason != "tool_calls" {
            session.add(Entry::new("assistant", res.content.clone()), session.active.clone());
            session.save().map_err(|e| format!("saving session: {e}"))?;
            return Ok(res.content.trim().to_string());
        }

        let tcs: Value = json!(res
            .tool_calls
            .iter()
            .map(|tc| json!({"id": tc.id, "type": "function",
                              "function": {"name": tc.name, "arguments": tc.raw_arguments}}))
            .collect::<Vec<_>>());
        let mut ae = Entry::new("assistant", res.content.clone());
        ae.tool_calls = Some(tcs);
        let a_id = session.add(ae, session.active.clone());

        // Chain tool results under each other, not as siblings of the assistant
        // entry. path_messages() walks the active leaf's parent chain, so sibling
        // results leave earlier ones off the path -> the next request has a
        // dangling tool call -> "Tool result is missing for tool call ...".
        let mut parent = a_id.clone();
        for tc in &res.tool_calls {
            let summary = tool_summary(&tc.name, &tc.arguments);
            emit(Event::ToolStart(summary.clone()));
            let t0 = std::time::Instant::now();
            let (ok, result) = match permitted(&tc.name, &summary).await {
                Ok(()) => dispatch(client, &tc.name, &tc.arguments, cancel).await,
                Err(e) => (false, e),
            };
            emit(Event::ToolEnd { summary, ok, ms: t0.elapsed().as_millis() });
            let mut te = Entry::new("tool", result);
            te.tool_call_id = Some(tc.id.clone());
            parent = session.add(te, Some(parent));
        }
        session.save().map_err(|e| format!("saving session: {e}"))?;
    }
    Err("hit max iterations without a final answer".into())
}

/// Wall time for a finished tool, terminal-short: "450ms" / "1.6s".
pub fn took(ms: u128) -> String {
    if ms < 1000 { format!("{ms}ms") } else { format!("{:.1}s", ms as f64 / 1000.0) }
}

/// Short human-ish summary for a tool call (path or command, not raw JSON).
fn tool_summary(name: &str, args: &Value) -> String {
    match name {
        "read_file" | "write_file" | "edit_file" | "list_dir" =>
            args["path"].as_str().unwrap_or("?").to_string(),
        "multi_edit" => format!("{} ({} edits)", args["path"].as_str().unwrap_or("?"),
            args["edits"].as_array().map_or(0, |a| a.len())),
        "run_command" => args["command"].as_str().unwrap_or("?").to_string(),
        "grep" => format!("\"{}\" in {}", args["pattern"].as_str().unwrap_or("?"),
            args["path"].as_str().filter(|p| !p.is_empty()).unwrap_or(".")),
        "glob" => args["pattern"].as_str().unwrap_or("?").to_string(),
        "run_background" => format!("(background) {}", args["command"].as_str().unwrap_or("?")),
        "job_output" | "job_stop" => format!("job {}", args["id"].as_u64().unwrap_or(0)),
        "delete_file" => args["path"].as_str().unwrap_or("?").to_string(),
        "move_file" => format!("{} -> {}", args["from"].as_str().unwrap_or("?"), args["to"].as_str().unwrap_or("?")),
        "todo" => format!("todo ({} items)", args["items"].as_array().map_or(0, |a| a.len())),
        "delegate" => format!("delegate: {}", args["task"].as_str().unwrap_or("?").chars().take(80).collect::<String>()),
        _ => format!("{name}({args})"),
    }
}

fn tool_schemas() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"read_file","description":"Read a file's contents. Optional offset (1-based line) and limit (max lines) read a slice with line numbers; use them for large files.","parameters":{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"]}}}),
        json!({"type":"function","function":{"name":"write_file","description":"Write content to a file, overwriting it.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}}),
        json!({"type":"function","function":{"name":"run_command","description":"Run a shell command; returns stdout, stderr and exit code. Killed after timeout_secs (default 120).","parameters":{"type":"object","properties":{"command":{"type":"string"},"timeout_secs":{"type":"integer"}},"required":["command"]}}}),
        json!({"type":"function","function":{"name":"edit_file","description":"Replace one exact text occurrence in a file. old_text must appear exactly once.","parameters":{"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"]}}}),
        json!({"type":"function","function":{"name":"multi_edit","description":"Apply several exact replacements to one file in order, all-or-nothing. Each old_text must appear exactly once.","parameters":{"type":"object","properties":{"path":{"type":"string"},"edits":{"type":"array","items":{"type":"object","properties":{"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["old_text","new_text"]}}},"required":["path","edits"]}}}),
        json!({"type":"function","function":{"name":"grep","description":"Search file contents for a regex. Returns path:line:text. path defaults to '.'; glob (e.g. '*.rs') filters files.","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"}},"required":["pattern"]}}}),
        json!({"type":"function","function":{"name":"glob","description":"List files matching a glob pattern such as '*.rs' or 'src/**/*.rs'. path defaults to '.'.","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]}}}),
        json!({"type":"function","function":{"name":"delete_file","description":"Delete a file inside the project.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}),
        json!({"type":"function","function":{"name":"move_file","description":"Move or rename a file inside the project. Fails if the destination exists.","parameters":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"}},"required":["from","to"]}}}),
        json!({"type":"function","function":{"name":"run_background","description":"Start a long-running command (dev server, watcher) without waiting. Returns a job id; read it with job_output, kill it with job_stop.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}}),
        json!({"type":"function","function":{"name":"job_output","description":"Output so far and status of a background job. id 0 lists all jobs.","parameters":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}}),
        json!({"type":"function","function":{"name":"job_stop","description":"Kill a background job.","parameters":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}}),
        json!({"type":"function","function":{"name":"todo","description":"Replace your task list; shown to the user. Call again with updated statuses as you progress. Empty list clears it.","parameters":{"type":"object","properties":{"items":{"type":"array","items":{"type":"object","properties":{"text":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","done"]}},"required":["text","status"]}}},"required":["items"]}}}),
        json!({"type":"function","function":{"name":"delegate","description":"Hand a self-contained subtask to a fresh sub-agent with the same tools; returns only its final report, keeping its work out of your context. Not nestable.","parameters":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"]}}}),
        json!({"type":"function","function":{"name":"list_dir","description":"List a directory (directories end with '/'). depth defaults to 1.","parameters":{"type":"object","properties":{"path":{"type":"string"},"depth":{"type":"integer"}},"required":["path"]}}}),
        json!({"type":"function","function":{"name":"ask_user","description":"Ask the user a question (clarification, decision, approval) and return their answer.","parameters":{"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}}}),
    ]
}

async fn dispatch(client: &llm::Client, name: &str, args: &Value, cancel: &AtomicBool) -> (bool, String) {
    match name {
        "read_file" => tools::read_file(
            args["path"].as_str().unwrap_or(""),
            args["offset"].as_u64().unwrap_or(0) as usize,
            args["limit"].as_u64().unwrap_or(0) as usize,
        ),
        "write_file" => tools::write_file(args["path"].as_str().unwrap_or(""), args["content"].as_str().unwrap_or("")),
        "run_command" => tools::run_command(
            args["command"].as_str().unwrap_or(""),
            args["timeout_secs"].as_u64().unwrap_or(0),
        ),
        "edit_file" => tools::edit_file(
            args["path"].as_str().unwrap_or(""),
            args["old_text"].as_str().unwrap_or(""),
            args["new_text"].as_str().unwrap_or(""),
        ),
        "multi_edit" => {
            let edits: Vec<(String, String)> = args["edits"]
                .as_array()
                .map(|a| a.iter().map(|e| (
                    e["old_text"].as_str().unwrap_or("").to_string(),
                    e["new_text"].as_str().unwrap_or("").to_string(),
                )).collect())
                .unwrap_or_default();
            tools::multi_edit(args["path"].as_str().unwrap_or(""), &edits)
        }
        "grep" => tools::grep(
            args["pattern"].as_str().unwrap_or(""),
            args["path"].as_str().unwrap_or(""),
            args["glob"].as_str().unwrap_or(""),
        ),
        "glob" => tools::glob(args["pattern"].as_str().unwrap_or(""), args["path"].as_str().unwrap_or("")),
        "delete_file" => tools::delete_file(args["path"].as_str().unwrap_or("")),
        "move_file" => tools::move_file(args["from"].as_str().unwrap_or(""), args["to"].as_str().unwrap_or("")),
        "run_background" => tools::run_background(args["command"].as_str().unwrap_or("")),
        "job_output" => tools::job_output(args["id"].as_u64().unwrap_or(0) as u32),
        "job_stop" => tools::job_stop(args["id"].as_u64().unwrap_or(0) as u32),
        "todo" => {
            let items: Vec<(String, String)> = args["items"].as_array()
                .map(|a| a.iter().map(|e| (
                    e["text"].as_str().unwrap_or("").to_string(),
                    e["status"].as_str().unwrap_or("pending").to_string(),
                )).collect())
                .unwrap_or_default();
            let r = tools::todo(items);
            for line in r.1.lines() {
                emit(Event::Text(format!("  {line}")));
            }
            r
        }
        "delegate" => delegate(client, args["task"].as_str().unwrap_or(""), cancel).await,
        "list_dir" => tools::list_dir(args["path"].as_str().unwrap_or(""), args["depth"].as_u64().unwrap_or(0) as usize),
        "ask_user" => tools::ask_user(args["question"].as_str().unwrap_or("")).await,
        other => (false, format!("unknown tool: {other}")),
    }
    // ponytail: sync tools block the agent task; spawn_blocking them when a
    // command ever runs long enough to stall streaming
}

pub fn self_test() {
    // Fake SSE server: first request -> two streamed tool calls (run_command +
    // list_dir), second -> a final answer. Two calls in one turn exercises the
    // sibling-tool-result chaining that caused dangling tool calls.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let tool_args_1 = r#"{"command":"echo hi"}"#.replace('"', "\\\"");
        let tool_args_2 = r#"{"path":".","depth":1}"#.replace('"', "\\\"");
        let sse_tool = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"tool_calls\":[{{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{{\"name\":\"run_command\",\"arguments\":\"\"}}}},{{\"index\":1,\"id\":\"c2\",\"type\":\"function\",\"function\":{{\"name\":\"list_dir\",\"arguments\":\"\"}}}}]}},\"finish_reason\":null}}]}}\r\n\r\n\
             data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":\"{tool_args_1}\"}}}}]}},\"finish_reason\":null}}]}}\r\n\r\n\
             data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":1,\"function\":{{\"arguments\":\"{tool_args_2}\"}}}}]}},\"finish_reason\":null}}]}}\r\n\r\n\
             data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\r\n\r\n\
             data: [DONE]\r\n\r\n"
        );
        let sse_done = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        );
        let mut n = 0;
        for s in listener.incoming() {
            let mut s = s.unwrap();
            let mut buf = [0u8; 65536];
            s.read(&mut buf).unwrap();
            n += 1;
            let body = if n == 1 { &sse_tool } else { sse_done };
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
            if n >= 2 { break; }
        }
    });

    tools::YOLO.store(true, Ordering::Relaxed); // the fake stream calls run_command; no stdin to answer a prompt
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let client = llm::Client::new(format!("http://127.0.0.1:{port}/v1/chat/completions"), "".into(), "fake".into());
        // text is streamed via emit() -> prints to stdout during the test; fine
        let mut session = crate::session::Session::with_path("fake".into(), "_test_session.json");
        assert_eq!(run_agent(&client, &mut session, "test task", &std::sync::atomic::AtomicBool::new(false)).await.unwrap(), "done");
        // tree: system, user, assistant(tool_calls), tool, tool, assistant(done)
        // both tool results must be on the active path (multi-tool-call fix)
        assert_eq!(session.entries.len(), 6);
        let msgs = session.path_messages();
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["tool_calls"].is_array(), true);
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[4]["role"], "tool");
        assert_eq!(msgs[5]["content"], "done");
        // branch from the user message: leaf moves to its parent (system)
        assert!(session.select("m2").is_some());
        assert_eq!(session.path().len(), 1);
        // reload from disk keeps the tree
        let s2 = crate::session::Session::load_from("_test_session.json");
        assert_eq!(s2.entries.len(), 5);
        std::fs::remove_file("_test_session.json").ok();

        // interrupt: a pre-set flag cancels before any network call
        let cancelled = std::sync::atomic::AtomicBool::new(true);
        let mut s3 = crate::session::Session::with_path("fake".into(), "_test_session2.json");
        assert_eq!(run_agent(&client, &mut s3, "x", &cancelled).await.unwrap_err(), "interrupted");
        std::fs::remove_file("_test_session2.json").ok();
    });

    assert_eq!(took(450), "450ms");
    assert_eq!(took(1600), "1.6s");

    // max iters: 1 round with a tool call and no final answer -> iteration error
    set_max_iters(1);
    assert_eq!(MAX_ITERS.load(Ordering::Relaxed), 1);
    set_max_iters(0); // floors at 1
    assert_eq!(MAX_ITERS.load(Ordering::Relaxed), 1);
    set_max_iters(50);

    // project instructions: AGENTS.md wins over RUSTI.md; empty/missing -> none
    let d = std::path::Path::new("_test_instr");
    std::fs::create_dir_all(d).unwrap();
    assert!(instructions_from(d).is_none());
    std::fs::write(d.join("RUSTI.md"), "use tabs").unwrap();
    assert_eq!(instructions_from(d).unwrap(), ("RUSTI.md".to_string(), "use tabs".to_string()));
    std::fs::write(d.join("AGENTS.md"), "  \n").unwrap(); // blank file is skipped
    assert_eq!(instructions_from(d).unwrap().0, "RUSTI.md");
    std::fs::write(d.join("AGENTS.md"), "run cargo test").unwrap();
    assert_eq!(instructions_from(d).unwrap(), ("AGENTS.md".to_string(), "run cargo test".to_string()));
    std::fs::remove_dir_all(d).unwrap();
    assert!(system_prompt().starts_with(SYSTEM_PROMPT));

    // permission decisions
    assert!(decide("write_file", "y") && decide("write_file", " Yes "));
    assert!(!decide("write_file", "n") && !decide("write_file", "") && !decide("write_file", "no answer given"));
    assert!(decide("run_command", "a"));
    assert!(ALLOWED.lock().unwrap().iter().any(|a| a == "run_command"));

    // project-root guard (yolo off for this block)
    tools::YOLO.store(false, Ordering::Relaxed);
    assert!(tools::guard("src/main.rs").is_ok());
    assert!(tools::guard("_new_dir/_new_file.txt").is_ok()); // not yet existing, still inside
    assert!(tools::guard("../_outside.txt").is_err());
    assert!(tools::write_file("../_outside.txt", "x").1.contains("refused"));
    assert!(tools::guard(&std::env::temp_dir().join("x").to_string_lossy()).is_err());
    tools::YOLO.store(true, Ordering::Relaxed);

    // delete / move
    tools::write_file("_test_mv.txt", "m");
    assert!(tools::move_file("_test_mv.txt", "_test_mv2.txt").0);
    assert!(!tools::move_file("_test_mv2.txt", "Cargo.toml").0); // destination exists
    assert_eq!(tools::read_file("_test_mv2.txt", 0, 0).1, "m");
    assert!(tools::delete_file("_test_mv2.txt").0);
    assert!(!tools::delete_file("_test_mv2.txt").0);

    // background jobs
    let slow = if cfg!(windows) { "ping -n 6 127.0.0.1" } else { "sleep 5" };
    let (ok, msg) = tools::run_background(slow);
    assert!(ok, "{msg}");
    let id: u32 = msg.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(tools::job_output(id).1.contains("running"));
    assert!(tools::job_output(0).1.contains(&format!("job {id}")));
    assert!(tools::job_stop(id).1.contains("stopped"));
    assert!(!tools::job_output(id).0);
    let (_, msg) = tools::run_background("echo bg-hi");
    let id: u32 = msg.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut out = String::new();
    for _ in 0..100 {
        out = tools::job_output(id).1;
        if out.contains("exited") { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(out.contains("exited 0") && out.contains("bg-hi"), "{out}");
    tools::job_stop(id);

    // todo render
    let items = vec![("read".to_string(), "done".to_string()), ("edit".to_string(), "in_progress".to_string()), ("test".to_string(), "pending".to_string())];
    assert_eq!(tools::todo(items).1, "\u{2611} read\n\u{25d0} edit\n\u{2610} test\n");
    assert_eq!(tools::todo(vec![]).1, "todo list cleared");

    // sync tool checks (no runtime needed)
    assert!(tools::run_command("echo hi", 0).1.contains("hi"));
    let slow = if cfg!(windows) { "ping -n 6 127.0.0.1" } else { "sleep 5" };
    let (ok, msg) = tools::run_command(slow, 1);
    assert!(!ok && msg.contains("timed out"), "{msg}");
    assert!(tools::write_file("_test_tmp.txt", "x").1.contains("wrote"));
    assert!(tools::read_file("_test_tmp.txt", 0, 0).1.contains("x"));
    assert!(tools::edit_file("_test_tmp.txt", "x", "y").1.contains("edited"));
    assert_eq!(tools::read_file("_test_tmp.txt", 0, 0).1, "y");
    assert!(tools::edit_file("_test_tmp.txt", "zzz", "y").1.contains("not found"));
    // ranged read + multi_edit (all-or-nothing)
    tools::write_file("_test_tmp.txt", "a\nb\nc\nd\n");
    assert_eq!(tools::read_file("_test_tmp.txt", 2, 2).1, "2: b\n3: c\n");
    let bad = [("a".to_string(), "A".to_string()), ("zzz".to_string(), "Z".to_string())];
    assert!(!tools::multi_edit("_test_tmp.txt", &bad).0);
    assert_eq!(tools::read_file("_test_tmp.txt", 0, 0).1, "a\nb\nc\nd\n"); // untouched
    let good = [("a".to_string(), "A".to_string()), ("c".to_string(), "C".to_string())];
    assert!(tools::multi_edit("_test_tmp.txt", &good).0);
    assert_eq!(tools::read_file("_test_tmp.txt", 0, 0).1, "A\nb\nC\nd\n");
    std::fs::remove_file("_test_tmp.txt").unwrap();
    // search tools (rg or std fallback — same assertions hold for both)
    assert!(tools::glob_match("*.rs", "src/ai_core/mod.rs"));
    assert!(tools::glob_match("src/**/*.rs", "src/ai_core/mod.rs"));
    assert!(tools::glob_match("src/*.rs", "src/main.rs"));
    assert!(!tools::glob_match("src/*.rs", "src/ai_core/mod.rs"));
    assert!(!tools::glob_match("*.toml", "src/main.rs"));
    assert!(tools::glob_match("m?in.rs", "main.rs"));
    assert!(tools::list_dir(".", 0).1.contains("src/"));
    assert!(tools::list_dir("src", 2).1.contains("ai_core/tools.rs"));
    assert!(tools::glob("*.toml", ".").1.contains("Cargo.toml"));
    assert!(tools::grep("fn run_agent", "src", "*.rs").1.contains("mod.rs"));
    assert!(tools::grep("no_such_token_xyz", "src", "").1.contains("no matches"));

    // config roundtrip
    let mut c = crate::config::Config::load_from("_test_model.json");
    assert!(c.resolve().is_none());
    c.add(crate::config::ModelProfile {
        name: "m1".into(),
        url: "http://x".into(),
        key: "".into(),
        model: "gpt-x".into(),
    });
    c.default = Some("m1".into());
    c.save_to("_test_model.json").unwrap();
    let c2 = crate::config::Config::load_from("_test_model.json");
    assert_eq!(c2.resolve().map(|m| m.model.as_str()), Some("gpt-x"));
    std::fs::remove_file("_test_model.json").unwrap();
    println!("self-test OK");
}
