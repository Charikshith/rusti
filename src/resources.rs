// Declarative extensions (E0): prompt templates and skills. Nothing here
// runs code: a template is text that becomes the user message, a skill is a
// file the model reads. Both come from ~/.rusti and, once the folder is
// trusted, ./.rusti; skills also from the Agent Skills folders ~/.agents/skills
// and ./.agents/skills, so a skill written for another agent works as is.
// No YAML crate: the frontmatter these files use is a small subset, read by hand.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where `sub` ("prompts", "skills", "themes") is read from, in priority
/// order, with the tag the / menu shows: u for yours, p for this project's.
/// Project folders only once the folder is trusted.
pub fn dirs(sub: &str) -> Vec<(PathBuf, &'static str)> {
    let mut v = vec![(crate::config::home_dir().join(sub), "u")];
    let agents = sub == "skills";
    if agents {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap_or_default();
        v.push((Path::new(&home).join(".agents").join("skills"), "u"));
    }
    if crate::config::trusted() {
        v.push((Path::new(".rusti").join(sub), "p"));
        if agents {
            v.push((Path::new(".agents").join("skills"), "p"));
        }
    }
    v
}

/// A `---` block at the top of a file: its `key: value` pairs, the text after
/// it, and one warning per line it could not read. A value may be quoted,
/// continue on indented lines, or be a `>` (folded) or `|` (literal) block.
/// No block, or one that never closes, is all body.
pub fn frontmatter(text: &str) -> (BTreeMap<String, String>, String, Vec<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut map = BTreeMap::new();
    let mut warn = Vec::new();
    let mut lines = text.split_inclusive('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return (map, text.to_string(), warn);
    }
    // (key, literal?) of the value still taking indented lines
    let mut open: Option<(String, bool)> = None;
    let mut used = text.find('\n').map_or(text.len(), |i| i + 1);
    for (n, raw) in lines.enumerate() {
        used += raw.len();
        let line = raw.trim_end();
        if line == "---" {
            for v in map.values_mut() {
                *v = v.trim().to_string();
            }
            return (map, text[used..].to_string(), warn);
        }
        let indented = line.starts_with([' ', '\t']);
        if line.trim().is_empty() {
            if let Some((k, true)) = &open {
                map.get_mut(k).expect("open key is in the map").push('\n');
            }
            continue;
        }
        if let (true, Some((k, literal))) = (indented, &open) {
            let v = map.get_mut(k).expect("open key is in the map");
            if !v.is_empty() {
                v.push_str(if *literal { "\n" } else { " " });
            }
            v.push_str(line.trim());
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        match line.split_once(':').filter(|(k, _)| !indented && !k.is_empty() && !k.contains(char::is_whitespace)) {
            Some((k, v)) => {
                let v = v.trim();
                let block = v.trim_end_matches(['-', '+']);
                open = Some((k.to_string(), block == "|"));
                map.insert(k.to_string(), if block == ">" || block == "|" { String::new() } else { unquote(v) });
            }
            None => warn.push(format!("frontmatter line {} not read: {line}", n + 2)),
        }
    }
    warn.push("frontmatter never closes with ---; read as body".into());
    (BTreeMap::new(), text.to_string(), warn)
}

/// `"…"` reads as a JSON string (YAML's double quotes escape the same way),
/// `'…'` doubles its quote; anything else is the text itself.
fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return serde_json::from_str(v).unwrap_or_else(|_| v[1..v.len() - 1].to_string());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

/// A name that works as `/name`: no spaces, nothing a shell or a path would trip on.
fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The menu's one-line description: the frontmatter's, else the body's first
/// line with any heading marks off.
fn describe(fm: &BTreeMap<String, String>, body: &str) -> String {
    let d = fm.get("description").cloned().unwrap_or_else(|| {
        body.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
    });
    let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
    if d.chars().count() > 100 { format!("{}…", d.chars().take(99).collect::<String>()) } else { d }
}

/// prompts/<name>.md: typed as `/name args`, sent as its body with the
/// arguments filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub name: String,
    pub desc: String,
    pub hint: String,
    pub body: String,
    pub tag: &'static str,
}

pub fn templates(warn: &mut Vec<String>) -> Vec<Template> {
    templates_in(&dirs("prompts"), warn)
}

