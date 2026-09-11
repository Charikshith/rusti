// Session persistence as a message TREE, mirroring pi's session model:
// every entry has an id and parentId; the current position is the active
// leaf. Branching = moving the leaf to an earlier entry and continuing —
// no new files. Stored as one JSON file (session.json).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PATH: &str = "session.json";

/// Named sessions live here; the bare session.json at the root stays the
/// unnamed default, so sessions saved before naming existed still load.
pub const DIR: &str = ".rusti/sessions";

pub fn path_for(name: &str) -> String {
    format!("{DIR}/{name}.json")
}

/// Display name of a session file: its stem, so
/// `.rusti/sessions/main.json` reads "main" and the root file "session".
pub fn name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One saved session, as the /resume picker lists it.
pub struct Info {
    pub name: String,
    pub path: String,
    pub entries: usize,
    pub age_s: u64,
    pub head: String, // latest user message on the active path
}

/// Saved sessions, most recently touched first: everything under DIR plus the
/// root session.json. Empty files are skipped — nothing to resume.
pub fn list() -> Vec<Info> {
    let mut paths: Vec<String> = std::fs::read_dir(DIR)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|p| p.ends_with(".json"))
        .collect();
    if std::path::Path::new(PATH).exists() {
        paths.push(PATH.to_string());
    }
    let mut out: Vec<Info> = paths.iter().filter_map(|p| info(p)).collect();
    out.sort_by_key(|i| i.age_s);
    out
}

fn info(path: &str) -> Option<Info> {
    let s = Session::load_from(path);
    if s.is_empty() {
        return None;
    }
    let age_s = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|d| d.as_secs()).unwrap_or(0))
        .unwrap_or(u64::MAX);
    let head = s
        .path()
        .iter()
        .rev()
        .find(|e| e.role == "user")
        .map(|e| e.content.replace('\n', " ").chars().take(48).collect())
        .unwrap_or_default();
    Some(Info {
        name: name_of(path),
        path: path.to_string(),
        entries: s.entries.len(),
        age_s,
        head,
    })
}

/// A session name is a file stem under DIR, so it gets the character check
/// that implies: ASCII letters, digits, `.`, `_`, `-`, at most 40 of them.
pub fn valid_name(name: &str) -> bool {
    let n = name.chars().count();
    n >= 1 && n <= 40 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Coarse relative time for the picker: "2m ago", "3h ago", "4d ago".
pub fn ago(s: u64) -> String {
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(rename = "parentId", default)]
    pub parent: Option<String>,
    pub role: String, // system | user | assistant | tool
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Option<Value>,   // assistant only
    #[serde(rename = "toolCallId", default)]
    pub tool_call_id: Option<String>, // tool only
    #[serde(default)]
    pub ts: u64,
}

impl Entry {
    pub fn new(role: &str, content: String) -> Entry {
        Entry { id: String::new(), parent: None, role: role.into(), content, tool_calls: None, tool_call_id: None, ts: 0 }
    }

    /// This entry in OpenAI chat-completions message format.
    pub fn to_message(&self) -> Value {
        if self.role == "tool" {
            json!({"role": "tool", "tool_call_id": self.tool_call_id, "content": self.content})
        } else {
            let mut m = json!({"role": self.role, "content": self.content});
            if let Some(tc) = &self.tool_calls {
                m["tool_calls"] = tc.clone();
            }
            m
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Session {
    pub model: String,
    #[serde(default)]
    pub active: Option<String>, // active leaf id
    #[serde(rename = "nextId", default)]
    pub next_id: u64,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(skip)]
    pub path: String, // where save() writes
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Session {
    pub fn new(model: String) -> Session {
        Session { model, active: None, next_id: 1, entries: Vec::new(), path: PATH.into() }
    }
    pub fn with_path(model: String, path: &str) -> Session {
        let mut s = Session::new(model);
        s.path = path.into();
        s
    }
    pub fn load_from(path: &str) -> Session {
        let mut s = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| Session::new("".into()));
        s.path = path.into();
        // defensive: never leave the active leaf dangling
        if s.active.is_none() && !s.entries.is_empty() {
            s.active = s.entries.last().map(|e| e.id.clone());
        }
        s
    }
    pub fn save(&self) -> Result<(), String> {
        if let Some(dir) = std::path::Path::new(&self.path).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
            }
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| format!("writing {}: {e}", self.path))
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append an entry under `parent`; becomes the new active leaf.
    pub fn add(&mut self, mut e: Entry, parent: Option<String>) -> String {
        e.id = format!("m{}", self.next_id);
        self.next_id += 1;
        e.parent = parent;
        e.ts = now();
        self.active = Some(e.id.clone());
        self.entries.push(e);
        self.active.clone().unwrap()
    }

    fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Entries from the root down to the active leaf.
    pub fn path(&self) -> Vec<&Entry> {
        let mut out = Vec::new();
        let mut cur = self.active.clone();
        while let Some(id) = cur {
            match self.find(&id) {
                Some(e) => {
                    out.push(e);
                    cur = e.parent.clone();
                }
                None => break,
            }
        }
        out.reverse();
        out
    }

    /// Active path in OpenAI message format.
    pub fn path_messages(&self) -> Vec<Value> {
        self.path().iter().map(|e| e.to_message()).collect()
    }

    /// Branch from the given entry, pi-style: selecting a user message moves
    /// the leaf to its parent (its text is offered back for editing, which
    /// then forks a new branch); selecting an assistant message continues
    /// from that point. Returns the entry's text as a prompt prefill.
    pub fn select(&mut self, id: &str) -> Option<String> {
        let e = self.find(id)?;
        let text = e.content.clone();
        self.active = if e.role == "user" { e.parent.clone() } else { Some(e.id.clone()) };
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_reports_entry_count_and_latest_user_message() {
        let p = std::env::temp_dir().join("rusti_info_test.json");
        let path = p.to_string_lossy().into_owned();
        let mut s = Session::with_path("m".into(), &path);
        let sys = s.add(Entry::new("system", "sys".into()), None);
        let u = s.add(Entry::new("user", "first
second".into()), Some(sys));
        s.add(Entry::new("assistant", "ok".into()), Some(u));
        s.save().unwrap();

        let i = info(&path).unwrap();
        assert_eq!(i.entries, 3);
        assert_eq!(i.head, "first second");
        assert_eq!(i.name, "rusti_info_test");
        std::fs::remove_file(&path).ok();

        assert!(valid_name("tui-colors") && valid_name("a.b_c9"));
        assert!(!valid_name("") && !valid_name("has space") && !valid_name("../etc"));
        assert!(!valid_name(&"x".repeat(41)) && valid_name(&"x".repeat(40)));

        assert_eq!(ago(30), "just now");
        assert_eq!(ago(600), "10m ago");
        assert_eq!(ago(7200), "2h ago");
        assert_eq!(ago(200_000), "2d ago");
    }
}
