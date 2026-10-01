// The colours the TUI draws with. One table, one global index, picked by
// /themes and remembered in model.json — so the palette is data, not literals
// scattered through render.rs.
//
// Every field is a complete SGR sequence, built by the macro from the code, so
// a theme reads as the codes you would write in the source. Theme 0 is what
// rusti has always drawn: the default has to stay put, or an update silently
// restyles everyone's terminal.

use std::sync::atomic::{AtomicUsize, Ordering};

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
        user: "36", info: "34", ok: "32", fail: "31", warn: "33", add: "32", del: "31",
        code: "36", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "36"),
    theme!("recommended", "bright info, inline code joins fenced tan, bold selection",
        user: "36", info: "94", ok: "32", fail: "31", warn: "33", add: "32", del: "31",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("periwinkle", "softer info than bright blue",
        user: "36", info: "38;5;111", ok: "32", fail: "31", warn: "33", add: "32", del: "31",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("teal", "you are teal, calmer beside the greens",
        user: "38;5;37", info: "38;5;111", ok: "32", fail: "31", warn: "33", add: "32", del: "31",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;38;5;37"),
    theme!("violet", "info is not a cold blue",
        user: "36", info: "38;5;141", ok: "32", fail: "31", warn: "33", add: "32", del: "31",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;249", dim: "38;5;245", sel: "1;38;5;141"),
    theme!("one-accent", "greys, with colour only where you act or are asked",
        user: "36", info: "36", ok: "38;5;248", fail: "31", warn: "38;5;248", add: "38;5;248", del: "31",
        code: "38;5;251", fenced: "38;5;251", think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("solarized", "Solarized dark",
        user: "38;5;37", info: "38;5;33", ok: "38;5;100", fail: "38;5;160", warn: "38;5;136",
        add: "38;5;100", del: "38;5;160", code: "38;5;61", fenced: "38;5;109",
        think: "3;38;5;66", dim: "38;5;66", sel: "1;38;5;37"),
    theme!("gruvbox", "warm, low glare",
        user: "38;5;108", info: "38;5;109", ok: "38;5;142", fail: "38;5;167", warn: "38;5;214",
        add: "38;5;142", del: "38;5;167", code: "38;5;175", fenced: "38;5;223",
        think: "3;38;5;245", dim: "38;5;246", sel: "1;38;5;108"),
    theme!("nord", "cool and even",
        user: "38;5;110", info: "38;5;110", ok: "38;5;144", fail: "38;5;131", warn: "38;5;222",
        add: "38;5;144", del: "38;5;131", code: "38;5;139", fenced: "38;5;188",
        think: "3;38;5;103", dim: "38;5;103", sel: "1;38;5;109"),
    theme!("tokyo-night", "high separation between blue, cyan and magenta",
        user: "38;5;117", info: "38;5;111", ok: "38;5;150", fail: "38;5;210", warn: "38;5;180",
        add: "38;5;150", del: "38;5;210", code: "38;5;141", fenced: "38;5;189",
        think: "3;38;5;60", dim: "38;5;103", sel: "1;38;5;117"),
    theme!("catppuccin", "pastel, nothing shouts",
        user: "38;5;116", info: "38;5;111", ok: "38;5;151", fail: "38;5;211", warn: "38;5;223",
        add: "38;5;151", del: "38;5;211", code: "38;5;218", fenced: "38;5;189",
        think: "3;38;5;60", dim: "38;5;103", sel: "1;38;5;116"),
    theme!("high-contrast", "the bright ANSI pair for every marker",
        user: "96", info: "94", ok: "92", fail: "91", warn: "93", add: "92", del: "91",
        code: "38;5;180", fenced: "38;5;180", think: "3;38;5;248", dim: "38;5;247", sel: "1;96"),
    theme!("colourblind", "diff in blue/orange, not red/green",
        user: "36", info: "94", ok: "38;5;74", fail: "38;5;208", warn: "38;5;227",
        add: "38;5;74", del: "38;5;208", code: "38;5;180", fenced: "38;5;180",
        think: "3;38;5;249", dim: "38;5;245", sel: "1;36"),
    theme!("quiet", "desaturated: colour marks the row, text stays calm",
        user: "38;5;73", info: "38;5;67", ok: "38;5;108", fail: "38;5;138", warn: "38;5;179",
        add: "38;5;108", del: "38;5;138", code: "38;5;180", fenced: "38;5;180",
        think: "3;38;5;245", dim: "38;5;243", sel: "1;38;5;73"),
    theme!("amber", "one hue, brightness does the work",
        user: "38;5;214", info: "38;5;221", ok: "38;5;178", fail: "38;5;202", warn: "38;5;222",
        add: "38;5;178", del: "38;5;202", code: "38;5;179", fenced: "38;5;179",
        think: "3;38;5;94", dim: "38;5;136", sel: "1;38;5;222"),
];

static IDX: AtomicUsize = AtomicUsize::new(0);

/// Row backgrounds for the two rows that mean "look here": a failed tool and a
/// tool waiting on your answer. Your own message gets bold text, no fill.
/// Everything else gets at most a coloured edge. Kept apart from the palettes because what reads as subtle
/// depends on the terminal's own background, which rusti cannot see: a pale
/// wash on white is a dark block on black. "light": true in the config picks
/// the pale set.
pub struct Tints {
    pub fail: &'static str,
    pub ask: &'static str,
}
const DARK: Tints = Tints {
    fail: "\x1b[48;2;42;22;24m",
    ask: "\x1b[48;2;42;36;20m",
};
const LIGHT: Tints = Tints {
    fail: "\x1b[48;2;249;230;230m",
    ask: "\x1b[48;2;251;241;214m",
};
static LIGHT_BG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT_BG.store(on, Ordering::Relaxed);
}

pub fn tints() -> &'static Tints {
    if LIGHT_BG.load(Ordering::Relaxed) { &LIGHT } else { &DARK }
}

pub fn current() -> &'static Theme {
    &THEMES[IDX.load(Ordering::Relaxed).min(THEMES.len() - 1)]
}

pub fn index_of(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name.eq_ignore_ascii_case(name.trim()))
}

/// Switch by name. Returns false for a name that is not in the table, so a
/// stale entry in model.json is reported rather than silently ignored.
pub fn set(name: &str) -> bool {
    match index_of(name) {
        Some(i) => {
            IDX.store(i, Ordering::Relaxed);
            true
        }
        None => false,
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
}
