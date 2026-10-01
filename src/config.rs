// Settings live in two files: ~/.rusti/config.json (models, keys, theme,
// footer, MCP servers — shared by every project) and ./model.json (the
// "always" allow list and per-project limits). A key the project file has
// overrides the global one and is saved back there. Saved model profiles: On launch the CLI picks the
// default profile; if none is saved it asks interactively before running.
// --url/--key/--model flags and LLM_* env vars override the saved profile.

use serde::{Deserialize, Serialize};

pub const PATH: &str = "model.json";
/// Keys whose home is the project file; every other key lives in the global one.
/// "allow" is per project on purpose: a tool safe in one repo is not safe in all.
const PROJECT_KEYS: [&str; 3] = ["allow", "max_iters", "context"];

/// ~/.rusti — RUSTI_HOME overrides it (tests, a second setup). ponytail: no
/// `dirs` crate; USERPROFILE/HOME is all it would read on these platforms.
pub fn home_dir() -> std::path::PathBuf {
    if let Some(h) = std::env::var_os("RUSTI_HOME") {
        return h.into();
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_default();
    std::path::Path::new(&home).join(".rusti")
}

pub fn global_path() -> String {
    home_dir().join("config.json").to_string_lossy().into_owned()
}

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
/// `serde(default)`: a file saved before a segment existed still loads, with it on.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Footer {
    pub session: bool,
    pub model: bool,
    pub branch: bool,
    pub tokens: bool,
    pub context: bool,
    pub cache: bool,
}

impl Default for Footer {
    fn default() -> Self {
        Self { session: true, model: true, branch: true, tokens: true, context: true, cache: true }
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
    /// The terminal has a light background: row tints use pale washes instead
    /// of dark blocks. rusti cannot detect it, so it is a setting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub light: bool,
    /// BTreeMap, not HashMap: the tool list sent to the model must be in a
    /// stable order, or every run reshuffles it and defeats prompt caching.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub mcp: std::collections::BTreeMap<String, McpServer>,
    /// The shell every command runs in, as a path (a leading ~ is the home
    /// folder). Unset: Git Bash, then bash on PATH, then cmd /C or sh -c.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Put in front of every shell command, on its own line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_command_prefix: Option<String>,
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
    /// Top-level keys ./model.json had when loaded: those are saved back there.
    #[serde(skip)]
    project_keys: std::collections::BTreeSet<String>,
}

impl Config {
    pub fn load() -> Config {
        Self::load_pair(&global_path(), PATH)
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
        self.save_pair(&global_path(), PATH)
    }
    /// Global file, then the project file's top-level keys laid over it. The
    /// first run after this change finds models in ./model.json and no global
    /// file: those keys move home, so API keys stop living in the repo.
    pub fn load_pair(global: &str, project: &str) -> Config {
        let (g, p) = match (read_obj(global), read_obj(project)) {
            (Ok(g), Ok(p)) => (g, p),
            (Err(e), _) | (_, Err(e)) => return Config { err: Some(e), ..Default::default() },
        };
        let migrate = !std::path::Path::new(global).exists() && p.contains_key("models");
        let mut merged = g;
        let mut keys = std::collections::BTreeSet::new();
        for (k, v) in p {
            if !migrate || PROJECT_KEYS.contains(&k.as_str()) {
                keys.insert(k.clone());
            }
            merged.insert(k, v);
        }
        let mut cfg: Config = match serde_json::from_value(serde_json::Value::Object(merged)) {
            Ok(c) => c,
            Err(e) => return Config { err: Some(format!("{global} + {project}: {e}")), ..Default::default() },
        };
        cfg.project_keys = keys;
        if migrate {
            match cfg.save_pair(global, project) {
                Ok(()) => eprintln!("moved models/theme/footer/mcp from {project} to {global}"),
                Err(e) => eprintln!("⚠ could not move settings to {global}: {e}"),
            }
        }
        cfg
    }
    /// Each key goes back to the file it came from; a new key goes to its home
    /// (PROJECT_KEYS -> project, the rest -> global). The project file is only
    /// written when it exists or has something to hold, so running rusti in a
    /// folder does not leave a model.json behind.
    pub fn save_pair(&self, global: &str, project: &str) -> Result<(), String> {
        if let Some(e) = &self.err {
            return Err(format!("{e} — not overwriting it, fix the file by hand"));
        }
        let serde_json::Value::Object(all) = serde_json::to_value(self).map_err(|e| e.to_string())? else {
            unreachable!("Config serializes to an object")
        };
        // A key the project overrides keeps its global value: the override is
        // this project's, and dropping it here would change every other project.
        let mut g = read_obj(global)?;
        g.retain(|k, _| self.project_keys.contains(k));
        let mut p = serde_json::Map::new();
        for (k, v) in all {
            let to_project = self.project_keys.contains(&k) || PROJECT_KEYS.contains(&k.as_str());
            if to_project { p.insert(k, v) } else { g.insert(k, v) };
        }
        if let Some(dir) = std::path::Path::new(global).parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        write_obj(global, g)?;
        if !p.is_empty() || std::path::Path::new(project).exists() {
            write_obj(project, p)?;
        }
        Ok(())
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

/// A missing file is an empty object (first run); an unreadable or unparsable
/// one is an error, so save refuses and neither file is overwritten.
fn read_obj(path: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{path}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("{path}: {e}")),
    }
}

fn write_obj(path: &str, m: serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let s = serde_json::to_string_pretty(&serde_json::Value::Object(m)).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| format!("writing {path}: {e}"))
}

