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
    /// Per-project defaults for --max-iters / --context; flags and env win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iters: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
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
}
