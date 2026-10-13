// --json and --rpc end to end: the real binary against a fake chat server,
// stdout read as the JSONL a consuming program would read.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn sse(delta: Value, finish: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices": [{"delta": delta, "finish_reason": finish}]}))
}

/// Serves `replies` in order on a fresh port; each request body goes to the channel.
fn fake_llm(replies: Vec<String>) -> (String, mpsc::Receiver<Value>) {
    fake_llm_held(replies.into_iter().map(|r| (r, false)).collect())
}

/// fake_llm where a reply can hold its socket open after the body, the way a
/// stalled provider (or one that never closes after [DONE]) does. A held reply
/// has no Content-Length, so only the socket closing would end its stream.
fn fake_llm_held(replies: Vec<(String, bool)>) -> (String, mpsc::Receiver<Value>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", listener.local_addr().unwrap().port());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for ((reply, hold), s) in replies.iter().zip(listener.incoming()) {
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
            let len = if *hold { String::new() } else { format!("Content-Length: {}\r\n", reply.len()) };
            // a reply that is already a whole response (an error status) goes out as it is
            let resp = if reply.starts_with("HTTP/") {
                reply.clone()
            } else {
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n{len}Connection: close\r\n\r\n{reply}")
            };
            s.write_all(resp.as_bytes()).unwrap();
            if *hold {
                held.push(s);
            }
        }
        // out of replies: stop listening, so a further request is refused;
        // held sockets stay open for as long as the test runs
        drop(listener);
        while !held.is_empty() {
            std::thread::park();
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

/// The first line within `t` whose type is one of `types`; None if none came.
fn first_of(rx: &mpsc::Receiver<Value>, types: &[&str], t: Duration) -> Option<Value> {
    let end = Instant::now() + t;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(v) if types.contains(&v["type"].as_str().unwrap_or("")) => return Some(v),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
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

/// ask_user whose arguments hold no usable question (empty, null, or JSON that
/// does not parse, here a raw newline inside the string) is refused back to the
/// model. Asked, it was a question with no text that sat unseen under
/// "working…" until someone happened to type. A batch whose arguments do not
/// parse is refused with the parse error too, not as a batch with no calls.
#[test]
fn ask_user_without_a_usable_question_is_refused_not_asked() {
    let cases = [("ask_user", "", "non-empty"), ("ask_user", "null", "non-empty"),
        ("ask_user", "{\"question\": \"push where?\nmain or feature\"}", "not valid JSON"),
        ("batch", "{\"calls\": [{\"name\": \"ask_user\", \"args\": {\"question\": \"a\nb\"}}]}", "not valid JSON")];
    for (name, raw, why) in cases {
        let call = sse(json!({"tool_calls": [{"index": 0, "id": "c1", "type": "function",
            "function": {"name": name, "arguments": raw}}]}), "tool_calls");
        let (url, bodies) = fake_llm(vec![call, sse(json!({"content": "ok"}), "stop")]);
        let (mut child, rx) = spawn("rpc_ask_args", &url, &["--rpc"]);
        let mut stdin = child.stdin.take().unwrap();
        until(&rx, "session");
        writeln!(stdin, "{}", json!({"id": 1, "type": "prompt", "text": "git push"})).unwrap();
        let ev = first_of(&rx, &["ask", "task_end"], Duration::from_secs(10)).expect("the run neither asked nor ended");
        assert_eq!(ev["type"], "task_end", "arguments {raw:?}: asked instead of refusing: {ev}");
        drop(stdin);
        assert_eq!(child.wait().unwrap().code(), Some(0));
        let second = bodies.iter().nth(1).expect("the refusal went back to the model");
        let result = second["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(result["role"], "tool");
        assert!(result["content"].as_str().unwrap().contains(why), "arguments {raw:?}: {result}");
    }
}

/// A provider that stops mid-reply and keeps the socket open: abort (Esc in the
/// TUI) must end the run at once, not whenever the next byte arrives.
#[test]
fn abort_ends_a_stalled_stream() {
    let partial = format!("data: {}\n\n", json!({"choices": [{"delta": {"content": "I'll use a shell command instead."}, "finish_reason": null}]}));
    let (url, _) = fake_llm_held(vec![(partial, true)]);
    let (mut child, rx) = spawn("rpc_stall", &url, &["--rpc"]);
    let mut stdin = child.stdin.take().unwrap();
    until(&rx, "session");
    writeln!(stdin, "{}", json!({"id": 1, "type": "prompt", "text": "add the models"})).unwrap();
    until(&rx, "text_delta");
    writeln!(stdin, "{}", json!({"id": 2, "type": "abort"})).unwrap();
    let Some(end) = first_of(&rx, &["task_end"], Duration::from_secs(5)) else {
        let _ = child.kill();
        panic!("abort left the run parked on the silent stream");
    };
    assert_eq!((end["ok"].clone(), end["error"].clone()), (json!(false), json!("interrupted")));
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

/// A reply is over at [DONE], whether or not the server then closes the socket.
#[test]
fn a_done_line_ends_the_reply_without_waiting_for_close() {
    let (url, _) = fake_llm_held(vec![(sse(json!({"content": "All done."}), "stop"), true)]);
    let (mut child, rx) = spawn("json_done_open", &url, &["--json", "hi"]);
    let Some(end) = first_of(&rx, &["task_end"], Duration::from_secs(5)) else {
        let _ = child.kill();
        panic!("the reply was complete but the run waited for the socket to close");
    };
    assert_eq!((end["ok"].clone(), end["answer"].clone()), (json!(true), json!("All done.")));
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

/// A provider error whose body is not ASCII still ends the turn. Cut to 500
/// bytes mid-char for the message, it panicked the agent thread: RPC never sent
/// task_end and stayed busy for good, and --json exited without one.
#[test]
fn a_provider_error_that_is_not_ascii_still_ends_the_turn() {
    let body = json!({"error": {"message": "请求参数错误".repeat(40)}}).to_string();
    let err = format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let (url, _) = fake_llm(vec![err.clone(), sse(json!({"content": "fine"}), "stop"), err]);
    let (mut child, rx) = spawn("rpc_cjk_error", &url, &["--rpc"]);
    let mut stdin = child.stdin.take().unwrap();
    until(&rx, "session");
    writeln!(stdin, "{}", json!({"id": 1, "type": "prompt", "text": "hello"})).unwrap();
    let end = until(&rx, "task_end").pop().unwrap();
    assert_eq!(end["ok"], false);
    assert!(end["error"].as_str().unwrap().starts_with("HTTP 400"), "{end}");
    writeln!(stdin, "{}", json!({"id": 2, "type": "prompt", "text": "again"})).unwrap();
    assert_eq!(until(&rx, "task_end").pop().unwrap()["ok"], true, "the session is not left busy");
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));

    let (mut child, rx) = spawn("json_cjk_error", &url, &["--json", "hello"]);
    let end = until(&rx, "task_end").pop().unwrap();
    assert!(end["ok"] == false && end["error"].as_str().unwrap().starts_with("HTTP 400"), "{end}");
    assert_eq!(child.wait().unwrap().code(), Some(1));
}

/// abort (Esc in the TUI) kills a command the model is running, rather than
/// waiting out its timeout_secs.
#[test]
fn abort_kills_a_running_command() {
    let call = sse(json!({"tool_calls": [{"index": 0, "id": "c1", "type": "function",
        "function": {"name": "run_command", "arguments": "{\"command\":\"sleep 30\",\"timeout_secs\":60}"}}]}), "tool_calls");
    let (url, _) = fake_llm(vec![call, sse(json!({"content": "stopped"}), "stop")]);
    let (mut child, rx) = spawn("rpc_abort_cmd", &url, &["--rpc", "--yolo"]);
    let mut stdin = child.stdin.take().unwrap();
    until(&rx, "session");
    writeln!(stdin, "{}", json!({"id": 1, "type": "prompt", "text": "wait a bit"})).unwrap();
    until(&rx, "tool_start");
    let t0 = Instant::now();
    writeln!(stdin, "{}", json!({"id": 2, "type": "abort"})).unwrap();
    let Some(end) = first_of(&rx, &["task_end"], Duration::from_secs(10)) else {
        let _ = child.kill();
        panic!("abort waited for the command");
    };
    assert_eq!(end["error"], "interrupted");
    assert!(t0.elapsed() < Duration::from_secs(5), "ended after {:?}", t0.elapsed());
    drop(stdin);
    assert_eq!(child.wait().unwrap().code(), Some(0));
}

/// Run rusti with `args` in a fresh folder and an empty home, feed it `input`
/// on stdin, and return its exit code (None if it was still running after 15 s,
/// then killed) and its stderr.
fn exit_of(name: &str, args: &[&str], input: Option<&str>) -> (Option<i32>, String) {
    let dir = std::env::temp_dir().join(format!("rusti_{name}_exit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rusti"))
        .args(args)
        .current_dir(&dir)
        .env("RUSTI_HOME", dir.join("home"))
        .env_remove("LLM_URL")
        .env_remove("RUSTI_YOLO")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = writeln!(stdin, "{text}");
        std::mem::forget(stdin); // held open: a reader of stdin would wait, not see EOF
    }
    let mut err = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let end = Instant::now() + Duration::from_secs(15);
    let code = loop {
        match child.try_wait().unwrap() {
            Some(st) => break st.code(),
            None if Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    (code, reader.join().unwrap())
}

/// --tui on a stdin that is not a terminal says so and exits. It fell into a
/// plain mode that never started a task and waited forever, printing nothing.
#[test]
fn tui_without_a_terminal_exits_with_a_reason() {
    let (code, err) = exit_of("tui_notty", &["--url", "http://127.0.0.1:9/v1/chat/completions", "--model", "m", "--tui"], None);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("interactive terminal"), "{err}");
}

/// With no saved model, --rpc and --json stop with the reason instead of asking
/// for a profile on stdin. In --rpc stdin is the command channel: the client's
/// first command was read as the profile's name, and both sides then waited.
#[test]
fn rpc_and_json_without_a_model_exit_instead_of_asking() {
    let prompt = json!({"id": 1, "type": "prompt", "text": "hello"}).to_string();
    for args in [&["--rpc"][..], &["--json", "hello"][..]] {
        let (code, err) = exit_of("no_model", args, Some(&prompt));
        assert_eq!(code, Some(1), "{args:?}: {err}");
        assert!(err.contains("no model configured") && !err.contains("add one"), "{args:?}: {err}");
    }
}

/// A background job runs in a process group of its own, out of the terminal's
/// reach, so closing the terminal no longer killed it. It dies with rusti
/// instead: when rusti exits, and when it is told to stop (SIGTERM).
#[cfg(unix)]
#[test]
fn a_background_job_dies_with_rusti() {
    for how in ["exit", "SIGTERM"] {
        let call = sse(json!({"tool_calls": [{"index": 0, "id": "c1", "type": "function",
            "function": {"name": "run_background", "arguments": json!({"command": "echo $$ > job.pid; exec sleep 300"}).to_string()}}]}), "tool_calls");
        let (url, _) = fake_llm(vec![call, sse(json!({"content": "started"}), "stop")]);
        let name = "rpc_job_dies";
        let (mut child, rx) = spawn(name, &url, &["--rpc", "--yolo"]);
        let pidfile = std::env::temp_dir().join(format!("rusti_{name}_e2e_{}", std::process::id())).join("job.pid");
        let mut stdin = child.stdin.take().unwrap();
        until(&rx, "session");
        writeln!(stdin, "{}", json!({"id": 1, "type": "prompt", "text": "start the server"})).unwrap();
        until(&rx, "task_end");
        let end = Instant::now() + Duration::from_secs(10);
        let pid = loop {
            if let Some(p) = std::fs::read_to_string(&pidfile).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
                break p;
            }
            assert!(Instant::now() < end, "{how}: the job never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        if how == "SIGTERM" {
            Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
        } else {
            drop(stdin);
        }
        child.wait().unwrap();
        let alive = || Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success();
        let end = Instant::now() + Duration::from_secs(5);
        while alive() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        if alive() {
            let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            panic!("{how}: the job outlived rusti");
        }
    }
}
