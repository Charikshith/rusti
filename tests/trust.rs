// A1 end to end: a one-shot run in a folder whose model.json names an MCP
// server must not start it unless --trust says so, and --yolo is not --trust.
// connect_all reports every server it tries on stderr, so "mcp evil" there
// means the gated key was applied.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// stderr up to the first LLM retry (the unreachable URL), then kill: the
/// trust decision and MCP startup are both done by then.
fn stderr_of(dir: &std::path::Path, extra: &[&str]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rusti"))
        .args(["--url", "http://127.0.0.1:1/v1/chat/completions", "--model", "m"])
        .args(extra)
        .arg("say hi")
        .current_dir(dir)
        .env("RUSTI_HOME", dir.join("home"))
        .env_remove("RUSTI_YOLO")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = String::new();
    for line in BufReader::new(child.stderr.take().unwrap()).lines() {
        let line = line.unwrap();
        out.push_str(&line);
        out.push('\n');
        if line.contains("retry") || line.contains("error") {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    out
}

#[test]
fn one_shot_runs_ignore_an_untrusted_model_json() {
    let dir = std::env::temp_dir().join(format!("rusti_trust_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.json"), r#"{"mcp":{"evil":{"command":"rusti-no-such-command"}},"allow":["run_command"]}"#).unwrap();

    for extra in [&[][..], &["--yolo"][..]] {
        let err = stderr_of(&dir, extra);
        assert!(err.contains("untrusted folder: ignoring mcp, allow"), "{extra:?}: one warning line\n{err}");
        assert!(!err.contains("mcp evil"), "{extra:?}: the project's MCP server must not start\n{err}");
    }
    let err = stderr_of(&dir, &["--trust"]);
    assert!(err.contains("mcp evil") && !err.contains("untrusted"), "--trust applies the project's keys\n{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
