# Shared brief: tasklens merged feature spec (round 2)

tasklens is a native Linux task manager (Rust, GPUI + gpui-component UI, asupersync engine, no Tokio)
that must replicate ALL functionality of TaskSlinger 0.9.2 (Windows) and Mission Center 1.2.0 (Linux),
on the owner's machine: CachyOS/Arch, KDE Plasma 6 Wayland, NVIDIA RTX 5070 Ti (driver 615; DRM fdinfo
is empty, so per-process GPU needs NVML), AMD CPU with 32 logical CPUs (amd-pstate-epp, k10temp), zram,
three monitors at 110% scale, 144 Hz. The owner said: "Map out ALL features/functionality in
taskslinger and mission center and create plan to replicate it all." Implementation agents must not
make significant decisions later, so every feature entry must be decided now.

Read first (fully, for your area):
- /home/oystein/work/tasklens/docs/PLAN.md (draft plan; crates tl-model, tl-collect, tl-engine, tl-ui, tl-helper)
- /home/oystein/work/tasklens/docs/DECISIONS.md
- /home/oystein/work/tasklens/docs/research/claude-taskslinger-inventory.md
- /home/oystein/work/tasklens/docs/research/claude-missioncenter-inventory.md
- /home/oystein/work/tasklens/docs/research/astra-missioncenter-inventory.md (large; grep your area)
- /home/oystein/work/tasklens/docs/research/astra-architecture-review.md. Its recommendations are
  binding unless the owner overrides them: in-process engine; NVML only in a tl-gpu-probe child
  process; per-source sample rates; identity-based selection and action targets; validity states
  never shown as zero; pidfd for signals.
- /home/oystein/work/tasklens/docs/research/claude-gpui-apis.md and claude-gpui-component-widgets.md
  (UI feasibility)

Verdict rules:
- Default is BUILD (same behaviour) or ADAPT (Linux equivalent; say exactly what changes).
- DROP only when the feature has no Linux meaning (licence and purchase, in-app updater, installer,
  Windows-only internals with no analogue). State the reason. If an analogue exists, it is ADAPT.
- When TaskSlinger and Mission Center differ, pick one behaviour (or a merge) and say why.
  TaskSlinger's look, density and layout win by default (the owner wants to "more or less copy" it).
  Mission Center wins on Linux data correctness and Linux-only features.
- Assign a phase. P1 core: process table, performance overview, chrome, settings basics.
  P2 depth: properties windows, services, startup, connections, information, actions.
  P3 extras: history recording, detached and tray graphs, theme editor, handle search, low-memory
  mode, strings scan, privileged helper features. P4 polish and rare items.
  Everything gets a phase; nothing is cut.
- Privilege: say whether it needs root, capabilities or polkit, the exact mechanism (pkexec, polkit
  action, setcap, udev rule), and what the unprivileged fallback shows.

Output: Markdown, returned as text in your FINAL message. Do not write files; the leader saves it.
One entry per feature, in this exact shape:

    ### <ID> <Short name>
    - Source: TS <inventory section or line refs> | MC <file:line or inventory section> | both
    - Behaviour: what the user sees and does (precise labels, defaults, units, states)
    - Linux data/mechanism: exact files, sysfs nodes, syscalls, D-Bus APIs, NVML calls
    - Verdict: BUILD | ADAPT (what changes) | DROP (why)
    - Phase: P1-P4
    - Privilege: none | details + fallback
    - Open question: only if truly undecidable from evidence; give your recommended answer anyway

Use your assigned ID prefix and number from 001. Group entries under headings by sub-area.
End with a table of every entry (ID, name, verdict, phase), then a list of any conflicts with the
architecture review.
