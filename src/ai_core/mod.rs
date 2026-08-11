// ai_core: the agent engine — async LLM client, tools, and the agent loop.

pub mod llm;
pub mod tools;

use crate::session::{Entry, Session};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, OnceLock};

/// Events the agent emits, for a TUI (or any front end) to render.
pub enum Event {
    TextDelta(String),                         // a chunk of model text (streamed)
    Text(String),                              // a complete line of text
    ToolStart(String),                         // tool about to run (short summary)
    ToolEnd { summary: String, ok: bool },     // tool finished
    Ask { question: String, reply: tokio::sync::oneshot::Sender<String> },
    TaskEnd { ok: bool, error: Option<String> }, // whole task finished
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
            Event::Text(t) => println!("{t}"),
            Event::ToolStart(t) => eprintln!("  ⠋ {t}"),
            Event::ToolEnd { summary, ok } => eprintln!("  {} {summary}", if ok { "✓" } else { "✗" }),
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
        },
    }
}

const SYSTEM_PROMPT: &str = "You are a coding agent running in a terminal on the user's machine. \
Use the provided tools to inspect code, modify files, and run commands. Work step by step. \
Never invent file contents or command output — use tools to verify. \
When the task is done, reply with a concise summary of what you changed.";

const MAX_ITERS: usize = 10;

pub async fn run_agent(
    client: &llm::Client,
    session: &mut Session,
    task: &str,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if session.entries.is_empty() {
        session.add(Entry::new("system", SYSTEM_PROMPT.into()), None);
    }
    if !task.is_empty() {
        // branch from the current leaf (or root); resume/--tree set active
        session.add(Entry::new("user", task.into()), session.active.clone());
    }
    let tools = tool_schemas();

    for _ in 0..MAX_ITERS {
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

        for tc in &res.tool_calls {
            let summary = tool_summary(&tc.name, &tc.arguments);
            emit(Event::ToolStart(summary.clone()));
            let (ok, result) = dispatch(&tc.name, &tc.arguments).await;
            emit(Event::ToolEnd { summary, ok });
            let mut te = Entry::new("tool", result);
            te.tool_call_id = Some(tc.id.clone());
            session.add(te, Some(a_id.clone()));
        }
        session.save().map_err(|e| format!("saving session: {e}"))?;
    }
    Err("hit max iterations without a final answer".into())
}

/// Short human-ish summary for a tool call (path or command, not raw JSON).
fn tool_summary(name: &str, args: &Value) -> String {
    match name {
        "read_file" | "write_file" | "edit_file" =>
            args["path"].as_str().unwrap_or("?").to_string(),
        "run_command" => args["command"].as_str().unwrap_or("?").to_string(),
        _ => format!("{name}({args})"),
    }
}

fn tool_schemas() -> Vec<Value> {
    vec![
        json!({"type":"function","function":{"name":"read_file","description":"Read a file's contents.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}),
        json!({"type":"function","function":{"name":"write_file","description":"Write content to a file, overwriting it.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}}),
        json!({"type":"function","function":{"name":"run_command","description":"Run a shell command; returns stdout, stderr and exit code.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}}),
        json!({"type":"function","function":{"name":"edit_file","description":"Replace one exact text occurrence in a file. old_text must appear exactly once.","parameters":{"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"]}}}),
        json!({"type":"function","function":{"name":"ask_user","description":"Ask the user a question (clarification, decision, approval) and return their answer.","parameters":{"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}}}),
    ]
}

async fn dispatch(name: &str, args: &Value) -> (bool, String) {
    match name {
        "read_file" => tools::read_file(args["path"].as_str().unwrap_or("")),
        "write_file" => tools::write_file(args["path"].as_str().unwrap_or(""), args["content"].as_str().unwrap_or("")),
        "run_command" => tools::run_command(args["command"].as_str().unwrap_or("")),
        "edit_file" => tools::edit_file(
            args["path"].as_str().unwrap_or(""),
            args["old_text"].as_str().unwrap_or(""),
            args["new_text"].as_str().unwrap_or(""),
        ),
        "ask_user" => tools::ask_user(args["question"].as_str().unwrap_or("")).await,
        other => (false, format!("unknown tool: {other}")),
    }
    // ponytail: sync tools block the agent task; spawn_blocking them when a
    // command ever runs long enough to stall streaming
}

pub fn self_test() {
    // Fake SSE server: first request -> a streamed run_command tool call
    // (arguments split across chunks), second -> a final answer.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let tool_args_1 = r#"{"command":"echo hi"}"#;
        let sse_tool = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"tool_calls\":[{{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{{\"name\":\"run_command\",\"arguments\":\"\"}}}}]}},\"finish_reason\":null}}]}}\r\n\r\n\
             data: {{\"choices\":[{{\"delta\":{{\"tool_calls\":[{{\"index\":0,\"function\":{{\"arguments\":\"{}\"}}}}]}},\"finish_reason\":null}}]}}\r\n\r\n\
             data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\r\n\r\n\
             data: [DONE]\r\n\r\n",
            tool_args_1.replace('"', "\\\"")
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

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let client = llm::Client::new(format!("http://127.0.0.1:{port}/v1/chat/completions"), "".into(), "fake".into());
        // text is streamed via emit() -> prints to stdout during the test; fine
        let mut session = crate::session::Session::with_path("fake".into(), "_test_session.json");
        assert_eq!(run_agent(&client, &mut session, "test task", &std::sync::atomic::AtomicBool::new(false)).await.unwrap(), "done");
        // tree: system, user, assistant(tool_calls), tool, assistant(done)
        assert_eq!(session.entries.len(), 5);
        let msgs = session.path_messages();
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[2]["tool_calls"].is_array(), true);
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[4]["content"], "done");
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

    // sync tool checks (no runtime needed)
    assert!(tools::run_command("echo hi").1.contains("hi"));
    assert!(tools::write_file("_test_tmp.txt", "x").1.contains("wrote"));
    assert!(tools::read_file("_test_tmp.txt").1.contains("x"));
    assert!(tools::edit_file("_test_tmp.txt", "x", "y").1.contains("edited"));
    assert_eq!(tools::read_file("_test_tmp.txt").1, "y");
    assert!(tools::edit_file("_test_tmp.txt", "zzz", "y").1.contains("not found"));
    std::fs::remove_file("_test_tmp.txt").unwrap();

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
