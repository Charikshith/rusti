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
    pub msgs: usize,     // numbered user messages on the active path: what resuming shows
    pub branches: usize, // leaves in the tree; resuming shows only the active one
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
        msgs: s.msgs(),
        branches: s.branches(),
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

/// "1 msg", "3 msgs": the picker's counts read as words, not a bare number.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
    /// Whether a tool call succeeded, so a resumed transcript can redraw its
    /// ✓/✗. None on entries written before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    /// A data: URL sent alongside this entry's text as an image part. Kept
    /// beside `content` rather than turning content into parts: every reader
    /// of a transcript (export, replay, compaction, the tree) wants the text,
    /// and only the request builder cares that a picture rides with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default)]
    pub ts: u64,
}

impl Entry {
    pub fn new(role: &str, content: String) -> Entry {
        Entry { id: String::new(), parent: None, role: role.into(), content, tool_calls: None, tool_call_id: None, ok: None, image: None, ts: 0 }
    }

    /// This entry in OpenAI chat-completions message format.
    pub fn to_message(&self) -> Value {
        if self.role == "tool" {
            json!({"role": "tool", "tool_call_id": self.tool_call_id, "content": self.content})
        } else {
            // an OpenAI tool message takes a string only, which is why an image
            // travels on a user entry after the tool results, never on one
            let content = match &self.image {
                Some(url) => json!([
                    {"type": "text", "text": self.content},
                    {"type": "image_url", "image_url": {"url": url}},
                ]),
                None => json!(self.content),
            };
            let mut m = json!({"role": self.role, "content": content});
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
    epoch_secs()
}

/// Seconds since the unix epoch, UTC.
pub fn epoch_secs() -> u64 {
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

    /// User messages on the active path, counted as the transcript numbers
    /// them (N›): an attached image rides as a user entry nobody typed.
    pub fn msgs(&self) -> usize {
        self.path().iter().filter(|e| e.role == "user" && e.image.is_none()).count()
    }

    /// Leaves of the tree: one per branch left behind by /undo, /tree or
    /// compaction, plus the one being continued.
    pub fn branches(&self) -> usize {
        let parents: std::collections::HashSet<&str> =
            self.entries.iter().filter_map(|e| e.parent.as_deref()).collect();
        self.entries.iter().filter(|e| !parents.contains(e.id.as_str())).count()
    }

    /// Active path in OpenAI message format.
    pub fn path_messages(&self) -> Vec<Value> {
        self.path().iter().map(|e| e.to_message()).collect()
    }

    /// The active path as markdown, for sharing a session or filing an issue
    /// from it. Tool results are clipped — the point is a readable transcript,
    /// not a faithful dump; session.json is still the full record.
    pub fn export_markdown(&self) -> String {
        let mut out = format!("# rusti session: {}\n\nmodel: `{}`\n", name_of(&self.path), self.model);
        let mut n = 0;
        for e in self.path() {
            match e.role.as_str() {
                "user" => {
                    n += 1;
                    out.push_str(&format!("\n## {n} · user\n\n{}\n", e.content.trim()));
                }
                "assistant" => {
                    if !e.content.trim().is_empty() {
                        out.push_str(&format!("\n### assistant\n\n{}\n", e.content.trim()));
                    }
                    for (name, args) in tool_calls_of(e) {
                        out.push_str(&format!("\n- **{name}** `{args}`\n"));
                    }
                }
                "tool" => out.push_str(&format!("\n```\n{}\n```\n", clip(e.content.trim(), 500))),
                _ => {} // the system prompt is boilerplate, not transcript
            }
        }
        out
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

/// (name, one-line arguments) for each call on an assistant entry.
fn tool_calls_of(e: &Entry) -> Vec<(String, String)> {
    let Some(calls) = e.tool_calls.as_ref().and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    calls
        .iter()
        .map(|c| {
            let args = c["function"]["arguments"].as_str().unwrap_or("").replace('\n', " ");
            (c["function"]["name"].as_str().unwrap_or("?").to_string(), clip(&args, 160))
        })
        .collect()
}

fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_reports_message_count_and_latest_user_message() {
        let p = std::env::temp_dir().join("rusti_info_test.json");
        let path = p.to_string_lossy().into_owned();
        let mut s = Session::with_path("m".into(), &path);
        let sys = s.add(Entry::new("system", "sys".into()), None);
        let u = s.add(Entry::new("user", "first
second".into()), Some(sys));
        s.add(Entry::new("assistant", "ok".into()), Some(u));
        s.save().unwrap();

        let i = info(&path).unwrap();
        assert_eq!((i.msgs, i.branches), (1, 1)); // the system prompt and reply are not messages you typed
        assert_eq!(i.head, "first second");
        assert_eq!(i.name, "rusti_info_test");
        std::fs::remove_file(&path).ok();

        assert!(valid_name("tui-colors") && valid_name("a.b_c9"));
        assert!(!valid_name("") && !valid_name("has space") && !valid_name("../etc"));
        assert!(!valid_name(&"x".repeat(41)) && valid_name(&"x".repeat(40)));

        assert_eq!(count(1, "msg", "msgs"), "1 msg");
        assert_eq!(count(3, "branch", "branches"), "3 branches");

        assert_eq!(ago(30), "just now");
        assert_eq!(ago(600), "10m ago");
        assert_eq!(ago(7200), "2h ago");
        assert_eq!(ago(200_000), "2d ago");
    }

    #[test]
    fn export_renders_the_active_path_and_leaves_the_system_prompt_out() {
        let mut s = Session::with_path("m1".into(), "notes.json");
        let sys = s.add(Entry::new("system", "you are a coding agent".into()), None);
        let u = s.add(Entry::new("user", "read main.rs".into()), Some(sys));
        let mut a = Entry::new("assistant", String::new()); // a tool-call turn carries no prose
        a.tool_calls = Some(json!([{"function": {"name": "read_file", "arguments": "{\"path\":\n\"src/main.rs\"}"}}]));
        let a = s.add(a, Some(u));
        let t = s.add(Entry::new("tool", "fn main() {}".into()), Some(a));
        s.add(Entry::new("assistant", "It prints nothing.".into()), Some(t));

        let md = s.export_markdown();
        assert!(md.starts_with("# rusti session: notes\n\nmodel: `m1`\n"));
        assert!(md.contains("## 1 · user\n\nread main.rs"));
        assert!(md.contains("- **read_file** `{\"path\": \"src/main.rs\"}`")); // newline flattened
        assert!(md.contains("```\nfn main() {}\n```"));
        assert!(md.contains("### assistant\n\nIt prints nothing."));
        assert!(!md.contains("you are a coding agent"), "the system prompt is boilerplate");
        assert!(!md.contains("### assistant\n\n\n"), "an empty assistant turn gets no heading");

        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("héllo wörld", 4), "héll…"); // clips on chars, not bytes
    }
}

#[cfg(test)]
mod image_tests {
    use super::*;

    /// An image rides beside the text as a content part, and only for the roles
    /// that can carry one — a tool message takes a string, so putting parts on
    /// it is what the whole out-of-band attach exists to avoid.
    #[test]
    fn an_image_entry_becomes_content_parts() {
        let mut e = Entry::new("user", "image: shot.png".into());
        e.image = Some("data:image/png;base64,AAA".into());
        let m = e.to_message();
        assert_eq!(m["content"][0]["type"], "text");
        assert_eq!(m["content"][0]["text"], "image: shot.png");
        assert_eq!(m["content"][1]["image_url"]["url"], "data:image/png;base64,AAA");

        let plain = Entry::new("user", "hi".into()).to_message();
        assert_eq!(plain["content"], "hi", "no image, no parts");

        let mut t = Entry::new("tool", "attached shot.png".into());
        t.tool_call_id = Some("call_1".into());
        t.image = Some("data:image/png;base64,AAA".into());
        assert_eq!(t.to_message()["content"], "attached shot.png", "a tool message stays a string");
    }
}
