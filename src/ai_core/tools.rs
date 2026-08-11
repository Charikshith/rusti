// Tools the agent can call. Each returns a string the LLM sees as the result.
// ponytail: no command timeout — a hung command hangs the agent. Add a kill
// timer (std has none; spawn a watcher thread) when you hit a real hang.

pub const MAX_RESULT: usize = 20_000; // cap tool output so the conversation stays small

pub fn read_file(path: &str) -> (bool, String) {
    match std::fs::read_to_string(path) {
        Ok(s) => (true, truncate(&s, MAX_RESULT)),
        Err(e) => (false, format!("error reading {path}: {e}")),
    }
}

pub fn write_file(path: &str, content: &str) -> (bool, String) {
    match std::fs::write(path, content) {
        Ok(()) => (true, format!("wrote {} bytes to {path}", content.len())),
        Err(e) => (false, format!("error writing {path}: {e}")),
    }
}

pub fn run_command(cmd: &str) -> (bool, String) {
    let out = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", cmd]).output()
    } else {
        std::process::Command::new("sh").args(["-c", cmd]).output()
    };
    match out {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            let mut s = format!("[exit {code}]\n");
            s.push_str(&String::from_utf8_lossy(&o.stdout));
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (code == 0, truncate(&s, MAX_RESULT))
        }
        Err(e) => (false, format!("failed to run command: {e}")),
    }
}

pub fn edit_file(path: &str, old_text: &str, new_text: &str) -> (bool, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return (false, format!("error reading {path}: {e}")),
    };
    let count = content.matches(old_text).count();
    if count == 0 {
        return (false, format!("edit failed: old text not found in {path}"));
    }
    if count > 1 {
        return (false, format!("edit failed: old text appears {count} times in {path}; make it unique"));
    }
    let new = content.replace(old_text, new_text);
    match std::fs::write(path, &new) {
        Ok(()) => (true, format!("edited {path}: {} chars -> {} chars", old_text.len(), new_text.len())),
        Err(e) => (false, format!("error writing {path}: {e}")),
    }
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

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}\n…[truncated]", &s[..n])
    }
}
