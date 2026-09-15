// Saved model profiles in model.json (cwd). On launch the CLI picks the
// default profile; if none is saved it asks interactively before running.
// --url/--key/--model flags and LLM_* env vars override the saved profile.

use serde::{Deserialize, Serialize};

pub const PATH: &str = "model.json";

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ModelProfile {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub model: String,
}

/// Which status-line segments are drawn. All on by default; /settings toggles
/// them and writes the result back here, so the choice outlives the process.
#[derive(Clone, Serialize, Deserialize)]
pub struct Footer {
    pub session: bool,
    pub model: bool,
    pub branch: bool,
    pub tokens: bool,
    pub context: bool,
}

impl Default for Footer {
    fn default() -> Self {
        Self { session: true, model: true, branch: true, tokens: true, context: true }
    }
}

/// One MCP server: a child process spoken to over stdio. `enabled` is what
/// /mcp flips, so a server can be switched off without deleting its config.
#[derive(Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelProfile>,
    /// Tools the user answered "always" to. Seeded into the permission gate at
    /// startup, which is what makes that answer outlive the process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default)]
    pub footer: Footer,
    /// The palette /themes picked, by name. None is the built-in default, and
    /// an unknown name is reported rather than applied — a theme table that
    /// loses an entry must not silently restyle the terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// BTreeMap, not HashMap: the tool list sent to the model must be in a
    /// stable order, or every run reshuffles it and defeats prompt caching.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub mcp: std::collections::BTreeMap<String, McpServer>,
    /// Per-project defaults for --max-iters / --context; flags and env win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iters: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// Set when the file exists but could not be read or parsed. Save refuses
    /// while it is set: /settings and /mcp do load -> flip -> save, so without
    /// this a typo in model.json would be overwritten by these defaults and
    /// take the models, allow list and MCP servers with it.
    #[serde(skip)]
    pub err: Option<String>,
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(PATH)
    }
    /// No file is a first run and loads the defaults. A file that exists but
    /// does not read or parse is an error, not an empty config: it keeps the
    /// defaults so the process still runs, but records why so nothing saves.
    pub fn load_from(path: &str) -> Config {
        let s = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
            Err(e) => return Config { err: Some(format!("{path}: {e}")), ..Default::default() },
        };
        serde_json::from_str(&s)
            .unwrap_or_else(|e| Config { err: Some(format!("{path}: {e}")), ..Default::default() })
    }
    pub fn save(&self) -> Result<(), String> {
        self.save_to(PATH)
    }
    pub fn save_to(&self, path: &str) -> Result<(), String> {
        if let Some(e) = &self.err {
            return Err(format!("{e} — not overwriting it, fix the file by hand"));
        }
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| format!("writing {path}: {e}"))
    }
    /// The active profile: the named default, else the first saved.
    pub fn resolve(&self) -> Option<&ModelProfile> {
        self.models
            .iter()
            .find(|m| Some(&m.name) == self.default.as_ref())
            .or_else(|| self.models.first())
    }
    pub fn add(&mut self, p: ModelProfile) {
        match self.models.iter_mut().find(|m| m.name == p.name) {
            Some(slot) => *slot = p, // replace same-named profile
            None => self.models.push(p),
        }
    }
}

/// Interactive profile creation (stderr prompts, stdin answers).
/// Returns None when stdin closes (EOF).
pub fn ask_profile() -> Option<ModelProfile> {
    eprintln!("No saved model in model.json — add one (enter keeps [default]):");
    let name = required("name", "default")?;
    let url = required("url (http://host:port/v1/chat/completions)", "")?;
    let key = prompt("api key (optional)", "")?;
    let model = prompt("model id", "gpt-4o-mini")?;
    Some(ModelProfile { name, url, key, model })
}

fn prompt(label: &str, default: &str) -> Option<String> {
    use std::io::Write;
    let suffix = if default.is_empty() { String::new() } else { format!(" [{default}]") };
    eprint!("  {label}{suffix} > ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => None, // EOF
        Ok(_) => {
            let s = line.trim().to_string();
            Some(if s.is_empty() { default.to_string() } else { s })
        }
        Err(_) => None,
    }
}

fn required(label: &str, default: &str) -> Option<String> {
    loop {
        match prompt(label, default) {
            Some(s) if !s.is_empty() => return Some(s),
            Some(_) => eprintln!("  (required)"),
            None => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// /settings does load -> flip -> save. The risk is that round-trip
    /// dropping the rest of model.json, or an older file with no "footer" key
    /// failing to parse at all.
    #[test]
    fn toggling_footer_keeps_the_rest_of_the_file() {
        let p = std::env::temp_dir().join("rusti_footer_test.json");
        let p = p.to_str().unwrap();
        std::fs::write(p, r#"{"default":"mimo","models":[{"name":"mimo","url":"u","key":"k","model":"m"}],"allow":["read"]}"#).unwrap();

        // an older file has no "footer": it must still load, all segments on
        let mut cfg = Config::load_from(p);
        assert!(cfg.footer.context && cfg.footer.session, "missing footer must default to on");

        cfg.footer.context = false;
        cfg.save_to(p).unwrap();

        let back = Config::load_from(p);
        assert!(!back.footer.context, "toggle must survive the round-trip");
        assert!(back.footer.session, "untouched segments stay on");
        assert_eq!(back.default.as_deref(), Some("mimo"), "default must survive");
        assert_eq!(back.models.len(), 1, "models must survive");
        assert_eq!(back.allow, vec!["read"], "allow must survive");
        let _ = std::fs::remove_file(p);
    }

    /// A hand-edit typo ("base" for "url") fails the whole parse. It must not
    /// read as "no config", and the next /settings or /mcp toggle must not
    /// write defaults over it.
    #[test]
    fn a_malformed_file_is_reported_and_never_overwritten() {
        let p = std::env::temp_dir().join("rusti_bad_config_test.json");
        let p = p.to_str().unwrap();
        let raw = r#"{"default":"mimo","models":[{"name":"mimo","base":"u"}],"allow":["read"]}"#;
        std::fs::write(p, raw).unwrap();

        let mut cfg = Config::load_from(p);
        assert!(cfg.err.is_some(), "a malformed file must report, not default silently");

        cfg.footer.context = false;
        assert!(cfg.save_to(p).is_err(), "save must refuse while the file is unreadable");
        assert_eq!(std::fs::read_to_string(p).unwrap(), raw, "the file must be byte-identical");

        // a missing file is a first run, not an error: it saves normally
        let _ = std::fs::remove_file(p);
        let fresh = Config::load_from(p);
        assert!(fresh.err.is_none(), "no file is a first run, not a failure");
        fresh.save_to(p).unwrap();
        let _ = std::fs::remove_file(p);
    }
}
