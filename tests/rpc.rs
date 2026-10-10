// --json and --rpc end to end: the real binary against a fake chat server,
// stdout read as the JSONL a consuming program would read.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn sse(delta: Value, finish: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices": [{"delta": delta, "finish_reason": finish}]}))
}

/// Serves `replies` in order on a fresh port; each request body goes to the channel.
fn fake_llm(replies: Vec<String>) -> (String, mpsc::Receiver<Value>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", listener.local_addr().unwrap().port());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (reply, s) in replies.iter().zip(listener.incoming()) {
            let mut s = s.unwrap();
            let (mut req, mut buf) = (Vec::new(), [0u8; 65536]);
            let body = loop {
                let k = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..k]);
                let Some(h) = req.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&req[..h]).to_lowercase();
                let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                if req.len() >= h + 4 + len {
                    break serde_json::from_slice::<Value>(&req[h + 4..h + 4 + len]).unwrap();
                }
            };
            let _ = tx.send(body);
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
            s.write_all(resp.as_bytes()).unwrap();
        }
    });
    (url, rx)
}

fn spawn(name: &str, url: &str, args: &[&str]) -> (Child, mpsc::Receiver<Value>) {
    let dir = std::env::temp_dir().join(format!("rusti_{name}_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rusti"))
        .args(["--url", url, "--model", "m"])
        .args(args)
        .current_dir(&dir)
        .env("RUSTI_HOME", dir.join("home"))
        .env_remove("RUSTI_YOLO")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // every stdout line must parse: stdout is protocol only
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines() {
            let line = line.unwrap();
            let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"));
            assert_eq!(v["v"], 1, "every line is versioned: {line}");
            if tx.send(v).is_err() {
                return;
            }
        }
    });
    (child, rx)
}

/// Lines up to and including the first whose type is `ty`.
fn until(rx: &mpsc::Receiver<Value>, ty: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let v = rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| panic!("no {ty} line; saw {seen:?}"));
        let done = v["type"] == ty;
        seen.push(v);
        if done {
            return seen;
        }
    }
}

#[test]
fn json_mode_streams_a_one_shot_run_as_versioned_lines() {
    let (url, _) = fake_llm(vec![sse(json!({"content": "hello"}), "stop")]);
    let (mut child, rx) = spawn("json", &url, &["--json", "say hi"]);
    let lines = until(&rx, "task_end");
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(lines[0]["type"], "session", "the header comes first: {lines:?}");
    assert!(lines.iter().any(|l| l["type"] == "text_delta" && l["text"] == "hello"));
    let end = lines.last().unwrap();
    assert_eq!((end["ok"].clone(), end["answer"].clone()), (json!(true), json!("hello")));
}

#[test]
fn rpc_mode_runs_prompts_and_turns_questions_into_requests() {
    let ask = sse(json!({"tool_calls": [{"index": 0, "id": "c1", "type": "function",
        "function": {"name": "ask_user", "arguments": "{\"question\":\"which file?\"}"}}]}), "tool_calls");
    let (url, bodies) = fake_llm(vec![sse(json!({"content": "hi there"}), "stop"), ask, sse(json!({"content": "done"}), "stop")]);
    let (mut child, rx) = spawn("rpc", &url, &["--rpc"]);
    let mut stdin = child.stdin.take().unwrap();
    let mut send = |v: Value| writeln!(stdin, "{v}").unwrap();
    assert_eq!(until(&rx, "session")[0]["type"], "session");

    send(json!({"id": 1, "type": "prompt", "text": "hello"}));
    let lines = until(&rx, "task_end");
    let r = lines.iter().find(|l| l["type"] == "response").expect("the prompt is answered");
    assert_eq!((r["id"].clone(), r["ok"].clone(), r["data"]["accepted"].clone()), (json!(1), json!(true), json!("run")));
    assert!(lines.iter().any(|l| l["type"] == "text_delta" && l["text"] == "hi there"));

    send(json!({"id": 2, "type": "prompt", "text": "edit something"}));
    let ask = until(&rx, "ask").pop().unwrap();
    assert_eq!(ask["question"], "which file?");
    send(json!({"id": 3, "type": "get_state"}));
    let state = until(&rx, "response").pop().unwrap();
    assert_eq!((state["id"].clone(), state["data"]["busy"].clone()), (json!(3), json!(true)));
    assert_eq!(state["data"]["asks"], json!([ask["id"]]));
    send(json!({"id": 4, "type": "answer", "ask": ask["id"], "text": "src/main.rs"}));
    let lines = until(&rx, "task_end");
    assert_eq!(lines.last().unwrap()["ok"], true);

    send(json!({"id": 5, "type": "get_messages"}));
    let msgs = until(&rx, "response").pop().unwrap();
    assert_eq!(msgs["id"], 5);
    let roles: Vec<&str> = msgs["data"]["messages"].as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["system", "user", "assistant", "user", "assistant", "tool", "assistant"]);

    drop(stdin); // EOF: rusti finishes and exits
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let (_, _, third) = (bodies.recv().unwrap(), bodies.recv().unwrap(), bodies.recv().unwrap());
    let last = third["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["role"].clone(), last["content"].clone()), (json!("tool"), json!("src/main.rs")), "the answer is the tool result");
}

/// --yolo starts in devil mode; set_plan enters plan and leaving it lands on manual.
#[test]
fn yolo_starts_in_devil_and_plan_returns_to_manual() {
    let (url, _) = fake_llm(vec![]);
    for (args, start) in [(&["--rpc"][..], "manual"), (&["--rpc", "--yolo"][..], "devil")] {
        let (mut child, rx) = spawn(&format!("rpc_mode_{start}"), &url, args);
        let mut stdin = child.stdin.take().unwrap();
        until(&rx, "session");
        // set_plan answers too, so every command here is one response line
        let mut send = |cmd: Value| {
            writeln!(stdin, "{cmd}").unwrap();
            until(&rx, "response").pop().unwrap()
        };
        let state = json!({"id": 1, "type": "get_state"});
        assert_eq!(send(state.clone())["data"]["mode"], start);
        send(json!({"id": 2, "type": "set_plan", "on": true}));
        assert_eq!(send(state.clone())["data"]["mode"], "plan");
        send(json!({"id": 3, "type": "set_plan", "on": false}));
        assert_eq!(send(state)["data"]["mode"], "manual");
        drop(stdin);
        assert_eq!(child.wait().unwrap().code(), Some(0));
    }
}

#[test]
fn rpc_abort_right_after_accepted_cancels_that_run() {
    let (url, _) = fake_llm(vec![]); // nothing listens: a run that is not cancelled retries and fails
    let (mut child, rx) = spawn("rpc_abort", &url, &["--rpc"]);
    let mut stdin = child.stdin.take().unwrap();
    until(&rx, "session");
    writeln!(stdin, "{}\n{}", json!({"id": 1, "type": "prompt", "text": "hello"}), json!({"id": 2, "type": "abort"})).unwrap();
    let end = until(&rx, "task_end").pop().unwrap();
    assert_eq!((end["ok"].clone(), end["error"].clone()), (json!(false), json!("interrupted")));
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));
}
