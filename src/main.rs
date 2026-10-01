// rusti: minimal coding agent in Rust. CLI front end —
// all agent logic lives in the ai_core module. Model details persist in
// model.json (see config.rs); the CLI picks the saved profile, or asks to
// add one when none exists. Flags/env override the saved profile.
//   cargo run --release -- "add a --version flag to src/main.rs"
//   cargo run --release -- --list | --use NAME | --add
//   cargo run --release -- --session NAME "task"   (named session file)

mod ai_core;
mod config;
mod session;
mod tree;
mod tui;

const HELP: &str = "\
rusti — minimal coding agent

usage: rusti [flags]              interactive terminal UI
       rusti [flags] TASK         one-shot: run the task, print the answer, exit

flags
  --tui                  force the UI (bare `rusti` in a terminal already opens it)
  --session NAME         use .rusti/sessions/NAME.json instead of ./session.json
  --resume               continue the session from its active leaf
  --tree                 browse the session tree and branch from an earlier entry
  --url U --key K --model M   override the saved profile for this run
  --use NAME             make a saved profile the default, then exit
  --list                 list saved profiles      --add   add one interactively
  --max-iters N          tool rounds per task (default 50)
  --context N            compact the history once the prompt passes N tokens (default 100000)
  --yolo                 no permission prompts, no project-root guard
  --self-test            offline check against a fake server
  --help, --version

env: LLM_URL LLM_KEY LLM_MODEL RUSTI_SESSION RUSTI_MAX_ITERS RUSTI_CONTEXT RUSTI_YOLO

settings: ~/.rusti/config.json holds models, keys, theme, footer and \"mcp\" for every
project (RUSTI_HOME moves it). ./model.json holds this project's \"allow\" (tools answered
[a]lways, so the next run doesn't ask), \"max_iters\", \"context\"; any other key put there
overrides the global one for this project. Flags and env override both.
\"mcp\" lists MCP servers to start, each with a command, optional args/env,
and \"enabled\". Their tools join the built-in ones as mcp__<server>__<tool>, are always
permission-gated, and are refused in plan mode. Toggle them live with /mcp.
\"shell\" is the path of the shell every command runs in (~ is your home folder); unset, it is
Git Bash (Windows), then bash on PATH, then cmd /C or sh -c. \"shell_command_prefix\" is
put in front of every command, on its own line.

