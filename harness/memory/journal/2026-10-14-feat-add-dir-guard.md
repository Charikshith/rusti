## 2026-10-14 - feat-add-dir-guard
- The sandbox had no cargo, cc or libc headers: `apt-get download` works unprivileged against a private Dir::State/Dir::Cache, `dpkg -x` into a scratch dir, then a `cc` wrapper script with -B/-L/-isystem and a rewritten libc.so linker script is enough to build and test (openssl-sys also needed CPATH).
- The old guard used Path::exists(), which is false for a dangling symlink, so a write through one would have created its target outside the root; symlink_metadata() is the right probe.
- Adding built-in slash commands changes the `/re` prefix tests; one command with a --remove flag avoided touching three unrelated tests.
- Tests that mutate a process-global (the /add-dir list) run in parallel: assert on "contains my dir", never "is empty".
