// Session persistence as a message TREE, mirroring pi's session model:
// every entry has an id and parentId; the current position is the active
// leaf. Branching = moving the leaf to an earlier entry and continuing —
// no new files. Stored as one JSON file (session.json).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PATH: &str = "session.json";

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
    pub fn load() -> Session {
        Self::load_from(PATH)
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