slash commands (--tui)
  /model /use     switch model profile        /resume /rename   list, switch and name sessions
  /tree           browse and branch           /undo             revert the last turn's file changes
  /plan           propose, change nothing     /commit           stage and commit the work
  /export         transcript to markdown      /reload           rebuild and relaunch
  /settings       show/hide status-line parts  /mcp              MCP servers on/off
  /themes         pick the colour palette     /quit
  !CMD            run a shell command, output streamed; it goes to the model too
  !!CMD           the same, but the output is shown to you only
  shift+enter     newline in the draft (alt+enter where the terminal eats shift)
  ctrl+t          hide/show the model's thinking (saved as \"hide_thinking\" in config.json)
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("rusti {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--self-test") {
        ai_core::self_test();
        return;
    }
    if args.iter().any(|a| a == "--list") {
        let cfg = config::Config::load();
        if let Some(e) = &cfg.err {
            eprintln!("⚠ {e}");
        }
        eprintln!("config: {} + ./{}", config::global_path(), config::PATH);
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
        if let Some(e) = &cfg.err {
            eprintln!("⚠ {e}");
        }
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
                eprintln!("saved to {}", config::global_path());
            }
            None => eprintln!("aborted"),
        }
        return;
    }

    // project settings first, so flags and env below still override them
    let saved = config::Config::load();
    if let Some(e) = &saved.err {
        eprintln!("⚠ {e}\n  running with defaults: no saved models, permissions or MCP servers, and saving is off");
    }
    ai_core::allow_from_config(&saved.allow);
    ai_core::tools::set_shell(saved.shell.as_deref(), saved.shell_command_prefix.as_deref());
    if let Some(n) = saved.max_iters {
        ai_core::set_max_iters(n);
    }
    if let Some(n) = saved.context {
        ai_core::set_context_limit(n);
    }

    if args.iter().any(|a| a == "--yolo") || std::env::var("RUSTI_YOLO").map_or(false, |v| v == "1") {
        ai_core::tools::YOLO.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Some(n) = get(&args, "--max-iters", "RUSTI_MAX_ITERS") {
        match n.parse::<usize>() {
            Ok(n) if n > 0 => ai_core::set_max_iters(n),
            _ => { eprintln!("--max-iters needs a positive integer, got '{n}'"); std::process::exit(1); }
        }
    }
    if let Some(n) = get(&args, "--context", "RUSTI_CONTEXT") {
        match n.parse::<u64>() {
            Ok(n) if n > 0 => ai_core::set_context_limit(n),
            _ => { eprintln!("--context needs a positive token count, got '{n}'"); std::process::exit(1); }
        }
    }
    let (url, key, model) = resolve_model(&args);
    let task_arg = task_arg(&args);
    // bare `rusti` (or `rusti --resume`) in a terminal opens the TUI, like other
    // coding agents; a task argument or piped stdin stays a one-shot run
    let tui_mode = args.iter().any(|a| a == "--tui")
        || (task_arg.is_none() && !args.iter().any(|a| a == "--tree") && is_terminal::is_terminal(std::io::stdin()));

    // session + task first (so --tree can be cancelled before any model config)
    // --session NAME → .rusti/sessions/NAME.json, else the root session.json
    let session_path = get(&args, "--session", "RUSTI_SESSION")
        .map(|n| session::path_for(&n))
        .unwrap_or_else(|| session::PATH.to_string());
    let mut session;
    let task: String;
    if args.iter().any(|a| a == "--tree") {
        session = session::Session::load_from(&session_path);
        if session.is_empty() {
            eprintln!("no session.json to browse (run a task first)");
            std::process::exit(1);
        }
        match tree::browse(&mut session) {
            Some(prefill) => task = task_arg.unwrap_or_else(|| prompt("next instruction", &prefill)),
            None => return,
        }
    } else if args.iter().any(|a| a == "--resume") {
        session = session::Session::load_from(&session_path);
        if session.is_empty() {
            eprintln!("no session.json to resume (run a task first)");
            std::process::exit(1);
        }
        // tui mode never uses `task` (TuiConfig doesn't take one) — skip the
        // blocking stdin prompt so `--tui --resume` (incl. /reload's relaunch)
        // doesn't stall before the TUI even starts
        task = if tui_mode { String::new() } else { task_arg.unwrap_or_else(|| prompt("next instruction", "")) };
    } else {
        session = session::Session::with_path(model.clone(), &session_path);
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

    // plain mode connects too, so a one-shot run gets the same tools as the TUI;
    // status goes to stderr to keep stdout the answer alone for piping
    for line in ai_core::mcp::connect_all() {
        eprintln!("{}", line.trim_start());
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

/// Flags that consume the next argument — their values are not the task.
const VALUE_FLAGS: &[&str] = &["--url", "--key", "--model", "--session", "--use", "--max-iters", "--context"];

/// The first bare argument that isn't some flag's value.
fn task_arg(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if VALUE_FLAGS.contains(&a.as_str()) {
            it.next();
        } else if !a.starts_with('-') {
            return Some(a.clone());
        }
    }
    None
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
        cfg.save().unwrap_or_else(|e| eprintln!("warning: could not save settings: {e}"));
    }

    let url = explicit_url.unwrap_or(prof.url);
    let key = explicit_key.unwrap_or(prof.key);
    let model = explicit_model.unwrap_or(if prof.model.is_empty() { "gpt-4o-mini".into() } else { prof.model });
    (url, key, model)
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_arg_skips_flag_values() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(super::task_arg(&a(&["--session", "smoke", "do it"])), Some("do it".into()));
        assert_eq!(super::task_arg(&a(&["--url", "http://x", "--tui"])), None);
        assert_eq!(super::task_arg(&a(&["do it"])), Some("do it".into()));
    }
}
