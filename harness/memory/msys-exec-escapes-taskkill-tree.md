# taskkill /T cannot follow a Git Bash command's tree; a job object can

Under Git Bash (MSYS), an exec of one MSYS program by another starts a new Windows process
whose Windows parent has already exited. `taskkill /T /F /PID <bash>` walks parent ids, so it
kills the shell and any native program it started directly (`cd . && python …`), but not
`sleep`, an inner `sh -c '…'`, or anything those start, such as the `node` under an `npm`
script. A Windows job object holds every one of them, and `TerminateJobObject` kills them all.

**Why:** In the 2026-10-10 feat-bughunt-hangs session, the tree-kill regression test (a
timed-out `sh -c 'sleep 2; echo late > m'`) still wrote its marker after
`taskkill /T` reported SUCCESS. A probe of `Win32_Process` showed `sh.exe`'s parent pid was
a process that no longer existed. The same probe with a job object assigned right after the
spawn left nothing running.

**How to apply:** To kill a command's tree on Windows, put the child in a job object at spawn
time (`tools::Tree`). Never trust `taskkill /T` alone for anything that runs through Git Bash.
On Unix the equivalent is a process group (`process_group(0)`) killed as `-pgid` while the
child is still unreaped.
