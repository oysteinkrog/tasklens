# B1 infra beads: plan keys and bead IDs

Batch B1 covers epics E01, E02, E03, E31, E32 and E33 (`docs/plan/00-epics.md` section 5). The
leader created the epic and anchor beads (`docs/plan/anchors.md`); B1 replaced each anchor's
placeholder with the full template and created every other bead under its epic. Cross-epic
dependencies point at anchor beads only.

Bead titles start with the plan key. Open items list values the docs do not decide. Each bead
that uses one carries the recommendation in its text and says it is pending the leader.

## E02 Workspace, toolchain, lints, check-deps, CI

Epic bead: `tasklens-v9m`. 12 beads, all P0 (priority 0), labels `phase-p0`, `area-workspace-ci` plus a `crate-*` label. No cross-epic dependencies (E02's row in 00-epics section 4 is "none").

| Plan key | Bead ID | Title | P | Depends on (plan keys or anchor IDs) |
|---|---|---|---|---|
| E02-001 | tasklens-v9m.3 | Toolchain pin, cargo alias, clippy config | 0 | none |
| E02-002 | tasklens-v9m.4 | Root Cargo.toml with pins, profiles and lints | 0 | E02-001 |
| E02-003 | tasklens-v9m.5 | Skeletons for tl-model, tl-collect and tl-ops | 0 | E02-002 |
| E02-004 | tasklens-v9m.6 | Skeletons for tl-engine and tl-gpu-probe | 0 | E02-003 |
| E02-005 | tasklens-v9m.7 | Skeletons for tl-ui and tl-app | 0 | E02-004 |
| E02-006 | tasklens-v9m.8 | xtask package with subcommand dispatch | 0 | E02-002 |
| E02-A1 | tasklens-v9m.1 | Workspace compiles (Cargo.lock, `build --workspace --locked`, ADR-002 verification, first U-1 reading) | 0 | E02-001, E02-003, E02-004, E02-005, E02-006 |
| E02-007 | tasklens-v9m.9 | check-deps: banned crates and asupersync greps (checks 1, 3, 4) | 0 | E02-A1 |
| E02-008 | tasklens-v9m.10 | check-deps: allowed rows and closure rules (check 2) | 0 | E02-007 |
| E02-009 | tasklens-v9m.11 | scripts/ci.sh running the ADR-013 CI rows | 0 | E02-008 |
| E02-010 | tasklens-v9m.12 | GitHub Actions workflow with lavapipe | 0 | E02-009 |
| E02-A2 | tasklens-v9m.2 | check-deps and CI (failing-branch run recorded, U-1 settled) | 0 | E02-008, E02-009, E02-010 |

Structural choices made by this batch (no doc gives them; each is the smallest option):
- Skeletons carry the internal workspace edges plus `gpui-kit` in tl-ui and `asupersync` in tl-engine, so U-1 and the ADR-002 verification commands can run in E02. Owners add every other external crate when they first use it.
- `[workspace.package]` holds `edition = "2024"`; crates use `edition.workspace = true`.
- `.gitignore` with `/target`; `clippy.toml` with `allow-unwrap-in-tests = true` (implements ADR-002 "unwrap_used ... allowed in tests").

### Open items

1. **Package name of the `tasklens` binary.** ADR-001 calls the crate `tl-app` (bin "tasklens"), but ADR-002's verification writes `cargo tree -p tasklens`, which needs the package to be named `tasklens`. Recommendation: package `tl-app` with `[[bin]] name = "tasklens"` (matches the crate directory, the ADR-001 table and the `crate-tl-app` label) and change ADR-002's command to `-p tl-app`. Beads E02-005, E02-A1 and E02-007 are written this way and say how to switch.
2. **`bench` feature lines conflict with the E01 chain edit.** 00-epics section 2 says E01 adds the `bench` feature to `tl-ui/Cargo.toml` by chain edit, but ADR-002 fixes the exact text, and E33-A1 (P0, `args.rs` declares `--bench` behind `bench`) needs `tl-app`'s `bench = ["tl-ui/bench"]`, which cannot compile without tl-ui's `bench`. E01 depends on E33-A1, so waiting for E01 would block E33-A1. Recommendation: E02-005 writes both `bench` lines up front (done); drop or empty the E01 chain-edit bead for the tl-ui `bench` feature.
3. **Two banned asupersync APIs cannot be found by a name grep.** ADR-004 bans `JoinHandle` from `RuntimeHandle::spawn` and `Budget::with_poll_quota` on long-lived tasks only. Recommendation (written into E02-007): match the literals `RuntimeHandle::spawn(` and `.with_poll_quota(`, accepted only with a same-line `// check-deps: allow <reason>` comment.
4. **gpui-kit may put `zbus` in tl-ui's closure.** GPUI's Linux platform uses the XDG portals, which usually go through `ashpd` and `zbus`. ADR-001 check 2 fails if `zbus` is in tl-ui's normal closure. Not verified. E02-A1 records the answer from `cargo tree -p tl-ui`. Recommendation if it appears: narrow the rule to "tl-ui has no direct dependency on zbus or arc-swap and no path to them that does not go through gpui-kit", recorded as an ADR-001 change.
5. **CI system packages on Ubuntu.** ADR-002 lists Arch packages only. Recommendation (written into E02-010): `clang pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libfontconfig1-dev libvulkan1 mesa-vulkan-drivers`, lavapipe ICD at `/usr/share/vulkan/icd.d/lvp_icd.x86_64.json`.
6. **Workspace `version` and `publish`.** Not set by any doc; `--version` (E33) prints it. Recommendation: `version = "0.1.0"`, `publish = false` in `[workspace.package]` (written into E02-002 as pending).
7. **`rustfmt.toml`.** 00-epics lists it as an E02 file, but no doc gives options. Recommendation: do not create it; default rustfmt style (E02-001 is written that way).

