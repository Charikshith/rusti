// Machine front ends. --json runs one task and writes the agent's events to
// stdout as JSONL; --rpc stays up, reads JSONL commands on stdin and writes
// responses plus the same events to stdout. Logs go to stderr in both, so
// stdout only ever carries protocol lines. The formats are in
// docs/json-rpc.md.

use crate::ai_core::{self, llm, Event};
use crate::session::Session;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Every line carries it as "v". Bumped only when a field changes meaning or
/// goes away; a new field or a new line type is not a break.
pub const VERSION: u64 = 1;

/// What a question gets when nobody can answer it: tools.rs's own wording for
/// an empty answer, which a permission prompt reads as no.
const NO_ANSWER: &str = "no answer given";

/// One event as a protocol line, or None for the TUI-only ones (pickers,
/// prefill, relaunch). An `ask` gets its "id" from the pump that holds its reply.
pub fn event_json(ev: &Event) -> Option<Value> {
    let mut v = match ev {
        Event::TextDelta(t) => json!({"type": "text_delta", "text": t}),
        Event::ReasoningDelta(t) => json!({"type": "thinking_delta", "text": t}),
        Event::Text(t) => json!({"type": "text", "text": t}),
        Event::ToolStart { id, summary } => json!({"type": "tool_start", "id": id, "summary": summary}),
        Event::ToolEnd { id, summary, ok, ms, output } => {
            json!({"type": "tool_end", "id": id, "summary": summary, "ok": ok, "ms": *ms as u64, "output": output})
        }
        Event::Resumed { msg_num, .. } => json!({"type": "resumed", "messages": msg_num}),
        Event::SessionName(n) => json!({"type": "session_name", "name": n}),
        Event::Git(b) => json!({"type": "git", "branch": b}),
        Event::Notice(t) => json!({"type": "notice", "text": t}),
        Event::Retry { attempt, of, wait_ms, err } => {
            json!({"type": "retry", "attempt": attempt, "of": of, "wait_ms": wait_ms, "error": err})
        }
        Event::Usage { tokens, prompt, est, gen_ms, cached } => json!({"type": "usage", "tokens": tokens,
            "prompt": prompt, "estimated": est, "gen_ms": *gen_ms as u64, "cached": cached}),
        Event::Ask { question, choices, .. } => json!({"type": "ask", "question": question,
            "choices": choices.iter().map(|(label, value)| json!({"label": label, "value": value})).collect::<Vec<_>>()}),
        Event::TaskEnd { ok, error } => json!({"type": "task_end", "ok": ok, "error": error}),
        Event::Delivered(t) => json!({"type": "steer_delivered", "text": t}),
        Event::CompactStart { reason, messages, tokens } => {
            json!({"type": "compaction_start", "reason": reason, "messages": messages, "tokens": tokens})
        }
        Event::CompactEnd { summary, tokens } => json!({"type": "compaction_end", "summary": summary, "tokens": tokens}),
        Event::Tree(_) | Event::Prefill(_) | Event::Reload { .. } => return None,
    };
    v["v"] = json!(VERSION);
    Some(v)
}

