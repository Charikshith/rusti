# A silent default that is later saved destroys the file it defaulted for

**Seen:** 2026-09-14, `Config::load_from` (feat-056). Filed as "swallows a malformed
model.json silently" in session 19; the real cost only appeared once `/settings` and
`/mcp` started writing to that same file.

## The shape

```rust
std::fs::read_to_string(path).ok().and_then(|s| from_str(&s).ok()).unwrap_or_default()
```

Reads as forgiving. It merges three different outcomes into one value:

| reality | what the caller sees |
|---|---|
| no file — first run | empty config ✅ correct |
| file unreadable (locked, permissions) | empty config ❌ |
| file present but one key is wrong | empty config ❌ |

A silent default is survivable on its own — you get a confusing "no saved models" and go
looking. What makes it destructive is the **load → mutate → save** round trip that any
settings toggle performs: the defaults get serialized back over the real file, and the
typo that caused it is now the least of the losses.

## The rule

**A loader may only default for the case that is genuinely absent.** Anything else that
failed must be remembered, and the writer must refuse while it is. Distinguish
`ErrorKind::NotFound` from every other read error; treat a parse error as a hard failure
that keeps the process running but blocks the save.

Put the refusal in the single `save` function, not at each call site. There were three
writers here and the next feature adds a fourth.

## Tell

Grep for `.ok()` / `unwrap_or_default()` on a config or state load, then ask: *does anything
ever write this struct back?* If yes, the silent default is a delete command with a delay.
