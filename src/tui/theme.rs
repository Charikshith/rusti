// The colours the TUI draws with. One table, one global index, picked by
// /themes and remembered in model.json — so the palette is data, not literals
// scattered through render.rs.
//
// Every field is a complete SGR sequence, built by the macro from the code, so
// a theme reads as the codes you would write in the source. Theme 0 is what
// rusti has always drawn: the default has to stay put, or an update silently
// restyles everyone's terminal. The one exception is a colour that does not
// read: every role is held to 4.5:1 on a dark terminal, and "light" derives
// darker versions of them (lightened() below).
//
// Theme files (F15) join the table at runtime: themes/<name>.json under
// ~/.rusti, and ./.rusti once the folder is trusted, in rusti's own roles,
// inheriting whatever they leave out from a `base` theme:
//   {"name": "my-theme", "note": "…", "base": "nord",
//    "colors": {"user": "#88c0d0", "ok": "38;5;144", "think": "3;38;5;103"},
//    "tints": {"fail": "#2a1618", "ask": "#2a2414", "fail_text": "#e4e4e4", "ask_text": "#e4e4e4"}}
// A value is #rrggbb (24-bit colour) or raw SGR parameters. The active
// theme's file is re-read when it changes, so editing it restyles live.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
    pub note: &'static str,
    pub user: &'static str,   // the user's own "N›" marker and the input prompt
    pub info: &'static str,   // ℹ — notices and the permission question
    pub ok: &'static str,     // ✓
    pub fail: &'static str,   // ✗
    pub warn: &'static str,   // ⚠ and the running spinner
    pub add: &'static str,    // a "+" hunk row
    pub del: &'static str,    // a "-" hunk row
    pub code: &'static str,   // inline `code`
    pub fenced: &'static str, // a fenced block's body
    pub think: &'static str,  // reasoning_content
    pub dim: &'static str,    // tool output, stats, everything secondary
    pub sel: &'static str,    // the selected row in a picker
    // Syntax colours. None derives from the palette above, so a new theme is
    // still one decision; Some overrides it where a theme wants its own.
    syn_key: Option<&'static str>,
    syn_str: Option<&'static str>,
    syn_num: Option<&'static str>,
    syn_bool: Option<&'static str>,
    // A theme file's row backgrounds and the text on them; None keeps the
    // light/dark default.
    tint_fail: Option<&'static str>,
    tint_ask: Option<&'static str>,
    tint_fail_text: Option<&'static str>,
    tint_ask_text: Option<&'static str>,
}

impl Theme {
    /// A table's header, and the rule under it.
    pub fn head(&self) -> &'static str {
        self.syn_key.unwrap_or(self.user)
    }
    pub fn rule(&self) -> &'static str {
        self.dim
    }
    /// Code tokens. A key is a string in a position that names something, so it
    /// takes the accent; punctuation is structure, so it recedes to grey.
    pub fn key(&self) -> &'static str {
        self.syn_key.unwrap_or(self.user)
    }
    pub fn string(&self) -> &'static str {
        self.syn_str.unwrap_or(self.ok)
    }
    pub fn num(&self) -> &'static str {
        self.syn_num.unwrap_or(self.code)
    }
    pub fn boolean(&self) -> &'static str {
        self.syn_bool.unwrap_or(self.info)
    }
    pub fn punct(&self) -> &'static str {
        self.dim
    }
}

macro_rules! theme {
    ($name:literal, $note:literal, $($f:ident: $c:literal),+ $(,)?) => {
        theme!(@build $name, $note, [None, None, None, None], $($f: $c),+)
    };
    // the syn arm: key, string, number, boolean spelled out
    ($name:literal, $note:literal, syn[$k:literal, $s:literal, $n:literal, $b:literal],
     $($f:ident: $c:literal),+ $(,)?) => {
        theme!(@build $name, $note,
            [Some(concat!("\x1b[", $k, "m")), Some(concat!("\x1b[", $s, "m")),
             Some(concat!("\x1b[", $n, "m")), Some(concat!("\x1b[", $b, "m"))],
            $($f: $c),+)
    };
    (@build $name:literal, $note:literal, [$k:expr, $s:expr, $n:expr, $b:expr],
     $($f:ident: $c:literal),+) => {
        Theme {
            name: $name, note: $note,
            syn_key: $k, syn_str: $s, syn_num: $n, syn_bool: $b,
            tint_fail: None, tint_ask: None, tint_fail_text: None, tint_ask_text: None,
            $($f: concat!("\x1b[", $c, "m")),+
        }
    };
}