/// The `session` line: which session the events that follow belong to.
fn session_json(s: &Session, thinking: Option<&str>) -> Value {
    let cwd = std::env::current_dir().map(|d| d.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    json!({"v": VERSION, "type": "session", "name": crate::session::name_of(&s.path), "path": s.path,
           "model": s.model, "messages": s.msgs(), "cwd": cwd, "thinking": thinking})
}

/// Where protocol lines go. Each line is written and flushed under the lock,
/// so the event pump and the command threads never interleave inside one.
#[derive(Clone)]
struct Out(Arc<Mutex<Box<dyn Write + Send>>>);

impl Out {
    fn stdout() -> Out {
        Out(Arc::new(Mutex::new(Box::new(std::io::stdout()))))
    }
    fn send(&self, mut v: Value) {
        v["v"] = json!(VERSION);
        let mut w = self.0.lock().unwrap();
        let _ = writeln!(w, "{v}");
        let _ = w.flush();
    }
}

/// Questions waiting for an RPC `answer`, by the id their `ask` line carried.
#[derive(Default)]
struct Asks {
    next: AtomicU64,
    open: Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>,
    closed: AtomicBool, // nobody left to answer: stdin is gone
    cancel: Arc<AtomicBool>, // the run was aborted: nobody will answer it
}

impl Asks {
    /// Answer every open question with NO_ANSWER; with `close`, every later one too.
    fn drop_all(&self, close: bool) {
        let mut open = self.open.lock().unwrap();
        if close {
            self.closed.store(true, Ordering::Relaxed);
        }
        for (_, reply) in open.drain() {
            let _ = reply.send(NO_ANSWER.into());
        }
    }
}

/// Write agent events to `out` until `stop` is set and nothing is left. An
/// `ask` is held in `asks` for an `answer` (RPC); with no `asks` (--json) or
/// after stdin closed or an abort, it is answered NO_ANSWER at once.
fn pump(rx: mpsc::Receiver<Event>, out: Out, asks: Option<Arc<Asks>>, stop: Arc<AtomicBool>) {
    loop {
        let ev = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ev) => ev,
            Err(mpsc::RecvTimeoutError::Timeout) if !stop.load(Ordering::Relaxed) => continue,
            Err(_) => return,
        };
        let Some(mut v) = event_json(&ev) else { continue };
        if let Event::Ask { reply, .. } = ev {
            match &asks {
                Some(a) => {
                    let id = format!("q{}", a.next.fetch_add(1, Ordering::Relaxed) + 1);
                    v["id"] = json!(id);
                    // held before the line goes out, so an instant answer finds it
                    let mut open = a.open.lock().unwrap();
                    match a.closed.load(Ordering::Relaxed) || a.cancel.load(Ordering::Relaxed) {
                        true => drop(reply.send(NO_ANSWER.into())),
                        false => drop(open.insert(id, reply)),
                    }
                }
                None => drop(reply.send(NO_ANSWER.into())),
            }
        }
        out.send(v);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// --json: one task, its events on stdout, then a last `task_end` line that
/// adds the final answer. Returns the exit code.
pub fn run_json(client: llm::Client, mut session: Session, task: &str) -> i32 {
    let (tx, rx) = mpsc::channel();
    ai_core::set_event_sink(tx);
    let out = Out::stdout();
    out.send(session_json(&session, None));
    let stop = Arc::new(AtomicBool::new(false));
    let pumping = {
        let (out, stop) = (out.clone(), stop.clone());
        std::thread::spawn(move || pump(rx, out, None, stop))
    };
    let r = runtime().block_on(ai_core::run_agent(&client, &mut session, task, &crate::tui::image_refs(task), &AtomicBool::new(false)));
    stop.store(true, Ordering::Relaxed);
    let _ = pumping.join(); // every event of the run is out before task_end
    match r {
        Ok(answer) => {
            out.send(json!({"type": "task_end", "ok": true, "error": null, "answer": answer}));
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            out.send(json!({"type": "task_end", "ok": false, "error": e, "answer": null}));
            1
        }
    }
}

/// What the agent thread does for a command. Everything here owns the
/// session or the client, so it waits its turn there.
enum Work {
    Prompt(String),
    New(Option<String>),
    Resume(Option<String>),
    Model(String),
    Thinking(Option<String>),
    Compact(Option<String>),
}

struct Job {
    id: Value,
    command: String,
    work: Work,
}

/// Whether the agent thread is working, and the follow-ups that run after it.
/// One lock for both: a prompt that finds it busy queues, and the agent
/// thread sees that queue before it goes idle, never after.
#[derive(Default)]
struct Queue {
    busy: bool,
    follow: VecDeque<String>,
}

/// The state the stdin thread and the agent thread share.
struct Rpc {
    out: Out,
    asks: Arc<Asks>,
    queue: Arc<Mutex<Queue>>,
    info: Arc<Mutex<Value>>, // the last `session` line, for get_state and get_messages
    jobs: mpsc::Sender<Job>,
}

fn response(id: Value, command: &str, r: Result<Value, String>) -> Value {
    match r {
        Ok(data) => json!({"v": VERSION, "type": "response", "id": id, "command": command, "ok": true, "data": data}),
        Err(e) => json!({"v": VERSION, "type": "response", "id": id, "command": command, "ok": false, "error": e}),
    }
}

impl Rpc {
    /// One stdin line. Returns its response, or None when the agent thread
    /// answers it once the work is done.
    fn command(&self, line: &str) -> Option<Value> {
        let c: Value = match serde_json::from_str::<Value>(line) {
            Ok(c) if c.is_object() => c,
            Ok(_) => return Some(response(Value::Null, "", Err("a command is a JSON object".into()))),
            Err(e) => return Some(response(Value::Null, "", Err(format!("bad JSON: {e}")))),
        };
        let id = c["id"].clone();
        let command = c["type"].as_str().unwrap_or("").to_string();
        let text = |k: &str| c[k].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let work = match command.as_str() {
            "new_session" => Work::New(text("name")),
            "resume_session" => Work::Resume(text("name")),
            "set_model" => match text("name") {
                Some(n) => Work::Model(n),
                None => return Some(response(id, &command, Err("set_model needs \"name\": a saved profile".into()))),
            },
            "set_thinking" => Work::Thinking(text("level")),
            "compact" => Work::Compact(text("instructions")),
            _ => {
                let r = match command.as_str() {
                    "prompt" => self.prompt(text("text"), c["mode"].as_str()),
                    "abort" => Ok(self.abort()),
                    "answer" => self.answer(text("ask"), c["text"].as_str().unwrap_or("")),
                    "get_state" => Ok(self.state()),
                    "get_messages" => {
                        let path = self.info.lock().unwrap()["path"].as_str().unwrap_or("").to_string();
                        Ok(json!({"messages": Session::load_from(&path).path_messages()}))
                    }
                    "set_plan" => match c["on"].as_bool() {
                        Some(on) => {
                            ai_core::set_plan(on);
                            Ok(json!({"plan": on}))
                        }
                        None => Err("set_plan needs \"on\": true or false".into()),
                    },
                    "" => Err("missing \"type\"".into()),
                    other => Err(format!("unknown command: {other}")),
                };
                return Some(response(id, &command, r));
            }
        };
        let mut q = self.queue.lock().unwrap();
        if q.busy {
            return Some(response(id, &command, Err("a task is running: wait for task_end, or abort".into())));
        }
        q.busy = true;
        self.asks.cancel.store(false, Ordering::Relaxed);
        let _ = self.jobs.send(Job { id, command, work });
        None
    }

    /// Idle: run it. Busy: steer it into the running task, or queue it as a
    /// follow-up that runs on its own after; the client says which.
    fn prompt(&self, text: Option<String>, mode: Option<&str>) -> Result<Value, String> {
        let text = text.ok_or("prompt needs \"text\"")?;
        let mut q = self.queue.lock().unwrap();
        let how = match (q.busy, mode) {
            (false, _) => {
                q.busy = true;
                self.asks.cancel.store(false, Ordering::Relaxed);
                let _ = self.jobs.send(Job { id: Value::Null, command: "prompt".into(), work: Work::Prompt(text) });
                "run"
            }
            (true, Some("steer")) => {
                let images = crate::tui::image_refs(&text);
                ai_core::steer(text, images);
                "steer"
            }
            (true, Some("follow_up")) => {
                q.follow.push_back(text);
                "follow_up"
            }
            (true, None) => return Err("a task is running: send \"mode\": \"steer\" or \"follow_up\"".into()),
            (true, Some(m)) => return Err(format!("unknown mode '{m}': steer or follow_up")),
        };
        Ok(json!({"accepted": how}))
    }

    /// Stop the running task and drop what was queued behind it. A question
    /// it was waiting on is answered no, so the run can unwind.
    fn abort(&self) -> Value {
        let mut q = self.queue.lock().unwrap();
        self.asks.cancel.store(true, Ordering::Relaxed);
        let dropped = ai_core::take_steers().len() + std::mem::take(&mut q.follow).len();
        drop(q);
        self.asks.drop_all(false);
        json!({"dropped": dropped})
    }

    fn answer(&self, ask: Option<String>, text: &str) -> Result<Value, String> {
        let ask = ask.ok_or("answer needs \"ask\": the id of an ask line")?;
        let reply = self.asks.open.lock().unwrap().remove(&ask).ok_or(format!("no open question {ask}"))?;
        let _ = reply.send(if text.trim().is_empty() { NO_ANSWER.into() } else { text.to_string() });
        Ok(json!({}))
    }

    fn state(&self) -> Value {
        let q = self.queue.lock().unwrap();
        let mut asks: Vec<String> = self.asks.open.lock().unwrap().keys().cloned().collect();
        asks.sort();
        let mut session = self.info.lock().unwrap().clone();
        let thinking = session.as_object_mut().and_then(|o| o.remove("thinking")).unwrap_or(Value::Null);
        for k in ["v", "type", "cwd"] {
            session.as_object_mut().map(|o| o.remove(k));
        }
        json!({"busy": q.busy, "session": session, "thinking": thinking, "plan": ai_core::plan_mode(),
               "context_limit": ai_core::context_limit(), "steers": ai_core::steers().len(),
               "follow_ups": q.follow.len(), "asks": asks})
    }
}

/// The agent thread: owns the session and the client, does one Job at a
/// time, then runs whatever was queued meanwhile before going idle: leftover
/// steers first, all at once, then follow-ups one at a time.
fn agent(rx: mpsc::Receiver<Job>, mut client: llm::Client, mut session: Session, out: Out, cancel: Arc<AtomicBool>,
         queue: Arc<Mutex<Queue>>, info: Arc<Mutex<Value>>) {
    let rt = runtime();
    let run = |client: &llm::Client, session: &mut Session, task: String| {
        let images = crate::tui::image_refs(&task);
        let ev = match rt.block_on(ai_core::run_agent(client, session, &task, &images, &cancel)) {
            Ok(_) => Event::TaskEnd { ok: true, error: None },
            Err(e) => Event::TaskEnd { ok: false, error: Some(e) },
        };
        *info.lock().unwrap() = session_json(session, client.reasoning.as_deref());
        ev
    };
    for job in rx {
        let mut end = None;
        let r = match job.work {
            Work::Prompt(task) => {
                end = Some(run(&client, &mut session, task));
                None
            }
            Work::New(name) => Some(crate::tui::new_session(&mut session, name.as_deref()).map(|n| json!({"name": n}))),
            Work::Resume(name) => Some(match name.map(|n| crate::session::path_for(&n)).or_else(crate::session::latest) {
                None => Err("no saved session to resume".into()),
                Some(path) => {
                    let loaded = Session::load_from(&path);
                    if loaded.is_empty() {
                        Err(format!("nothing to resume in {path}"))
                    } else {
                        session = loaded;
                        ai_core::tools::undo_begin_turn(); // the last turn's files belong to the session left
                        Ok(json!({"name": crate::session::name_of(&path), "messages": session.msgs()}))
                    }
                }
            }),
            Work::Model(name) => Some(match crate::config::Config::load().models.into_iter().find(|m| m.name == name) {
                None => Err(format!("no saved model named '{name}'")),
                Some(p) => {
                    ai_core::tools::set_vision(p.vision != Some(false));
                    ai_core::use_profile_context(p.context);
                    let reasoning = client.reasoning.take();
                    client = llm::Client::new(p.url, p.key, p.model.clone());
                    client.reasoning = reasoning;
                    client.cache = p.cache;
                    session.model = p.model.clone();
                    Ok(json!({"model": p.model, "context_limit": ai_core::context_limit()}))
                }
            }),
            Work::Thinking(level) => {
                client.reasoning = level; // none or "" clears it: the provider's default
                Some(Ok(json!({"thinking": client.reasoning})))
            }
            Work::Compact(focus) => {
                Some(rt.block_on(ai_core::compact(&client, &mut session, &cancel, "manual", focus.as_deref()))
                    .map(|done| json!({"compacted": done})))
            }
        };
        let line = session_json(&session, client.reasoning.as_deref());
        if line != *info.lock().unwrap() {
            *info.lock().unwrap() = line.clone();
            if !matches!(job.command.as_str(), "prompt" | "compact") {
                out.send(line); // a new, resumed or re-modelled session announces itself
            }
        }
        let mut reply = r.map(|r| response(job.id, &job.command, r));
        loop {
            let next = {
                let mut q = queue.lock().unwrap();
                let steers = ai_core::take_steers();
                let next = if steers.is_empty() { q.follow.pop_front() } else { Some(steers.join("\n\n")) };
                match next {
                    Some(_) => cancel.store(false, Ordering::Relaxed),
                    None => q.busy = false,
                }
                next
            };
            if let Some(v) = reply.take() {
                out.send(v);
            }
            if let Some(ev) = end.take() {
                ai_core::emit(ev);
            }
            match next {
                Some(task) => end = Some(run(&client, &mut session, task)),
                None => break,
            }
        }
    }
}

/// --rpc: serve commands from stdin until it closes, then finish the running
/// task and the queue and exit. Returns the exit code.
pub fn run_rpc(client: llm::Client, session: Session) -> i32 {
    let (tx, rx) = mpsc::channel();
    ai_core::set_event_sink(tx);
    let out = Out::stdout();
    let header = session_json(&session, None);
    out.send(header.clone());
    let asks = Arc::new(Asks::default());
    let stop = Arc::new(AtomicBool::new(false));
    let pumping = {
        let (out, asks, stop) = (out.clone(), asks.clone(), stop.clone());
        std::thread::spawn(move || pump(rx, out, Some(asks), stop))
    };
    let (jobs, job_rx) = mpsc::channel();
    let rpc = Rpc {
        out: out.clone(),
        asks,
        queue: Arc::new(Mutex::new(Queue::default())),
        info: Arc::new(Mutex::new(header)),
        jobs,
    };
    let working = {
        let (out, cancel, queue, info) = (out.clone(), rpc.asks.cancel.clone(), rpc.queue.clone(), rpc.info.clone());
        std::thread::spawn(move || agent(job_rx, client, session, out, cancel, queue, info))
    };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(r) = rpc.command(&line) {
            rpc.out.send(r);
        }
    }
    // nobody is left to answer a question, so the queue can still finish
    rpc.asks.drop_all(true);
    let Rpc { jobs, .. } = rpc;
    drop(jobs);
    let _ = working.join();
    stop.store(true, Ordering::Relaxed);
    let _ = pumping.join();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every event has one strict, versioned line shape; the TUI-only ones none.
    #[test]
    fn events_serialize_to_versioned_lines() {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let cases = [
            (Event::TextDelta("hi".into()), json!({"v": 1, "type": "text_delta", "text": "hi"})),
            (Event::ReasoningDelta("hm".into()), json!({"v": 1, "type": "thinking_delta", "text": "hm"})),
            (Event::ToolStart { id: 4, summary: "read a.rs".into() }, json!({"v": 1, "type": "tool_start", "id": 4, "summary": "read a.rs"})),
            (
                Event::ToolEnd { id: 4, summary: "run cargo test".into(), ok: false, ms: 1500, output: "[exit 101]".into() },
                json!({"v": 1, "type": "tool_end", "id": 4, "summary": "run cargo test", "ok": false, "ms": 1500, "output": "[exit 101]"}),
            ),
            (
                Event::Usage { tokens: 7, prompt: 100, est: false, gen_ms: 30, cached: None },
                json!({"v": 1, "type": "usage", "tokens": 7, "prompt": 100, "estimated": false, "gen_ms": 30, "cached": null}),
            ),
            (
                Event::Retry { attempt: 1, of: 3, wait_ms: 1000, err: "HTTP 503".into() },
                json!({"v": 1, "type": "retry", "attempt": 1, "of": 3, "wait_ms": 1000, "error": "HTTP 503"}),
            ),
            (
                Event::Ask { question: "allow run_command?".into(), choices: vec![("Yes".into(), "yes".into())], reply: tx },
                json!({"v": 1, "type": "ask", "question": "allow run_command?", "choices": [{"label": "Yes", "value": "yes"}]}),
            ),
            (Event::TaskEnd { ok: false, error: Some("interrupted".into()) }, json!({"v": 1, "type": "task_end", "ok": false, "error": "interrupted"})),
            (Event::Delivered("use tabs".into()), json!({"v": 1, "type": "steer_delivered", "text": "use tabs"})),
            (
                Event::CompactStart { reason: "overflow".into(), messages: 12, tokens: 90_000 },
                json!({"v": 1, "type": "compaction_start", "reason": "overflow", "messages": 12, "tokens": 90_000}),
            ),
            (Event::CompactEnd { summary: "## Goal".into(), tokens: 21_000 }, json!({"v": 1, "type": "compaction_end", "summary": "## Goal", "tokens": 21_000})),
        ];
        for (ev, want) in cases {
            let got = event_json(&ev).unwrap();
            assert_eq!(got, want);
            assert!(!got.to_string().contains('\n'), "one event, one line");
        }
        assert!(event_json(&Event::Prefill("x".into())).is_none() && event_json(&Event::Tree(vec![])).is_none());
    }

    /// An Rpc wired to a buffer and a job channel the test reads, no agent thread.
    fn rpc() -> (Rpc, mpsc::Receiver<Job>) {
        let (jobs, rx) = mpsc::channel();
        let info = json!({"v": 1, "type": "session", "name": "s", "path": "_rpc_none.json", "model": "m", "messages": 0, "cwd": ".", "thinking": null});
        let r = Rpc {
            out: Out(Arc::new(Mutex::new(Box::new(Vec::new())))),
            asks: Arc::new(Asks::default()),
            queue: Arc::new(Mutex::new(Queue::default())),
            info: Arc::new(Mutex::new(info)),
            jobs,
        };
        (r, rx)
    }

    #[test]
    fn rpc_commands_answer_by_id_and_queue_while_busy() {
        let _q = ai_core::STEER_TEST.lock().unwrap_or_else(|e| e.into_inner());
        ai_core::take_steers();
        let (r, jobs) = rpc();
        let err = |v: Value| v["error"].as_str().unwrap().to_string();

        let bad = r.command("{not json").unwrap();
        assert!(bad["id"].is_null() && bad["ok"] == false && err(bad).starts_with("bad JSON"));
        assert_eq!(err(r.command(r#"{"id":1,"type":"fly"}"#).unwrap()), "unknown command: fly");
        assert_eq!(err(r.command(r#"{"id":1,"type":"prompt"}"#).unwrap()), "prompt needs \"text\"");

        // idle: the prompt runs, and the response echoes the id whatever its type
        let ok = r.command(r#"{"id":"a1","type":"prompt","text":"fix it"}"#).unwrap();
        assert_eq!(ok, json!({"v": 1, "type": "response", "id": "a1", "command": "prompt", "ok": true, "data": {"accepted": "run"}}));
        assert!(matches!(jobs.try_recv().unwrap().work, Work::Prompt(t) if t == "fix it"));

        // busy: the client must say steer or follow-up
        assert!(err(r.command(r#"{"id":2,"type":"prompt","text":"more"}"#).unwrap()).contains("steer"));
        let s = r.command(r#"{"id":3,"type":"prompt","text":"use tabs","mode":"steer"}"#).unwrap();
        assert_eq!(s["data"]["accepted"], "steer");
        let f = r.command(r#"{"id":4,"type":"prompt","text":"then docs","mode":"follow_up"}"#).unwrap();
        assert_eq!(f["data"]["accepted"], "follow_up");
        assert!(jobs.try_recv().is_err(), "neither starts a run of its own");
        let st = r.command(r#"{"id":5,"type":"get_state"}"#).unwrap()["data"].clone();
        assert_eq!((st["busy"].clone(), st["steers"].clone(), st["follow_ups"].clone()), (json!(true), json!(1), json!(1)));
        assert_eq!(st["session"], json!({"name": "s", "path": "_rpc_none.json", "model": "m", "messages": 0}));

        // session work waits for idle rather than racing the running task
        assert!(err(r.command(r#"{"id":6,"type":"new_session"}"#).unwrap()).contains("a task is running"));
        assert_eq!(err(r.command(r#"{"id":7,"type":"set_model"}"#).unwrap()), "set_model needs \"name\": a saved profile");

        // a question: answered by its ask id; an unknown one is an error
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        r.asks.open.lock().unwrap().insert("q1".into(), tx);
        assert_eq!(r.command(r#"{"id":8,"type":"get_state"}"#).unwrap()["data"]["asks"], json!(["q1"]));
        assert!(r.command(r#"{"id":9,"type":"answer","ask":"q1","text":"yes"}"#).unwrap()["ok"] == true);
        assert_eq!(rx.try_recv().unwrap(), "yes");
        assert_eq!(err(r.command(r#"{"id":10,"type":"answer","ask":"q1","text":"yes"}"#).unwrap()), "no open question q1");

        // abort: cancels, drops the queue, and says no to a pending question
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        r.asks.open.lock().unwrap().insert("q2".into(), tx);
        assert_eq!(r.command(r#"{"id":11,"type":"abort"}"#).unwrap()["data"], json!({"dropped": 2}));
        assert!(r.asks.cancel.load(Ordering::Relaxed) && ai_core::steers().is_empty());
        assert_eq!(rx.try_recv().unwrap(), NO_ANSWER);

        // idle again: session work goes to the agent thread, which answers it
        r.queue.lock().unwrap().busy = false;
        assert!(r.command(r#"{"id":12,"type":"compact","instructions":"keep the API notes"}"#).is_none());
        let j = jobs.try_recv().unwrap();
        assert_eq!((j.id, j.command.as_str()), (json!(12), "compact"));
        assert!(matches!(j.work, Work::Compact(Some(f)) if f == "keep the API notes"));
    }

    /// --json and an RPC client after stdin closed: a question is answered
    /// "no answer given" at once (a permission prompt reads that as no), and
    /// the ask line still goes out so the consumer sees what was refused.
    #[test]
    fn unanswerable_questions_are_refused_not_hung() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        struct W(Arc<Mutex<Vec<u8>>>);
        impl Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out = Out(Arc::new(Mutex::new(Box::new(W(buf.clone())))));
        let asks = Arc::new(Asks::default());
        asks.drop_all(true);
        for held in [None, Some(asks)] {
            let (tx, rx) = mpsc::channel();
            let (reply, mut answer) = tokio::sync::oneshot::channel();
            tx.send(Event::Ask { question: "allow write_file x?".into(), choices: vec![], reply }).unwrap();
            pump(rx, out.clone(), held, Arc::new(AtomicBool::new(true)));
            assert_eq!(answer.try_recv().unwrap(), NO_ANSWER);
        }
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l["type"] == "ask" && l["v"] == 1));
    }

    /// A command's response is its end signal: by the time a client reads
    /// it, the agent is idle and a plain prompt is accepted.
    #[test]
    fn agent_is_idle_before_a_command_response_goes_out() {
        let _q = ai_core::STEER_TEST.lock().unwrap_or_else(|e| e.into_inner());
        ai_core::take_steers();
        struct W(Vec<u8>, Arc<Mutex<Queue>>, Arc<Mutex<Vec<bool>>>);
        impl Write for W {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.write(b)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                let line = String::from_utf8_lossy(&self.0).lines().last().unwrap_or("").to_string();
                if line.contains("\"type\":\"response\"") {
                    self.2.lock().unwrap().push(self.1.lock().unwrap().busy);
                }
                Ok(())
            }
        }
        let (mut r, jobs) = rpc();
        let seen = Arc::new(Mutex::new(Vec::new()));
        r.out = Out(Arc::new(Mutex::new(Box::new(W(Vec::new(), r.queue.clone(), seen.clone())))));
        assert!(r.command(r#"{"id":1,"type":"set_thinking","level":"high"}"#).is_none());
        let (out, cancel, queue, info) = (r.out.clone(), r.asks.cancel.clone(), r.queue.clone(), r.info.clone());
        drop(r);
        let client = llm::Client::new("http://127.0.0.1:9".into(), String::new(), "m".into());
        agent(jobs, client, Session::new("m".into()), out, cancel, queue, info);
        assert_eq!(*seen.lock().unwrap(), vec![false]);
    }

    /// A question the aborted run asked before the pump saw it, after the
    /// abort found nothing open, is answered no rather than left hanging.
    #[test]
    fn a_question_reaching_the_pump_after_abort_is_refused() {
        let _q = ai_core::STEER_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let (r, _jobs) = rpc();
        assert_eq!(r.command(r#"{"id":1,"type":"prompt","text":"go"}"#).unwrap()["data"]["accepted"], "run");
        let (tx, rx) = mpsc::channel();
        let (reply, mut answer) = tokio::sync::oneshot::channel();
        tx.send(Event::Ask { question: "allow run_command?".into(), choices: vec![], reply }).unwrap();
        assert_eq!(r.command(r#"{"id":2,"type":"abort"}"#).unwrap()["data"], json!({"dropped": 0}));
        pump(rx, r.out.clone(), Some(r.asks.clone()), Arc::new(AtomicBool::new(true)));
        assert_eq!(answer.try_recv().unwrap(), NO_ANSWER);
        assert!(r.asks.open.lock().unwrap().is_empty());
    }
}