/// Interactive profile creation (stderr prompts, stdin answers).
/// Returns None when stdin closes (EOF).
pub fn ask_profile() -> Option<ModelProfile> {
    eprintln!("No saved model in {} — add one (enter keeps [default]):", global_path());
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

        // a footer saved before the cache segment existed: cache comes up on, the rest as saved
        std::fs::write(p, r#"{"footer":{"session":true,"model":true,"branch":true,"tokens":true,"context":false}}"#).unwrap();
        let old = Config::load_from(p);
        assert!(old.footer.cache && !old.footer.context, "a missing segment defaults on, saved ones keep their value");
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
    /// The two-file split: each key saves back where it came from, a folder
    /// gains no model.json it does not need, an old model.json moves its
    /// models home once, and a broken global file freezes both files.
    #[test]
    fn global_and_project_files_split_and_merge() {
        let dir = std::env::temp_dir().join(format!("rusti_pair_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = dir.join("home").join("config.json");
        let (g, p) = (g.to_str().unwrap(), dir.join("model.json"));
        let p = p.to_str().unwrap();
        let read = |f: &str| std::fs::read_to_string(f).unwrap();

        // migration: an old model.json with models and no global file
        std::fs::write(p, r#"{"default":"mimo","models":[{"name":"mimo","url":"u","key":"k"}],"allow":["read"],"theme":"nord"}"#).unwrap();
        let cfg = Config::load_pair(g, p);
        assert!(cfg.err.is_none());
        assert!(read(g).contains("\"key\": \"k\"") && read(g).contains("nord"), "models and theme move home");
        assert!(!read(p).contains("models") && !read(p).contains("theme"), "and leave the repo");
        assert!(read(p).contains("read"), "allow stays in the project");

        // global only: a footer toggle lands in the global file, no model.json appears
        std::fs::remove_file(p).unwrap();
        let mut cfg = Config::load_pair(g, p);
        assert_eq!(cfg.resolve().unwrap().name, "mimo", "models load from the global file");
        cfg.footer.context = false;
        cfg.save_pair(g, p).unwrap();
        assert!(!std::path::Path::new(p).exists(), "no project file for global settings");
        assert!(!Config::load_pair(g, p).footer.context);

        // a new allow goes to the project, not the global file
        cfg.allow.push("edit".into());
        cfg.save_pair(g, p).unwrap();
        assert!(read(p).contains("edit") && !read(g).contains("allow"));

        // a project override is read from and saved to the project only
        std::fs::write(p, r#"{"theme":"gruvbox"}"#).unwrap();
        let before = read(g);
        let mut cfg = Config::load_pair(g, p);
        assert_eq!(cfg.theme.as_deref(), Some("gruvbox"), "project key wins");
        cfg.theme = Some("dracula".into());
        cfg.save_pair(g, p).unwrap();
        assert!(read(p).contains("dracula"));
        assert_eq!(read(g), before, "global file untouched by a project override");
        std::fs::remove_file(p).unwrap();
        assert_eq!(Config::load_pair(g, p).theme.as_deref(), Some("nord"), "other projects keep the global theme");

        // a malformed global file: reported, and neither file is written
        std::fs::write(g, "{ not json").unwrap();
        std::fs::write(p, r#"{"allow":["read"]}"#).unwrap();
        let before_p = read(p);
        let mut cfg = Config::load_pair(g, p);
        assert!(cfg.err.is_some());
        cfg.theme = Some("x".into());
        assert!(cfg.save_pair(g, p).is_err());
        assert_eq!(read(g), "{ not json");
        assert_eq!(read(p), before_p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