#[rustfmt::skip]
pub const THEMES: &[Theme] = &[
    // The syn colours are Nord's — chosen off the sample sheet — while every
    // colour that was already on screen stays exactly as it was.
    theme!("current", "what rusti has always drawn, plus Nord syntax colours",
        syn["38;5;110", "38;5;144", "38;5;139", "38;5;110"],
        user: "36", info: "94", ok: "32", fail: "38;5;203", warn: "33", add: "32", del: "38;5;203",
        code: "36", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "36"),
    theme!("recommended", "bright info, inline code joins fenced tan, bold selection",
        user: "36", info: "94", ok: "32", fail: "38;5;203", warn: "33", add: "32", del: "38;5;203",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("periwinkle", "softer info than bright blue",
        user: "36", info: "38;5;111", ok: "32", fail: "38;5;203", warn: "33", add: "32", del: "38;5;203",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("teal", "you are teal, calmer beside the greens",
        user: "38;5;37", info: "38;5;111", ok: "32", fail: "38;5;203", warn: "33", add: "32", del: "38;5;203",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;38;5;37"),
    theme!("violet", "info is not a cold blue",
        user: "36", info: "38;5;141", ok: "32", fail: "38;5;203", warn: "33", add: "32", del: "38;5;203",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;38;5;141"),
    theme!("one-accent", "greys, with colour only where you act or are asked",
        user: "36", info: "36", ok: "38;5;248", fail: "38;5;203", warn: "38;5;248", add: "38;5;248", del: "38;5;203",
        code: "38;5;251", fenced: "38;5;251", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("solarized", "Solarized dark",
        user: "38;5;37", info: "38;5;33", ok: "38;5;100", fail: "38;5;167", warn: "38;5;136",
        add: "38;5;100", del: "38;5;167", code: "38;5;104", fenced: "38;5;109",
        think: "3;38;5;66", dim: "38;5;66", sel: "1;38;5;37"),
    theme!("gruvbox", "warm, low glare",
        user: "38;5;108", info: "38;5;109", ok: "38;5;142", fail: "38;5;167", warn: "38;5;214",
        add: "38;5;142", del: "38;5;167", code: "38;5;175", fenced: "38;5;223",
        think: "3;38;5;245", dim: "38;5;246", sel: "1;38;5;108"),
    theme!("nord", "cool and even",
        user: "38;5;110", info: "38;5;110", ok: "38;5;144", fail: "38;5;167", warn: "38;5;222",
        add: "38;5;144", del: "38;5;167", code: "38;5;139", fenced: "38;5;188",
        think: "3;38;5;103", dim: "38;5;103", sel: "1;38;5;109"),
    theme!("tokyo-night", "high separation between blue, cyan and magenta",
        user: "38;5;117", info: "38;5;111", ok: "38;5;150", fail: "38;5;210", warn: "38;5;180",
        add: "38;5;150", del: "38;5;210", code: "38;5;141", fenced: "38;5;189",
        think: "3;38;5;104", dim: "38;5;103", sel: "1;38;5;117"),
    theme!("catppuccin", "pastel, nothing shouts",
        user: "38;5;116", info: "38;5;111", ok: "38;5;151", fail: "38;5;211", warn: "38;5;223",
        add: "38;5;151", del: "38;5;211", code: "38;5;218", fenced: "38;5;189",
        think: "3;38;5;104", dim: "38;5;103", sel: "1;38;5;116"),
    theme!("high-contrast", "the bright ANSI pair for every marker",
        user: "96", info: "94", ok: "92", fail: "38;5;203", warn: "93", add: "92", del: "91",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;248", dim: "38;5;247", sel: "1;96"),
    theme!("colourblind", "diff in blue/orange, not red/green",
        user: "36", info: "94", ok: "38;5;74", fail: "38;5;208", warn: "38;5;227",
        add: "38;5;74", del: "38;5;208", code: "38;5;180", fenced: "38;5;180",
        think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("quiet", "desaturated: colour marks the row, text stays calm",
        user: "38;5;73", info: "38;5;67", ok: "38;5;108", fail: "38;5;138", warn: "38;5;179",
        add: "38;5;108", del: "38;5;138", code: "38;5;180", fenced: "38;5;180",
        think: "3;38;5;245", dim: "38;5;244", sel: "1;38;5;73"),
    theme!("amber", "one hue, brightness does the work",
        user: "38;5;214", info: "38;5;221", ok: "38;5;178", fail: "38;5;202", warn: "38;5;222",
        add: "38;5;178", del: "38;5;202", code: "38;5;179", fenced: "38;5;179",
        think: "3;38;5;172", dim: "38;5;136", sel: "1;38;5;222"),
];

static CURRENT: RwLock<&'static Theme> = RwLock::new(&THEMES[0]);

/// Row backgrounds for the two rows that mean "look here": a failed tool and a
/// tool waiting on your answer. Your own message gets bold text, no fill.
/// Everything else gets at most a coloured edge. Kept apart from the palettes because what reads as subtle
/// depends on the terminal's own background, which rusti cannot see: a pale
/// wash on white is a dark block on black. "light": true in the config picks
/// the pale set. Each fill carries its own text colour: left to the terminal,
/// the text is its default foreground, and "light" left on under a dark
/// terminal put light grey on pale pink (1.3:1).
#[derive(Clone, Copy)]
pub struct Tints {
    pub fail: &'static str,
    pub ask: &'static str,
    pub fail_text: &'static str,
    pub ask_text: &'static str,
}
const DARK: Tints = Tints {
    fail: "\x1b[48;2;42;22;24m",
    ask: "\x1b[48;2;42;36;20m",
    fail_text: "\x1b[38;2;228;228;228m",
    ask_text: "\x1b[38;2;228;228;228m",
};
const LIGHT: Tints = Tints {
    fail: "\x1b[48;2;249;230;230m",
    ask: "\x1b[48;2;251;241;214m",
    fail_text: "\x1b[38;2;28;28;28m",
    ask_text: "\x1b[38;2;28;28;28m",
};
static LIGHT_BG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT_BG.store(on, Ordering::Relaxed);
}

/// The light/dark row tints, with the active theme file's own over them.
pub fn tints() -> Tints {
    tints_for(current(), LIGHT_BG.load(Ordering::Relaxed))
}

fn tints_for(t: &Theme, light: bool) -> Tints {
    let base = if light { LIGHT } else { DARK };
    Tints {
        fail: t.tint_fail.unwrap_or(base.fail),
        ask: t.tint_ask.unwrap_or(base.ask),
        fail_text: t.tint_fail_text.unwrap_or(t.tint_fail.map_or(base.fail_text, text_on)),
        ask_text: t.tint_ask_text.unwrap_or(t.tint_ask.map_or(base.ask_text, text_on)),
    }
}

/// Text for a theme file's fill that names no text: whichever of the two
/// defaults reads better on it when the fill is a 24-bit background (#rrggbb
/// or 48;2;r;g;b), or the terminal's own for any other SGR.
fn text_on(fill: &str) -> &'static str {
    let Some(bg) = rgb(fill).filter(|_| fill.starts_with("[48;")).map(lum) else { return "" };
    let ratio = |c: [f64; 3]| { let (a, b) = (lum(c), bg); (a.max(b) + 0.05) / (a.min(b) + 0.05) };
    if ratio([28.0; 3]) >= ratio([228.0; 3]) { LIGHT.fail_text } else { DARK.fail_text }
}

/// The colour in a 24-bit SGR ("[48;2;r;g;bm"), if it is one.
fn rgb(sgr: &str) -> Option<[f64; 3]> {
    let p: Vec<u8> = sgr.strip_prefix("[")?.strip_suffix('m')?.split(';').map(|v| v.parse().ok()).collect::<Option<_>>()?;
    match p[..] {
        [38 | 48, 2, r, g, b] => Some([r, g, b].map(f64::from)),
        _ => None,
    }
}

/// WCAG 2 relative luminance.
fn lum(c: [f64; 3]) -> f64 {
    let l = c.map(|v| { let v = v / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } });
    0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
}

/// The active theme, as drawn: under "light" its colours are darkened until
/// they read on a pale background.
pub fn current() -> &'static Theme {
    let t = *CURRENT.read().unwrap_or_else(|e| e.into_inner());
    if LIGHT_BG.load(Ordering::Relaxed) { lightened(t) } else { t }
}

/// The built-in palettes are made for a dark terminal; on a light one most of
/// their greys, tans and yellows fell under 3:1. Each theme's light version is
/// derived once and kept, keyed by the theme it came from.
fn lightened(t: &'static Theme) -> &'static Theme {
    static DONE: Mutex<Vec<(usize, &'static Theme)>> = Mutex::new(Vec::new());
    let key = t as *const Theme as usize;
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, l)) = done.iter().find(|d| d.0 == key) {
        return l;
    }
    // the darkest of the pale backgrounds a text colour sits on
    let bg = fg_rgb(LIGHT.fail).unwrap_or([255.0; 3]);
    let mut l = t.clone();
    for f in [&mut l.user, &mut l.info, &mut l.ok, &mut l.fail, &mut l.warn, &mut l.add, &mut l.del,
              &mut l.code, &mut l.fenced, &mut l.think, &mut l.dim, &mut l.sel] {
        *f = readable_on(f, bg);
    }
    for f in [&mut l.syn_key, &mut l.syn_str, &mut l.syn_num, &mut l.syn_bool].into_iter().flatten() {
        *f = readable_on(f, bg);
    }
    let l: &'static Theme = Box::leak(Box::new(l));
    done.push((key, l));
    l
}

/// WCAG 2 contrast of two colours.
fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (a, b) = (lum(a), lum(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `sgr` darkened until it reads at 4.5:1 on the light `bg`, its other
/// attributes (bold, italic) kept. A colour that already reads is left alone.
fn readable_on(sgr: &'static str, bg: [f64; 3]) -> &'static str {
    let Some(c) = fg_rgb(sgr) else { return sgr };
    if contrast(c, bg) >= 4.5 {
        return sgr;
    }
    let mut k = 1.0;
    while contrast(c.map(|v| v * k), bg) < 4.5 {
        k -= 0.05; // black reads at ~18:1, so this ends
    }
    let [r, g, b] = c.map(|v| (v * k).round() as u8);
    let (p, mut attrs, mut i) = (sgr_params(sgr), String::new(), 0);
    while i < p.len() {
        match p[i] {
            38 | 48 => i += if p.get(i + 1) == Some(&5) { 3 } else { 5 }, // a colour and its arguments
            a @ 1..=9 => {
                attrs.push_str(&format!("{a};"));
                i += 1;
            }
            _ => i += 1,
        }
    }
    leak(format!("\x1b[{attrs}38;2;{r};{g};{b}m"))
}

/// An SGR sequence's parameters.
fn sgr_params(sgr: &str) -> Vec<u16> {
    let p = sgr.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('m')).unwrap_or("");
    p.split(';').filter_map(|v| v.parse().ok()).collect()
}

/// The colour an SGR sequence sets (text or background), as RGB: 24-bit,
/// 256-colour, or one of the 16 ANSI colours.
/// ponytail: the 16 are the terminal's to choose; these are Windows Terminal's
/// default (Campbell), the palette the contrast was measured in.
fn fg_rgb(sgr: &str) -> Option<[f64; 3]> {
    let p = sgr_params(sgr);
    (0..p.len()).find_map(|i| match p[i..] {
        [38 | 48, 2, r, g, b, ..] => Some([r, g, b].map(f64::from)),
        [38 | 48, 5, n, ..] => Some(xterm(n)),
        [c @ 30..=37, ..] => Some(xterm(c - 30)),
        [c @ 90..=97, ..] => Some(xterm(c - 82)),
        _ => None,
    })
}

/// The 256-colour palette entry n.
fn xterm(n: u16) -> [f64; 3] {
    const ANSI: [u32; 16] = [
        0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC,
        0x767676, 0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
    ];
    let step = |v: u16| if v == 0 { 0.0 } else { f64::from(55 + 40 * v) };
    match n {
        0..=15 => {
            let v = ANSI[n as usize];
            [v >> 16, v >> 8 & 0xff, v & 0xff].map(f64::from)
        }
        16..=231 => {
            let n = n - 16;
            [step(n / 36), step(n / 6 % 6), step(n % 6)]
        }
        _ => [f64::from(8 + 10 * (n.min(255) - 232)); 3],
    }
}

/// Theme files as read: (file, its modified time then, the theme).
static LOADED: Mutex<Vec<(PathBuf, Option<SystemTime>, &'static Theme)>> = Mutex::new(Vec::new());

/// Built-ins, then the loaded theme files: what /themes lists.
pub fn all() -> Vec<&'static Theme> {
    let loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    THEMES.iter().chain(loaded.iter().map(|l| l.2)).collect()
}

pub fn index_of(name: &str) -> Option<usize> {
    all().iter().position(|t| t.name.eq_ignore_ascii_case(name.trim()))
}

fn find(name: &str) -> Option<&'static Theme> {
    all().into_iter().find(|t| t.name.eq_ignore_ascii_case(name.trim()))
}

/// Switch by name. Returns false for a name that is not in the table, so a
/// stale entry in model.json is reported rather than silently ignored.
pub fn set(name: &str) -> bool {
    match find(name) {
        Some(t) => {
            *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = t;
            true
        }
        None => false,
    }
}

/// ponytail: a loaded theme's strings live for the whole process, since
/// every field is &'static. A few hundred bytes per load or edit of a theme
/// file; intern them if anyone ever edits one thousands of times a session.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// A colour value as an SGR sequence: #rrggbb is 24-bit on `layer` (38 for
/// text, 48 for a background); anything else must already be SGR parameters.
fn sgr(v: &serde_json::Value, layer: u8) -> Result<&'static str, String> {
    let s = v.as_str().ok_or_else(|| format!("{v} is not a string"))?.trim();
    let code = match s.strip_prefix('#') {
        Some(h) if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) => {
            let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0);
            format!("{layer};2;{};{};{}", c(0), c(2), c(4))
        }
        None if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == ';') => s.to_string(),
        _ => return Err(format!("{s:?} is neither #rrggbb nor SGR parameters like 38;5;110")),
    };
    Ok(leak(format!("\x1b[{code}m")))
}

