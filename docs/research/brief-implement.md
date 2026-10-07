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
   A pathspec commit takes each named file whole from disk and ignores what you staged. Before
   committing, run `git diff -- <files>`. If any named file holds another agent's uncommitted
   hunks, do not use a pathspec for it: write a patch of only your hunks, stage it with
   `git apply --cached <patch>`, check that `git diff --cached` shows only your changes, then run
   `git commit -m "..."` with no pathspec right after that check. The index is shared: never
   leave anything staged between commits (that includes `git mv` and `git rm`, which stage at
   once; prefer a plain move and stage it in the same command as the commit). If
   `git diff --cached` shows anything that is not yours, do not commit without a pathspec; wait
   and tell the leader.
6. Run the bead's tests, `cargo fmt --check` and `cargo clippy -p <crate> --all-targets -- -D
   warnings`, all through `heavy-build`. Builds queue on one machine-wide lock, so start them in
   the background and wait for the result; always read the output before you report. Completion
   notices for background commands are sometimes lost, so do not go idle waiting for one: write
   the build output to a file and wait with a foreground loop such as
   `until grep -qE '^(EXIT|exit) ' out.txt; do sleep 10; done` (append `; echo "EXIT $?"` to the
   build command), then read the file. The safety hook blocks `>` redirects into `/var/tmp/claude`, so
   write the file with `2>&1 | tee /var/tmp/claude/<bead-id>-build.txt` instead. If another
   agent's half-finished work breaks the workspace build, wait a few minutes and retry once, then
   report it. Fix your own failures in follow-up commits. Never amend.
   To format, run `rustfmt --edition 2024 <your files>` only. Never run `cargo fmt` without
   `--check`: it rewrites other agents' files while they are working on them.
   Name every build output or scratch file after your bead (for example
   `/var/tmp/claude/<bead-id>-clippy.txt`). Generic names such as `out.txt` get overwritten by
   other agents.
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

Never run `cargo xtask capture-fixture` unless your bead is the one that captures a fixture. Use
the committed fixtures under `crates/tl-collect/tests/fixtures/`.

Never test against the live desktop in a way that changes it (no loading KWin scripts, no ending
real processes, no writing system files). UI tests use GPUI `TestAppContext` unless the bead says
otherwise.

Final message, short: bead ID, commit SHAs, files, test and clippy results, anything unsure or not
done.


Never stop a process by name or pattern (`pkill -f`, `killall`). Stop only processes you started, by the PID you recorded when you started them. Other agents' builds can have the same command line.
