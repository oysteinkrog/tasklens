# Brief for implementation agents (one bead each)

Read `AGENTS.md` first and follow it. Repo: `/home/oystein/work/tasklens`, branch `main`, shared
with up to 8 other agents working on other files at the same time. Your bead ID is in your prompt.
Do not pick up any other bead.

1. Claim: `br update <id> --status in_progress --no-auto-flush --lock-timeout 180000`.
2. Read `br show <id>` fully, the docs it cites, the code of the beads it depends on, and the real
   library API on disk (`~/.cargo/registry/src/*/<crate>-<version>/`) before you call it.
3. Run `git diff --stat -- <your files>`. If a file you must edit has uncommitted changes that are
   not yours, stop and report.
4. Implement exactly the Deliverable and Acceptance Criteria. Edit only the bead's files. For a
   chain edit in a shared file (for example `xtask/src/main.rs`, `lib.rs` module lines), re-read
   the file right before editing, add only your lines, and commit at once.
5. Commit right after writing files, with a pathspec:
   `git add <files> && git commit -m "<crate>(<id>): <what>" -- <files>`, ending the message with
   `Co-Authored-By: Claude <your model name> <noreply@anthropic.com>` and
   `Claude-Session: https://claude.ai/code/session_01Bmy1XwAdJ5bM8C4UG8Lb4g`.
   If `Cargo.lock` changes because of your dependency lines, commit it with your files. On
   `index.lock`, wait and retry; never delete it.
6. Run the bead's tests, `cargo fmt --check` and `cargo clippy -p <crate> --all-targets -- -D
   warnings`, all through `heavy-build`. Builds queue on one machine-wide lock, so start them in
   the background and wait for the result; always read the output before you report. Completion
   notices for background commands are sometimes lost, so do not go idle waiting for one: write
   the build output to a file and wait with a foreground loop such as
   `until grep -qE '^(EXIT|exit) ' out.txt; do sleep 10; done` (append `; echo "EXIT $?"` to the
   build command), then read the file. If another
   agent's half-finished work breaks the workspace build, wait a few minutes and retry once, then
   report it. Fix your own failures in follow-up commits. Never amend.
7. If the bead cannot be finished as written (an API that does not exist, a decision the docs do
   not make, a missing dependency), do not invent. Commit what is sound, run
   `br update <id> --status open --no-auto-flush --lock-timeout 180000`, and report the exact
   question.
8. Done: `br close <id> --reason "<what changed, commit sha>" --no-auto-flush --lock-timeout 180000`.

Test in the real workspace. Do not copy the workspace to a scratch folder or add a worktree for
testing: each copy builds its own target directory (over 100 MB) and the safety hook blocks
deleting it afterwards. If a file is not wired in yet (for example a module whose `mod` line comes
from a later bead), test it with a small `#[cfg(test)]` include in your own file or say in the
report that the test runs with the later bead.

Never test against the live desktop in a way that changes it (no loading KWin scripts, no ending
real processes, no writing system files). UI tests use GPUI `TestAppContext` unless the bead says
otherwise.

Final message, short: bead ID, commit SHAs, files, test and clippy results, anything unsure or not
done.
