# tl-collect test fixtures

Each directory here is one fixture. A fixture is a fake root: it holds `proc/` and `sys/` (and
`run/udev/data/` when captured), laid out like the real paths. Parsers read it through `SysRoot`
instead of `/`.

- `owner-desktop/`: captured on the owner's machine with
  `cargo xtask capture-fixture owner-desktop`. `CAPTURE.txt` lists what was copied and what was
  skipped. Never edit it by hand. To change it, re-capture with
  `cargo xtask capture-fixture owner-desktop --force` and check it for leaks again (user names,
  hostname, MAC addresses, machine-id, boot ID, disk serials and WWNs, e-mail addresses).
- Multi-step fixtures (for counters and deltas) use `step0/`, `step1/`, and so on. Each step has
  its own `proc/` and `sys/`.
- Hand-made edge fixtures are created by the bead that tests them.

The capture scrubs personal data: user names become `user1`, `user2`, ...; the hostname becomes
`REDACTED`; serial files become `REDACTED`; MAC addresses become `00:00:00:00:00:00`; the boot ID
becomes `00000000-0000-4000-8000-000000000001`; each `cmdline` becomes `<comm>\0--arg\0`. The
`environ`, `maps`, `smaps`, `smaps_rollup`, `mem`, `fd` and `fdinfo` entries are never copied.

`../fixture_layout.rs` checks the layout of `owner-desktop` and keeps an `insta` snapshot of its
file list in `../snapshots/`.
