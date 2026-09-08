// rustypi: minimal coding agent in Rust. CLI front end —
// all agent logic lives in the ai_core module. Model details persist in
// model.json (see config.rs); the CLI picks the saved profile, or asks to
// add one when none exists. Flags/env override the saved profile.
//   cargo run --release -- "add a --version flag to src/main.rs"
//   cargo run --release -- --list | --use NAME | --add

mod ai_core;
mod config;
mod session;
mod tree;
mod tui;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--self-test") {
        ai_core::self_test();
        return;
    }
    if args.iter().any(|a| a == "--list") {
        let cfg = config::Config::load();
        if cfg.models.is_empty() {
            println!("no saved models (launch with a task to add one, or --add)");
            return;
        }
        for m in &cfg.models {
            let star = if Some(&m.name) == cfg.default.as_ref() { "* " } else { "  " };
            let key = if m.key.is_empty() { "-" } else { "***" };
            println!("{star}{}  {}  model={}  key={key}", m.name, m.url, m.model);
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--use") {
        let name = args.get(i + 1).cloned().unwrap_or_default();
        let mut cfg = config::Config::load();
        if !cfg.models.iter().any(|m| m.name == name) {
            eprintln!("no saved model named '{name}' (see --list)");
            std::process::exit(1);
        }
        cfg.default = Some(name);
        cfg.save().unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1); });
        println!("default model set");
        return;
    }
    if args.iter().any(|a| a == "--add") {
        match config::ask_profile() {
            Some(p) => {
                let mut cfg = config::Config::load();
                cfg.default = Some(p.name.clone());
                cfg.add(p);
                cfg.save().unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1); });
                eprintln!("saved to model.json");
            }
            None => eprintln!("aborted"),
        }
        return;
    }

    let (url, key, model) = resolve_model(&args);
    let task_arg = args.iter().find(|a| !a.starts_with('-')).cloned();
    let tui_mode = args.iter().any(|a| a == "--tui");

    // session + task first (so --tree can be cancelled before any model config)
    let mut session;
    let task: String;
    if args.iter().any(|a| a == "--tree") {
        session = session::Session::load();
        if session.is_empty() {
            eprintln!("no session.json to browse (run a task first)");
            std::process::exit(1);
        }
        match tree::browse(&mut session) {
            Some(prefill) => task = task_arg.unwrap_or_else(|| prompt("next instruction", &prefill)),
            None => return,
        }
    } else if args.iter().any(|a| a == "--resume") {
        session = session::Session::load();
        if session.is_empty() {
            eprintln!("no session.json to resume (run a task first)");
            std::process::exit(1);
        }
        // tui mode never uses `task` (TuiConfig doesn't take one) — skip the
        // blocking stdin prompt so `--tui --resume` (incl. /reload's relaunch)
        // doesn't stall before the TUI even starts
        task = if tui_mode { String::new() } else { task_arg.unwrap_or_else(|| prompt("next instruction", "")) };
    } else {
        session = session::Session::new(model.clone());
        task = if tui_mode { String::new() } else { task_arg.unwrap_or_else(|| "say hello".into()) };
    }

    let client = ai_core::llm::Client::new(url, key, model.clone());

    if tui_mode {
        if let Err(e) = tui::run(tui::TuiConfig { client, session, model, cli_args: args.clone() }) {
            eprintln!("tui error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match rt.block_on(ai_core::run_agent(&client, &mut session, &task, &cancel)) {
        // reply text was already streamed live; just close the line
        Ok(r) => {
            if !r.is_empty() {
                println!();
            }
        }
        Err(e) => {
            eprintln!("\nerror: {e}");
            std::process::exit(1);
        }
    }
}

fn get(args: &[String], flag: &str, env: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var(env).ok())
}

/// Simple stdin prompt with a default kept on empty input.
fn prompt(label: &str, default: &str) -> String {
    use std::io::Write;
    let suffix = if default.is_empty() { String::new() } else { format!(" [{default}]") };
    eprint!("{label}{suffix} > ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    let s = line.trim().to_string();
    if s.is_empty() { default.to_string() } else { s }
}

/// Saved profile first; explicit flags/env override per field. When nothing
/// is saved and no --url is given, ask interactively and persist the answer.
fn resolve_model(args: &[String]) -> (String, String, String) {
    let explicit_url = get(args, "--url", "LLM_URL");
    let explicit_key = get(args, "--key", "LLM_KEY");
    let explicit_model = get(args, "--model", "LLM_MODEL");

    let mut cfg = config::Config::load();
    let mut prof = cfg.resolve().cloned().unwrap_or_default();
    if prof.url.is_empty() && explicit_url.is_none() {
        let p = match config::ask_profile() {
            Some(p) => p,
            None => {
                eprintln!("no input, aborting");
                std::process::exit(1);
            }
        };
        cfg.default = Some(p.name.clone());
        cfg.add(p.clone());
        prof = p;
        cfg.save().unwrap_or_else(|e| eprintln!("warning: could not save model.json: {e}"));
    }

    let url = explicit_url.unwrap_or(prof.url);
    let key = explicit_key.unwrap_or(prof.key);
    let model = explicit_model.unwrap_or(if prof.model.is_empty() { "gpt-4o-mini".into() } else { prof.model });
    (url, key, model)
}
