# tasklens

tasklens is a task manager for Linux, written in Rust with the GPUI UI framework. It shows
processes, live performance graphs for each device, services, startup apps, network
connections and system information.

![The Performance tab on the Overview page](docs/img/overview.png)

It is a personal project, built first for one desktop: CachyOS, KDE Plasma 6 on Wayland, an
NVIDIA GPU and three monitors. It may work elsewhere, but that is the only setup it is tested
on. UI and rendering speed come first in every design choice.

## What it does

The main window has six tabs.

| Tab | What it shows |
|---|---|
| Processes | A process tree grouped by app, with columns you can pick, a filter box and actions: end, send a signal, set priority and CPU affinity, open Properties (threads, file descriptors, memory maps, security, environment, strings). A History mode shows the last 10 minutes even when recording is off. |
| Performance | A device sidebar with live mini graphs, an Overview page, and a page for each CPU, memory, disk, network adapter, GPU, fan and battery |
| Services | systemd system and user units, with a Properties window that includes the unit's journal |
| Startup Apps | XDG autostart entries and systemd user units, which you can enable and disable |
| Connections | TCP and UDP sockets, IPv4 and IPv6, with the owning process |
| Information | CPU, board, OS and GPU details |

The Overview page puts CPU, memory, GPU, disk and network charts on one screen. You can
rearrange its boxes, and each box has its own layout choices (for example, per-core rows or a
power draw strip in the CPU box).

Other parts:

- Detached graph windows and a small always-on-top performance window.
- Tray icons that draw a live graph.
- A low-memory watcher (`tasklens-watch`) that runs as a systemd user service and opens a
  rescue window when memory runs low.
- A file handle search across processes.
- One instance at a time: starting tasklens again brings up the open window.
- Command-line options, for example `--tab services` or `--select-pid 1234`. Run
  `tasklens --help` for the list.

## Requirements

- Linux on x86_64. It reads `/proc`, `/sys` (cgroup v2, hwmon, cpufreq, block and net
  devices) and netlink `sock_diag` directly.
- A Vulkan driver. GPUI draws through wgpu.
- Native Wayland is the supported path. X11 is best effort.
- D-Bus for some features: systemd (Services), udisks2 (disk details and SMART), UPower
  (batteries), NetworkManager (network details) and KWin (window list). Without one of these
  services, the views that use it lose that data.
- polkit and `pkexec` for actions on other users' processes and for the setup steps below.
- NVIDIA GPUs: `nvidia-utils` for NVML. NVML is loaded only in a separate `tl-gpu-probe`
  process, so a driver fault cannot crash the app.
- AMD and Intel GPUs: read from sysfs and DRM fdinfo. This path is not tested on real hardware.
- The package owner shown in process Properties comes from `pacman`, so it works on Arch-based
  systems only.

Some sensors need one-time root setup, which the Additional Setup window runs through `pkexec`:
a udev rule that makes Intel RAPL energy counters readable (CPU power draw), and
`sensors-detect` for board fan and temperature chips.

## Install

There is no binary release and no crates.io package. Build it from source.

### Arch Linux and Arch-based distributions (makepkg)

`packaging/PKGBUILD` builds a package from the local checkout. It installs the binaries, the
desktop file, the polkit policy and the icon.

```sh
git clone https://github.com/oysteinkrog/tasklens
cd tasklens/packaging
makepkg -si
```

It needs `base-devel`, `clang`, `pkgconf` and `rustup`. rustup picks Rust 1.98.1 from
`rust-toolchain.toml`. The `check()` step runs the whole test suite; add `--nocheck` to skip it.

### Any distribution (cargo)

```sh
cargo build --release --locked --workspace
./target/release/tasklens
```

This build runs from `target/release/` and finds `tl-gpu-probe` next to it. The privileged
helper and the setup script are looked up only under `/usr/lib/tasklens/`, and they need the
polkit policy, so actions on other users' processes and the Additional Setup steps work only
from the installed package.

## Files

| Data | Path |
|---|---|
| Settings | `~/.config/tasklens/settings.toml` |
| User themes | `~/.config/tasklens/themes.json` |
| Window and table state | `~/.local/state/tasklens/state.toml` |
| Logs and crash reports | `~/.local/state/tasklens/logs/`, `~/.local/state/tasklens/crash/` |

The paths follow `XDG_CONFIG_HOME` and `XDG_STATE_HOME` when they are set.

## How it is built

The engine and the UI are separate crates. The engine collects data on its own runtime
(asupersync) and the UI reads the latest snapshot through a trait, so it never blocks on a
system call.

| Crate | Role |
|---|---|
| `tl-model` | Data types shared by every other crate |
| `tl-collect` | Parsers for `/proc`, `/sys` and netlink |
| `tl-ops` | Process operations: signals, priority, affinity, I/O priority |
| `tl-engine` | Workers, sampling schedule, D-Bus adapters, actions |
| `tl-history` | Storage format for metric history |
| `tl-gpu-probe` | The only process that loads NVML |
| `tl-ui` | GPUI views, charts, the process table, themes |
| `tl-app` | The `tasklens` binary: startup, settings, logging |
| `tl-helper` | Root helper started through `pkexec` |
| `tl-tray` | Tray icons and their graphs |
| `tl-watch` | The `tasklens-watch` low-memory watcher |

`docs/ARCHITECTURE.md` has the design records (ADR-001 to ADR-017). `docs/DECISIONS.md` has
the owner decisions, `docs/ui/` the UI specs and `docs/spec/` the feature specs.

## Development

Work is tracked as beads in `.beads/`, managed with the `br` tool. Each bead names its spec in
`docs/`. `AGENTS.md` has the rules for commits, builds and tests.

```sh
cargo test --workspace
cargo xtask check-deps    # fails if Tokio or a crate outside the allowed list gets in
```

## Limitations

- Tested on one machine only (KDE Plasma 6, Wayland, NVIDIA).
- x86_64 only.
- Window position is not saved, because Wayland does not allow it. Size and maximized state are.
- The Processes power column is an estimate. Linux has no per-process power source, so the
  column is off by default.
- Per-process network rates count TCP only. UDP traffic is not attributed to processes.

## About Contributions

> *About Contributions:* Please don't take this the wrong way, but I do not accept outside contributions for any of my projects. I simply don't have the mental bandwidth to review anything, and it's my name on the thing, so I'm responsible for any problems it causes; thus, the risk-reward is highly asymmetric from my perspective. I'd also have to worry about other "stakeholders," which seems unwise for tools I mostly make for myself for free. Feel free to submit issues, and even PRs if you want to illustrate a proposed fix, but know I won't merge them directly. Instead, I'll have Claude or Codex review submissions via `gh` and independently decide whether and how to address them. Bug reports in particular are welcome. Sorry if this offends, but I want to avoid wasted time and hurt feelings. I understand this isn't in sync with the prevailing open-source ethos that seeks community contributions, but it's the only way I can move at this velocity and keep my sanity.

## License

MIT. See `LICENSE`.