/// Every `*.md` in each folder, by name. The first folder to define a name
/// keeps it, so a repo's template cannot shadow your own.
pub fn templates_in(dirs: &[(PathBuf, &'static str)], warn: &mut Vec<String>) -> Vec<Template> {
    let mut out: Vec<Template> = Vec::new();
    for (dir, tag) in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
        files.sort();
        for p in files {
            let name = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let shown = slash(&p);
            if !valid_name(&name) {
                warn.push(format!("prompt {shown}: '{name}' cannot be a command (letters, digits, . _ - only)"));
                continue;
            }
            if out.iter().any(|t| t.name == name) {
                warn.push(format!("prompt {shown}: /{name} is already defined, skipped"));
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                warn.push(format!("prompt {shown}: unreadable, skipped"));
                continue;
            };
            let (fm, body, w) = frontmatter(&text);
            warn.extend(w.into_iter().map(|w| format!("prompt {shown}: {w}")));
            out.push(Template {
                desc: describe(&fm, &body),
                hint: fm.get("argument-hint").cloned().unwrap_or_default(),
                body: body.trim().to_string(),
                name,
                tag,
            });
        }
    }
    out
}

/// Arguments as a shell would split them: whitespace separates, '…' and "…"
/// group, and a backslash outside single quotes takes the next char as is.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut cur, mut quote, mut any) = (String::new(), None::<char>, false);
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"') | None, '\\') => cur.extend(it.next()),
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Fill a template: `$1`…`$9`, `$@` and `$ARGUMENTS` (all of them, space
/// joined), `${1:-default}` (the default when that one is missing or empty).
/// Any other `$` stays as written. Pi drops arguments a template never asks
/// for; here they are appended after a blank line, so nothing typed is lost.
/// ponytail: no `${@:N:L}` slices; add them when a template needs one.
pub fn expand(body: &str, args: &[String]) -> String {
    let all = args.join(" ");
    let arg = |d: char| d.to_digit(10).filter(|&n| n >= 1).and_then(|n| args.get(n as usize - 1)).cloned().unwrap_or_default();
    let mut out = String::new();
    let mut used = false;
    let mut rest = body;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let first = after.chars().next();
        let (text, skip) = match first {
            Some(d @ '1'..='9') => (Some(arg(d)), 1),
            Some('@') => (Some(all.clone()), 1),
            _ if after.starts_with("ARGUMENTS") => (Some(all.clone()), 9),
            Some('{') => match after[1..].split_once('}') {
                Some((inner, _)) => {
                    let (key, default) = inner.split_once(":-").map_or((inner, None), |(k, d)| (k, Some(d)));
                    let val = match key {
                        "@" | "ARGUMENTS" => Some(all.clone()),
                        k if k.len() == 1 && matches!(k.as_bytes()[0], b'1'..=b'9') => Some(arg(k.chars().next().unwrap())),
                        _ => None,
                    };
                    (val.map(|v| if v.is_empty() { default.unwrap_or("").to_string() } else { v }), inner.len() + 2)
                }
                None => (None, 0),
            },
            _ => (None, 0),
        };
        match text {
            Some(t) => {
                out.push_str(&t);
                used = true;
                rest = &after[skip..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    if !used && !all.is_empty() {
        out.push_str("\n\n");
        out.push_str(&all);
    }
    out
}

/// A folder holding SKILL.md. The model sees its name, description and path,
/// and reads the file itself when a task matches; `/skill:name` forces it.
#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub desc: String,
    pub hint: String,
    /// Absolute, so the model can read it from any working directory.
    pub path: PathBuf,
    /// `disable-model-invocation: true`: only `/skill:name` uses it.
    pub manual: bool,
    pub tag: &'static str,
}

pub fn skills(warn: &mut Vec<String>) -> Vec<Skill> {
    skills_in(&dirs("skills"), warn)
}

/// Skills under each folder, by name; the first folder to define a name
/// keeps it. A skill with no description is skipped: the model could never
/// tell when to use it.
pub fn skills_in(dirs: &[(PathBuf, &'static str)], warn: &mut Vec<String>) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    for (dir, tag) in dirs {
        let mut found = Vec::new();
        skill_files(dir, 0, &mut found);
        found.sort();
        for p in found {
            let path = std::path::absolute(&p).unwrap_or(p);
            let shown = slash(&path);
            let Ok(text) = std::fs::read_to_string(&path) else {
                warn.push(format!("skill {shown}: unreadable, skipped"));
                continue;
            };
            let (fm, _, w) = frontmatter(&text);
            warn.extend(w.into_iter().map(|w| format!("skill {shown}: {w}")));
            let folder = path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy().into_owned();
            let name = fm.get("name").cloned().unwrap_or(folder);
            let desc = fm.get("description").map(|d| d.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
            if !valid_name(&name) {
                warn.push(format!("skill {shown}: '{name}' cannot be a command (letters, digits, . _ - only)"));
            } else if desc.is_empty() {
                warn.push(format!("skill {shown}: no description, skipped"));
            } else if out.iter().any(|s| s.name == name) {
                warn.push(format!("skill {shown}: {name} is already defined, skipped"));
            } else {
                out.push(Skill {
                    name,
                    desc,
                    hint: fm.get("argument-hint").cloned().unwrap_or_default(),
                    manual: fm.get("disable-model-invocation").is_some_and(|v| v == "true"),
                    path,
                    tag,
                });
            }
        }
    }
    out
}

/// SKILL.md files at most three folders down, not looking inside a skill
/// (its own subfolders are its resources) or a hidden or dependency folder.
fn skill_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !p.is_dir() || name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if p.join("SKILL.md").is_file() {
            out.push(p.join("SKILL.md"));
        } else if depth < 2 {
            skill_files(&p, depth + 1, out);
        }
    }
}

/// The system prompt's list of skills the model may pick by itself, or
/// None when there are none.
pub fn skills_section(skills: &[Skill]) -> Option<String> {
    let rows: Vec<String> = skills
        .iter()
        .filter(|s| !s.manual)
        .map(|s| format!("- {}: {} ({})", s.name, s.desc, slash(&s.path)))
        .collect();
    (!rows.is_empty()).then(|| {
        format!(
            "# Skills\nA skill is a folder of instructions for one kind of task. When the task matches a skill's \
description, read its SKILL.md with read_file before starting, and follow it; paths in a skill are relative to \
its folder. Skip skills that do not match.\n{}",
            rows.join("\n")
        )
    })
}

/// What `/skill:name args` sends: the skill's body, read now, tagged with
/// where it lives, then the arguments.
pub fn skill_message(s: &Skill, args: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(&s.path).map_err(|e| format!("reading {}: {e}", slash(&s.path)))?;
    let (_, body, _) = frontmatter(&text);
    let dir = s.path.parent().map(slash).unwrap_or_default();
    let mut m = format!(
        "<skill name=\"{}\" location=\"{}\">\nPaths in this skill are relative to {dir}.\n\n{}\n</skill>",
        s.name,
        slash(&s.path),
        body.trim()
    );
    if !args.trim().is_empty() {
        m.push_str("\n\n");
        m.push_str(args.trim());
    }
    Ok(m)
}

/// Forward slashes: some models mangle Windows backslashes in a path.
pub fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// The subset real SKILL.md files use: plain, quoted (with escapes),
    /// folded and literal blocks, indented continuations, CRLF and a BOM.
    #[test]
    fn frontmatter_reads_the_yaml_subset_skills_use() {
        let (m, body, w) = frontmatter("---\nname: review\ndescription: \"Say \\\"hi\\\" now\"\nargument-hint: '<file> [it''s]'\n---\n# Body\ntext\n");
        assert_eq!(m["name"], "review");
        assert_eq!(m["description"], "Say \"hi\" now");
        assert_eq!(m["argument-hint"], "<file> [it's]");
        assert_eq!(body, "# Body\ntext\n");
        assert!(w.is_empty(), "{w:?}");

        let (m, body, w) = frontmatter("\u{feff}---\r\ndescription: >\r\n  folded one\r\n  and two\r\nnotes: |\r\n  line a\r\n  line b\r\nlong: starts here\r\n   and goes on\r\n# a comment\r\n- a list item\r\n---\r\nbody\r\n");
        assert_eq!(m["description"], "folded one and two");
        assert_eq!(m["notes"], "line a\nline b");
        assert_eq!(m["long"], "starts here and goes on");
        assert_eq!(body, "body\r\n");
        assert_eq!(w, ["frontmatter line 11 not read: - a list item"]);

        let (m, body, _) = frontmatter("no frontmatter\n---\n");
        assert!(m.is_empty() && body == "no frontmatter\n---\n");
        let (m, body, w) = frontmatter("---\nname: x\nnever closed\n");
        assert!(m.is_empty() && body.starts_with("---") && w.last().unwrap().contains("never closes"));
    }

    #[test]
    fn arguments_split_like_a_shell() {
        assert_eq!(split_args("a  b\tc"), s(&["a", "b", "c"]));
        assert_eq!(split_args(r#"src/main.rs "two words" 'it''s' """#), s(&["src/main.rs", "two words", "its", ""]));
        assert_eq!(split_args(r#"a\ b "q\"x" 'no\esc'"#), s(&["a b", "q\"x", "no\\esc"]));
        assert!(split_args("   ").is_empty());
    }

    /// The captain's worked example: /review with a file and a focus.
    #[test]
    fn template_placeholders_expand() {
        let body = "Review $1 for ${2:-bugs}. All: $@ | $ARGUMENTS | ${@}. Cost: $5 stays $x and ${9:-none}$";
        assert_eq!(
            expand(body, &s(&["src/a.rs", "style"])),
            "Review src/a.rs for style. All: src/a.rs style | src/a.rs style | src/a.rs style. Cost:  stays $x and none$"
        );
        assert_eq!(expand("Review $1 for ${2:-bugs}.", &s(&["a.rs"])), "Review a.rs for bugs.");
        assert_eq!(expand("Review ${1:-everything}", &s(&[""])), "Review everything", "empty counts as missing");
        assert_eq!(expand("Summarize the diff.", &s(&["focus", "tests"])), "Summarize the diff.\n\nfocus tests", "unused args are kept");
        assert_eq!(expand("Summarize the diff.", &[]), "Summarize the diff.");
        assert_eq!(expand("unclosed ${1", &s(&["a"])), "unclosed ${1\n\na");
    }

    #[test]
    fn templates_and_skills_load_with_user_first() {
        let dir = std::env::temp_dir().join(format!("rusti_res_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (u, p) = (dir.join("u"), dir.join("p"));
        for d in [u.join("prompts"), p.join("prompts")] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(u.join("prompts/review.md"), "---\ndescription: Review a file\nargument-hint: <file> [focus]\n---\nReview $1 for ${2:-bugs}.\n").unwrap();
        std::fs::write(p.join("prompts/review.md"), "repo's own review").unwrap();
        std::fs::write(p.join("prompts/fix.md"), "\n# Fix the build\nRun cargo build and fix it.\n").unwrap();
        std::fs::write(p.join("prompts/bad name.md"), "x").unwrap();
        std::fs::write(p.join("prompts/notes.txt"), "not a template").unwrap();
        let mut warn = Vec::new();
        let t = templates_in(&[(u.join("prompts"), "u"), (p.join("prompts"), "p")], &mut warn);
        let names: Vec<_> = t.iter().map(|t| (t.name.as_str(), t.tag)).collect();
        assert_eq!(names, [("review", "u"), ("fix", "p")]);
        assert_eq!((t[0].desc.as_str(), t[0].hint.as_str(), t[0].body.as_str()), ("Review a file", "<file> [focus]", "Review $1 for ${2:-bugs}."));
        assert_eq!(t[1].desc, "Fix the build", "no description: the first line");
        assert_eq!(warn.len(), 2, "{warn:?}");
        assert!(warn.iter().any(|w| w.contains("/review is already defined")) && warn.iter().any(|w| w.contains("cannot be a command")));

        let sk = |root: &Path, name: &str, fm: &str| {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("SKILL.md"), format!("---\n{fm}\n---\nDo the {name} thing.\n")).unwrap();
        };
        sk(&u.join("skills"), "pdf", "name: pdf\ndescription: Read and fill PDF forms");
        sk(&u.join("skills/group"), "deep", "description: nested one level");
        sk(&u.join("skills"), "manual", "name: manual\ndescription: Only by hand\ndisable-model-invocation: true");
        sk(&u.join("skills"), "blank", "name: blank");
        sk(&p.join("skills"), "pdf", "name: pdf\ndescription: the repo's pdf");
        std::fs::create_dir_all(u.join("skills/pdf/inner")).unwrap();
        std::fs::write(u.join("skills/pdf/inner/SKILL.md"), "---\ndescription: inside a skill\n---\n").unwrap();
        let mut warn = Vec::new();
        let skills = skills_in(&[(u.join("skills"), "u"), (p.join("skills"), "p")], &mut warn);
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["deep", "manual", "pdf"], "{warn:?}");
        assert!(skills.iter().all(|s| s.path.is_absolute() && s.tag == "u"));
        assert!(skills[1].manual && !skills[2].manual);
        assert!(warn.iter().any(|w| w.contains("no description")) && warn.iter().any(|w| w.contains("pdf is already defined")));

        let section = skills_section(&skills).unwrap();
        assert!(section.contains("- pdf: Read and fill PDF forms (") && section.contains("/pdf/SKILL.md)"), "{section}");
        assert!(!section.contains("manual") && !section.contains('\\'), "{section}");
        assert!(skills_section(&skills[1..2]).is_none(), "manual skills only: no section");

        let m = skill_message(&skills[2], " focus on forms ").unwrap();
        assert!(m.starts_with("<skill name=\"pdf\" location=\"") && m.contains("Paths in this skill are relative to "), "{m}");
        assert!(m.ends_with("Do the pdf thing.\n</skill>\n\nfocus on forms") && !m.contains("description:"), "{m}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