const ROLES: &str = "user info ok fail warn add del code fenced think dim sel key string number boolean";

/// One theme file. `stem` names it when it has no "name"; `known` finds its
/// base. A typo in a role or a value fails the file, with the reason.
fn parse(text: &str, stem: &str, known: &dyn Fn(&str) -> Option<&'static Theme>) -> Result<Theme, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let o = v.as_object().ok_or("not a JSON object")?;
    let get = |k: &str| o.get(k).and_then(serde_json::Value::as_str);
    let name = get("name").unwrap_or(stem).trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err(format!("name {name:?} must be one word, to be typed as /themes <name>"));
    }
    let mut t = match get("base") {
        Some(b) => known(b).ok_or_else(|| format!("no base theme called {b}"))?.clone(),
        None => THEMES[0].clone(),
    };
    t.name = leak(name.to_string());
    t.note = leak(get("note").map_or_else(|| format!("{stem}.json, based on {}", get("base").unwrap_or(THEMES[0].name)), str::to_string));
    let empty = serde_json::Map::new();
    let section = |k: &str| o.get(k).and_then(serde_json::Value::as_object).unwrap_or(&empty);
    for (role, val) in section("colors") {
        let c = sgr(val, 38).map_err(|e| format!("colors.{role}: {e}"))?;
        match role.as_str() {
            "user" => t.user = c,
            "info" => t.info = c,
            "ok" => t.ok = c,
            "fail" => t.fail = c,
            "warn" => t.warn = c,
            "add" => t.add = c,
            "del" => t.del = c,
            "code" => t.code = c,
            "fenced" => t.fenced = c,
            "think" => t.think = c,
            "dim" => t.dim = c,
            "sel" => t.sel = c,
            "key" => t.syn_key = Some(c),
            "string" => t.syn_str = Some(c),
            "number" => t.syn_num = Some(c),
            "boolean" => t.syn_bool = Some(c),
            r => return Err(format!("no colour role called {r} (roles: {ROLES})")),
        }
    }
    let tints = section("tints");
    if tints.contains_key("fail") { t.tint_fail_text = None }
    if tints.contains_key("ask") { t.tint_ask_text = None }
    for (row, val) in tints {
        let layer = if row.ends_with("_text") { 38 } else { 48 };
        let c = sgr(val, layer).map_err(|e| format!("tints.{row}: {e}"))?;
        match row.as_str() {
            "fail" => t.tint_fail = Some(c),
            "ask" => t.tint_ask = Some(c),
            "fail_text" => t.tint_fail_text = Some(c),
            "ask_text" => t.tint_ask_text = Some(c),
            r => return Err(format!("no tint called {r} (tints: fail ask fail_text ask_text)")),
        }
    }
    Ok(t)
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

