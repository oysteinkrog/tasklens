# Resume Plan After the 2026-10-07 Reboots

This file is for the leader session. It lists the beads that were in progress when the
agents stopped, and the order to restart them.

## Step 1: build check of HEAD

Run one combined job before any agent starts:

```sh
heavy-build sh -c 'cargo check -p tl-ui --lib && cargo check -p tl-ui --lib --features test-hooks && cargo check -p tl-app --all-targets'
```

gma.15's commits abd5beb and 14be5fe were never compiled. Fix any break first.

## Step 2: restart beads, two agents at a time

All nine beads below are IN_PROGRESS. Each fresh agent reads `docs/research/brief-implement.md`,
then the bead's latest comment, then `git diff` on the files listed, and finishes from there.
Each agent keeps at most one combined heavy-build job queued.

| Order | Bead | Files with uncommitted work | Then also |
|---|---|---|---|
| 1 | gma.15 | tests/ui_props.rs | gma.41, gma.40 |
| 2 | m1f.6 | crates/tl-app/tests/lowmem_unit.rs | m1f.21 after it closes |
| 3 | 9o0.34 | tests/ui_information.rs, tests/ui_components.rs | 9o0.23 |
| 4 | pdu.51 | pages/processes/mod.rs, tests/ui_process_menu.rs | |
| 5 | bj6.10 | window/menus_file.rs, tests/ui_menus.rs | |
| 6 | fut.11 | components/gallery.rs, six untracked tests/golden/gallery_{dialog,menu,shield}_1_{0,1}.png | |
| 7 | 8fx.24 | none (last commit 77cc6f16) | 8fx.39 |
| 8 | m1f.2 | none (last commit 3327378f) | m1f.20 |
| 9 | bj6.17 | none (last commit 5ff1c9ff); the tracker skips the success toast when the text is empty | |

Paths are under `crates/tl-ui/` unless they start with `crates/`.

## Step 3: after the queue is empty

Run the full tl-ui clippy with and without test-hooks, plus the workspace tests, as one job.
Then continue with `pick.py` as before.
