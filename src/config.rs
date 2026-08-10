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

#[derive(Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelProfile>,
}

impl Config {
    pub fn load() -> Config {
        Self::load_from(PATH)
    }
    pub fn load_from(path: &str) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        self.save_to(PATH)
    }
    pub fn save_to(&self, path: &str) -> Result<(), String> {
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