fn shown(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Read every themes/<name>.json in `dirs`, replacing what was loaded before;
/// one warning per file that does not load. A built-in's name is not taken
/// over, and the first folder to use a name keeps it. A base may be a
/// built-in or a theme file read before this one (by folder, then name).
pub fn load(dirs: &[(PathBuf, &'static str)]) -> Vec<String> {
    let mut warn = Vec::new();
    let mut out: Vec<(PathBuf, Option<SystemTime>, &'static Theme)> = Vec::new();
    for (dir, _) in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for p in files {
            let t = {
                let known = |n: &str| THEMES.iter().chain(out.iter().map(|l| l.2)).find(|t| t.name.eq_ignore_ascii_case(n.trim()));
                std::fs::read_to_string(&p)
                    .map_err(|e| e.to_string())
                    .and_then(|text| parse(&text, &stem(&p), &known))
                    .and_then(|t| match known(t.name) {
                        Some(_) => Err(format!("{} is already taken, skipped", t.name)),
                        None => Ok(t),
                    })
            };
            match t {
                Ok(t) => out.push((p.clone(), mtime(&p), Box::leak(Box::new(t)))),
                Err(e) => warn.push(format!("theme {}: {e}", shown(&p))),
            }
        }
    }
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = out;
    // the active theme may be one just re-read: point at its new version
    if let Some(t) = find(current().name) {
        *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = t;
    }
    warn
}

/// The loaded theme called `name`, re-read when its file changed since:
/// None when it is unchanged (or not a file), else the new version or why
/// it does not load. A file that fails is reported once per change.
fn refresh(name: &str) -> Option<Result<&'static Theme, String>> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let i = loaded.iter().position(|l| l.2.name == name)?;
    let now = mtime(&loaded[i].0);
    if now == loaded[i].1 {
        return None;
    }
    loaded[i].1 = now;
    let p = loaded[i].0.clone();
    let others: Vec<&'static Theme> = THEMES.iter().chain(loaded.iter().filter(|l| l.0 != p).map(|l| l.2)).collect();
    let known = |n: &str| others.iter().copied().find(|t| t.name.eq_ignore_ascii_case(n.trim()));
    let r = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|text| parse(&text, &stem(&p), &known));
    Some(match r {
        Ok(t) => {
            let t: &'static Theme = Box::leak(Box::new(t));
            loaded[i].2 = t;
            Ok(t)
        }
        Err(e) => Err(format!("theme {}: {e}", shown(&p))),
    })
}

/// Hot reload, from the render loop about once a second: an edit to the
/// active theme's file applies now, and one that breaks it says why and
/// keeps the last good version.
pub fn reload_if_changed() -> Option<String> {
    match refresh(current().name)? {
        Ok(t) => {
            *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = t;
            None
        }
        Err(e) => Some(e),
    }
}

pub fn name() -> &'static str {
    current().name
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is written by hand, so the checks are the ones a typo breaks:
    /// every escape well-formed, every name unique, and the default unmoved.
    #[test]
    fn every_theme_is_a_usable_palette() {
        assert_eq!(THEMES[0].name, "current", "theme 0 is the default and must not move");
        assert_eq!(THEMES[0].user, "\x1b[36m", "the macro must build a real SGR sequence");
        for t in THEMES {
            for (field, seq) in [("user", t.user), ("info", t.info), ("ok", t.ok), ("fail", t.fail),
                                 ("warn", t.warn), ("add", t.add), ("del", t.del), ("code", t.code),
                                 ("fenced", t.fenced), ("think", t.think), ("dim", t.dim), ("sel", t.sel)] {
                assert!(seq.starts_with("\x1b[") && seq.ends_with('m') && seq.len() > 3,
                    "{}.{field} is not an SGR sequence: {seq:?}", t.name);
            }
            assert_eq!(THEMES.iter().filter(|o| o.name == t.name).count(), 1, "duplicate name {}", t.name);
            assert!(!t.name.contains(' '), "{} must be typeable as /themes <name>", t.name);
        }
        // Lookup only — set() writes a global the render tests read, and cargo
        // runs tests in parallel, so flipping it here would make them flaky.
        assert_eq!(index_of("amber"), Some(THEMES.len() - 1));
        assert_eq!(index_of("CURRENT"), Some(0), "names are case-insensitive");
        assert_eq!(index_of("  nord  "), index_of("nord"), "a typed name arrives padded");
        assert_eq!(index_of("no-such-theme"), None);
    }

    /// A failed or asking row's text must read on its own fill in every theme,
    /// light flag on or off: the fill used to leave the text to the terminal,
    /// and a stale "light" put its light grey on pale pink at 1.3:1.
    #[test]
    fn tinted_rows_are_readable_in_every_theme_and_mode() {
        // WCAG 2 contrast of two 24-bit SGR colours ("\x1b[38;2;r;g;bm")
        let rgb = |sgr: &str| -> [f64; 3] {
            let p: Vec<f64> = sgr.trim_start_matches("\x1b[").trim_end_matches('m').split(';')
                .skip(2).map(|v| v.parse().unwrap()).collect();
            assert_eq!(p.len(), 3, "not a 24-bit colour: {sgr:?}");
            [p[0], p[1], p[2]]
        };
        let lum = |c: [f64; 3]| -> f64 {
            let l = c.map(|v| { let v = v / 255.0; if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) } });
            0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2]
        };
        let ratio = |a: &str, b: &str| {
            let (a, b) = (lum(rgb(a)), lum(rgb(b)));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for t in THEMES {
            for light in [false, true] {
                let k = tints_for(t, light);
                for (row, text, bg) in [("fail", k.fail_text, k.fail), ("ask", k.ask_text, k.ask)] {
                    let r = ratio(text, bg);
                    assert!(r >= 4.5, "{} light={light}: {row} row text is {r:.2}:1 on its fill", t.name);
                }
            }
        }
        // F13: every role reads on the terminal it is drawn on: the palettes
        // on a dark one (Campbell's black, where the "plan" label's 34 was
        // 2.4:1), with the ✗ also on its fail fill; their light versions on
        // white and on the pale fills
        let roles = |t: &Theme| [("user", t.user), ("info", t.info), ("ok", t.ok), ("fail", t.fail),
            ("warn", t.warn), ("add", t.add), ("del", t.del), ("code", t.code), ("fenced", t.fenced),
            ("think", t.think), ("dim", t.dim), ("sel", t.sel), ("key", t.key()), ("string", t.string()),
            ("number", t.num()), ("boolean", t.boolean())];
        let on = |sgr: &str, bg: [f64; 3]| contrast(fg_rgb(sgr).unwrap(), bg);
        for t in THEMES {
            for (role, sgr) in roles(t) {
                let r = on(sgr, [12.0; 3]);
                assert!(r >= 4.5, "{}: {role} {sgr:?} is {r:.2}:1 on a dark terminal", t.name);
            }
            let r = on(t.fail, fg_rgb(DARK.fail).unwrap());
            assert!(r >= 4.5, "{}: the ✗ is {r:.2}:1 on its fail fill", t.name);
            let l = lightened(t);
            assert!(std::ptr::eq(l, lightened(t)), "derived once");
            for (role, sgr) in roles(l) {
                for bg in [[255.0; 3], fg_rgb(LIGHT.fail).unwrap(), fg_rgb(LIGHT.ask).unwrap()] {
                    let r = on(sgr, bg);
                    assert!(r >= 4.5, "{} light: {role} {sgr:?} is {r:.2}:1 on {bg:?}", t.name);
                }
            }
        }
        // darkening keeps what is not colour: italic reasoning stays italic,
        // and a colour that already reads on white is not touched
        assert!(lightened(&THEMES[0]).think.starts_with("\x1b[3;38;2;"), "{:?}", lightened(&THEMES[0]).think);
        assert_eq!(readable_on("\x1b[38;5;24m", [255.0; 3]), "\x1b[38;5;24m");
        // a theme file's fill with no text of its own still reads, in either
        // mode, even over a base file that pinned text for its own fill
        let none = |_: &str| None;
        let base: &'static Theme = Box::leak(Box::new(parse(r##"{"tints":{"fail":"#2a1618","fail_text":"#e4e4e4"}}"##, "b", &none).unwrap()));
        let pale = parse(r##"{"base":"b","tints":{"fail":"#f9e6e6","ask":"#3a2a10"}}"##, "p", &|_| Some(base)).unwrap();
        for light in [false, true] {
            let k = tints_for(&pale, light);
            assert_eq!((k.fail_text, k.ask_text), (LIGHT.fail_text, DARK.fail_text), "light={light}");
            assert!(ratio(k.fail_text, k.fail) >= 4.5 && ratio(k.ask_text, k.ask) >= 4.5, "light={light}");
        }
        let sgr = parse(r#"{"tints":{"fail":"48;5;224"}}"#, "s", &none).unwrap();
        assert_eq!(tints_for(&sgr, false).fail_text, "", "a fill rusti cannot measure leaves the text to the terminal");
        assert_eq!((DARK.fail, DARK.fail_text), ("\x1b[48;2;42;22;24m", "\x1b[38;2;228;228;228m"), "the captain's pick: #2A1618 under #E4E4E4");
        assert_eq!((DARK.ask, DARK.ask_text), ("\x1b[48;2;42;36;20m", "\x1b[38;2;228;228;228m"));
    }

    /// F15: a theme file is rusti's roles over a base, values as hex or SGR;
    /// a typo fails the file with the reason instead of half-applying it.
    #[test]
    fn theme_files_load_inherit_and_hot_reload() {
        let none = |_: &str| None;
        let nord = |n: &str| THEMES.iter().find(|t| t.name == n);
        let t = parse(r##"{"name":"mine","note":"n","base":"nord","colors":{"user":"#88c0d0","ok":"38;5;144","key":"1;33"},"tints":{"fail":"#2a1618","ask_text":"#1c1c1c"}}"##, "f", &nord).unwrap();
        let base = nord("nord").unwrap();
        assert_eq!((t.name, t.note), ("mine", "n"));
        assert_eq!((t.user, t.ok, t.key()), ("\x1b[38;2;136;192;208m", "\x1b[38;5;144m", "\x1b[1;33m"));
        assert_eq!((t.info, t.think, t.dim), (base.info, base.think, base.dim), "what the file leaves out comes from its base");
        assert_eq!((t.tint_fail, t.tint_ask), (Some("\x1b[48;2;42;22;24m"), None), "a tint is a background");
        assert_eq!((t.tint_ask_text, t.tint_fail_text), (Some("\x1b[38;2;28;28;28m"), None), "a tint's _text is a foreground");

        let t = parse(r#"{"colors":{"user":"38;5;37"}}"#, "teal-ish", &none).unwrap();
        assert_eq!((t.name, t.dim), ("teal-ish", THEMES[0].dim), "no name: the file's; no base: the default");
        assert_eq!(t.string(), THEMES[0].string(), "the default's own syntax colours are inherited");
        for (bad, why) in [
            (r##"{"colors":{"usr":"#000000"}}"##, "no colour role called usr"),
            (r#"{"colors":{"user":"blue"}}"#, "colors.user: \"blue\" is neither"),
            (r##"{"colors":{"user":"#12345"}}"##, "is neither"),
            (r##"{"tints":{"user":"#000000"}}"##, "no tint called user"),
            (r#"{"base":"nope"}"#, "no base theme called nope"),
            (r#"{"name":"two words"}"#, "must be one word"),
            ("[1]", "not a JSON object"),
        ] {
            let e = parse(bad, "x", &none).err().unwrap_or_default();
            assert!(e.contains(why), "{bad}: {e}");
        }

        let dir = std::env::temp_dir().join(format!("rusti_themes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-dark.json"), r##"{"name":"rusti-test-dark","base":"gruvbox","colors":{"ok":"#00ff00"}}"##).unwrap();
        std::fs::write(dir.join("b-light.json"), r#"{"name":"rusti-test-light","base":"rusti-test-dark"}"#).unwrap();
        std::fs::write(dir.join("c.json"), r#"{"name":"nord"}"#).unwrap();
        std::fs::write(dir.join("d.json"), "{ broken").unwrap();
        let warn = load(&[(dir.clone(), "u")]);
        assert_eq!(warn.len(), 2, "{warn:?}");
        assert!(warn[0].contains("nord is already taken") && warn[1].contains("d.json"), "{warn:?}");
        let light = find("rusti-test-light").expect("loaded, based on another theme file");
        assert_eq!(light.ok, "\x1b[38;2;0;255;0m");
        assert!(all().len() == THEMES.len() + 2 && index_of("RUSTI-TEST-DARK").is_some());

        // an edit is picked up by mtime; a broken edit is reported once and the last good version kept
        let later = |secs| std::time::SystemTime::now() + std::time::Duration::from_secs(secs);
        let f = dir.join("a-dark.json");
        assert!(refresh("rusti-test-dark").is_none(), "unchanged");
        std::fs::write(&f, r##"{"name":"rusti-test-dark","colors":{"ok":"#0000ff"}}"##).unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(later(60)).unwrap();
        assert_eq!(refresh("rusti-test-dark").unwrap().unwrap().ok, "\x1b[38;2;0;0;255m");
        assert_eq!(find("rusti-test-dark").unwrap().ok, "\x1b[38;2;0;0;255m", "the table has the new version");
        std::fs::write(&f, "{ oops").unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(later(120)).unwrap();
        assert!(refresh("rusti-test-dark").unwrap().err().unwrap().contains("a-dark.json"));
        assert!(refresh("rusti-test-dark").is_none(), "reported once");
        assert_eq!(find("rusti-test-dark").unwrap().ok, "\x1b[38;2;0;0;255m");
        load(&[]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
