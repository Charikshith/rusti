## 2026-10-10 - fail/ask rows pin their text colour
- Surprise: the pale pink came back because the earlier fix trusted a wrong note ("set_light is never called"). One `grep set_light` would have shown the call in app.rs. Check a "dead code" claim before building a fix on it.
- Surprise: the captain's config had `"light": true` from when their terminal was white. Any setting rusti cannot verify (terminal background) can go stale silently, so a fill must never rely on the terminal's default foreground.
- Would do differently: give a background and its foreground together from the start; a fill without a text colour is only half a colour decision.
