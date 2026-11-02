#!/usr/bin/env python3
"""Move tl-ui modules into a new crate (ADR-020, "Move method", steps 1 to 4).

Usage: scripts/split/extract.py CRATE --lower CRATE [--lower CRATE ...]
                                [--from CRATE] [--dry-run] PATH [PATH ...]

CRATE is the new crate, for example tl-ui-base. Each --lower names a crate the
new crate may depend on (its ADR-020 row). --from is the crate the files leave,
tl-ui by default. Each PATH is relative to the source crate's src/: a file
(`i18n.rs`) or a directory module (`theme/`, which also takes `theme.rs` when
that file exists). Run it from the workspace root.

The script does steps 1 to 4 of the move method:

1. Writes crates/CRATE/Cargo.toml. Its dependencies are the source crate's
   [dependencies] lines whose crate the moved files name, plus the --lower
   crates. Its dev-dependencies are the source crate's [dev-dependencies] lines
   whose crate the moved files name, plus the gpui-kit test-support line when
   gpui-kit is a dependency. A feature the moved files test with
   cfg(feature = "X") becomes `X = []`, and the source crate's feature X gains
   "CRATE/X". Adds CRATE to [workspace] members (before the source crate) and to
   [workspace.dependencies], and adds it to the source crate's [dependencies].
2. `git mv`s the paths into crates/CRATE/src/, keeping their paths under src/.
3. Writes crates/CRATE/src/lib.rs (and a mod.rs for each parent directory of a
   nested path): `pub mod` lines for the moved modules, then root imports of
   the lower crates' modules that the moved code names as `crate::x`.
4. In the source crate, replaces each moved `pub mod x;` with
   `pub use tl_ui_x::x;` in the file that declared it, keeping its attributes.

It then prints the follow-up list: the ADR-001 and check-deps rows, the
check-layers source root, and the moved files to read for cross-crate
visibility, test helpers, actions!, macros and relative file paths.

--dry-run prints what it would do and changes nothing.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

MOD_DECL = re.compile(
    r"^(\s*)(pub(?:\([a-z: ]+\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;\s*$"
)
CRATE_PATH = re.compile(r"\bcrate::([a-z_][a-z0-9_]*(?:::[a-z_][a-z0-9_]*)*)")
FEATURE = re.compile(r'feature\s*=\s*"([A-Za-z0-9_-]+)"')
DEP_LINE = re.compile(r"^([A-Za-z0-9_-]+)\s*=")

CHECKS = [
    ("pub(crate) or pub(super) items", re.compile(r"\bpub\((crate|super)\)")),
    ("#[cfg(test)] items", re.compile(r"#\[cfg\(test\)\]")),
    (
        "actions! (needs an init or bind_keys call from app init)",
        re.compile(r"\bactions!"),
    ),
    ("macro_rules!", re.compile(r"\bmacro_rules!")),
    (
        "relative file paths (include_str!, include_bytes!, CARGO_MANIFEST_DIR)",
        re.compile(r"include_str!|include_bytes!|CARGO_MANIFEST_DIR"),
    ),
]


def ident(crate):
    return crate.replace("-", "_")


def sections(text):
    """Map each `[section]` of a Cargo.toml to its (start, end) line indexes."""
    lines = text.split("\n")
    found = {}
    name, start = None, 0
    for i, line in enumerate(lines):
        m = re.match(r"^\[([^\]]+)\]\s*$", line)
        if m:
            if name is not None:
                found[name] = (start, i)
            name, start = m.group(1), i + 1
    if name is not None:
        found[name] = (start, len(lines))
    return lines, found


def dep_lines(text, section):
    """The `name = ...` lines of one Cargo.toml section, by crate name."""
    lines, found = sections(text)
    if section not in found:
        return {}
    start, end = found[section]
    out = {}
    for line in lines[start:end]:
        m = DEP_LINE.match(line)
        if m:
            out[m.group(1)] = line
    return out


def feature_lines(text):
    lines, found = sections(text)
    if "features" not in found:
        return {}
    start, end = found["features"]
    out = {}
    for line in lines[start:end]:
        m = DEP_LINE.match(line)
        if m:
            out[m.group(1)] = line
    return out


def moved_files(src, paths):
    """(source path, path under src/) for each file the PATH list moves, and the git mv pairs."""
    files, moves = [], []
    for p in paths:
        p = p.rstrip("/")
        target = src / p
        if target.is_dir():
            moves.append(p)
            files += [f for f in sorted(target.rglob("*.rs"))]
            flat = src / f"{p}.rs"
            if flat.exists():
                moves.append(f"{p}.rs")
                files.append(flat)
        elif target.is_file():
            moves.append(p)
            files.append(target)
        else:
            sys.exit(f"extract: {target} does not exist")
    return files, moves


def module_of(path):
    """`theme/` -> ['theme'], `pages/minigraph/` -> ['pages', 'minigraph'], `i18n.rs` -> ['i18n']."""
    p = path.rstrip("/")
    if p.endswith(".rs"):
        p = p[:-3]
    return p.split("/")


def lower_modules(root, crate):
    """Module paths a lower crate declares in lib.rs and in the mod.rs files under src/."""
    src = root / "crates" / crate / "src"
    out = set()

    def walk(file, prefix):
        if not file.exists():
            return
        for line in file.read_text().split("\n"):
            m = MOD_DECL.match(line)
            if not m or not m.group(2):
                continue
            name = m.group(3)
            out.add(tuple(prefix + [name]))
            sub = src.joinpath(*prefix, name, "mod.rs")
            walk(sub, prefix + [name])

    walk(src / "lib.rs", [])
    return out


def declaring_file(src, module):
    """The file in the source crate that declares `module` (a list of names)."""
    if len(module) == 1:
        return src / "lib.rs"
    parent = src.joinpath(*module[:-1])
    for candidate in (parent / "mod.rs", parent.with_suffix(".rs")):
        if candidate.exists():
            return candidate
    sys.exit(f"extract: no mod.rs for {'::'.join(module[:-1])} under {src}")


def find_decl(lines, name):
    """Index of the `mod name;` line and the index where its attributes start."""
    for i, line in enumerate(lines):
        m = MOD_DECL.match(line)
        if m and m.group(3) == name:
            start = i
            while start > 0 and lines[start - 1].strip().startswith("#["):
                start -= 1
            return start, i, m
    return None


def run(cmd, dry):
    print("  $ " + " ".join(cmd))
    if not dry:
        subprocess.run(cmd, check=True)


def write(path, text, dry):
    print(f"  write {path}")
    if not dry:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("crate")
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--lower", action="append", default=[])
    ap.add_argument("--from", dest="source", default="tl-ui")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    root = Path.cwd()
    if not (root / "Cargo.toml").exists() or not (root / "crates").is_dir():
        sys.exit("extract: run it from the workspace root")
    dry = args.dry_run
    crate, source = args.crate, args.source
    src = root / "crates" / source / "src"
    dst_dir = root / "crates" / crate
    if dst_dir.exists():
        sys.exit(f"extract: {dst_dir} exists already")

    files, moves = moved_files(src, args.paths)
    texts = {f: f.read_text() for f in files}
    joined = "\n".join(texts.values())
    modules = sorted({tuple(module_of(p)) for p in args.paths})

    # Step 1: Cargo.toml files.
    print("Step 1: manifests")
    src_toml_path = root / "crates" / source / "Cargo.toml"
    src_toml = src_toml_path.read_text()
    deps = dep_lines(src_toml, "dependencies")
    dev = dep_lines(src_toml, "dev-dependencies")
    used = lambda name: re.search(rf"\b{ident(name)}\b", joined) is not None  # noqa: E731
    new_deps = [deps[n] for n in deps if used(n) and n not in args.lower]
    new_deps += [f"{c} = {{ workspace = true }}" for c in args.lower]
    dep_names = {n for n in deps if used(n)} | set(args.lower)
    new_dev = [
        dev[n] for n in dev if n != "gpui-kit" and used(n) and n not in dep_names
    ]
    if "gpui-kit" in dep_names and "gpui-kit" in dev:
        new_dev.append(dev["gpui-kit"])
    src_features = feature_lines(src_toml)
    features = sorted({f for f in FEATURE.findall(joined) if f in src_features})

    manifest = [
        "[package]",
        f'name = "{crate}"',
        "edition.workspace = true",
        "version.workspace = true",
        "publish.workspace = true",
        "",
        "[dependencies]",
        *new_deps,
    ]
    if new_dev:
        manifest += ["", "[dev-dependencies]", *new_dev]
    if features:
        manifest += ["", "[features]", "default = []", *[f"{f} = []" for f in features]]
    manifest += ["", "[lints]", "workspace = true", ""]
    write(dst_dir / "Cargo.toml", "\n".join(manifest), dry)

    ws_path = root / "Cargo.toml"
    ws = ws_path.read_text()
    member = f'  "crates/{source}",'
    if member not in ws:
        sys.exit(f"extract: {member.strip()} not in [workspace] members")
    ws = ws.replace(member, f'  "crates/{crate}",\n{member}', 1)
    ws_dep = re.search(rf"^{re.escape(source)}\s*=.*$", ws, re.M)
    if not ws_dep:
        sys.exit(f"extract: {source} not in [workspace.dependencies]")
    ws = (
        ws[: ws_dep.end()]
        + f'\n{crate} = {{ path = "crates/{crate}" }}'
        + ws[ws_dep.end() :]
    )
    write(ws_path, ws, dry)

    lines, found = sections(src_toml)
    start, end = found["dependencies"]
    last = start
    for i in range(start, end):
        m = DEP_LINE.match(lines[i])
        if m and (m.group(1) == "tl-model" or m.group(1).startswith("tl-ui")):
            last = i
    lines.insert(last + 1, f"{crate} = {{ workspace = true }}")
    for f in features:
        lines2, found2 = sections("\n".join(lines))
        fs, fe = found2["features"]
        for i in range(fs, fe):
            m = DEP_LINE.match(lines[i])
            if m and m.group(1) == f:
                lines[i] = re.sub(
                    r"\[(.*?)\]",
                    lambda mm: (
                        "["
                        + ", ".join(
                            x for x in [mm.group(1).strip(), f'"{crate}/{f}"'] if x
                        )
                        + "]"
                    ),
                    lines[i],
                    count=1,
                )
    write(src_toml_path, "\n".join(lines), dry)

    # Step 2: git mv.
    print("Step 2: git mv")
    for p in moves:
        to = dst_dir / "src" / p
        if not dry:
            to.parent.mkdir(parents=True, exist_ok=True)
        run(["git", "mv", str(src / p), str(to)], dry)

    # Step 3: the new crate's lib.rs and parent mod.rs files.
    print("Step 3: lib.rs")
    lower_mods = {c: lower_modules(root, c) for c in args.lower}
    named = {tuple(m.group(1).split("::")) for m in CRATE_PATH.finditer(joined)}
    own = set()
    for m in modules:
        for k in range(1, len(m) + 1):
            own.add(m[:k])
    imports = {}  # parent module path -> list of (lower crate, module path)
    for path in sorted(named):
        for k in range(len(path), 0, -1):
            prefix = path[:k]
            if prefix in own:
                continue
            hit = next((c for c, mods in lower_mods.items() if prefix in mods), None)
            if hit and prefix[:-1] in own | {()}:
                imports.setdefault(prefix[:-1], set()).add((hit, prefix))
                break
    decls = {}  # parent module path -> list of declaration lines
    for m in modules:
        # The parent modules of a nested path, for example `pub mod pages;` in lib.rs.
        for k in range(1, len(m)):
            line = f"pub mod {m[k - 1]};"
            if line not in decls.setdefault(m[: k - 1], []):
                decls[m[: k - 1]].append(line)
        decl_file = declaring_file(src, list(m))
        dlines = decl_file.read_text().split("\n")
        found_decl = find_decl(dlines, m[-1])
        attrs = []
        if found_decl:
            attrs = [line.strip() for line in dlines[found_decl[0] : found_decl[1]]]
        decls.setdefault(m[:-1], []).extend(attrs + [f"pub mod {m[-1]};"])
    for parent in sorted(set(decls) | set(imports)):
        body = []
        if parent == ():
            body.append(f"//! {crate}: modules moved from {source} (ADR-020).")
            body.append("")
        body += decls.get(parent, [])
        uses = sorted(imports.get(parent, ()))
        if uses:
            body.append("")
            vis = "" if parent == () else "pub(crate) "
            for lower, path in uses:
                body.append(f"{vis}use {ident(lower)}::{'::'.join(path)};")
        target = (
            dst_dir / "src" / ("lib.rs" if parent == () else Path(*parent) / "mod.rs")
        )
        write(target, "\n".join(body) + "\n", dry)

    # Step 4: the source crate re-exports the moved modules.
    print("Step 4: re-exports in " + source)
    edits = {}
    for m in modules:
        decl_file = declaring_file(src, list(m))
        text = edits.get(decl_file) or decl_file.read_text()
        dlines = text.split("\n")
        found_decl = find_decl(dlines, m[-1])
        if not found_decl:
            print(
                f"  NOTE: no `mod {m[-1]};` in {decl_file}; add the re-export by hand"
            )
            continue
        _, i, mm = found_decl
        vis = (mm.group(2) or "").strip()
        vis = f"{vis} " if vis else ""
        dlines[i] = f"{mm.group(1)}{vis}use {ident(crate)}::{'::'.join(m)};"
        edits[decl_file] = "\n".join(dlines)
    for path, text in edits.items():
        write(path, text, dry)

    # Follow-up list.
    externals = sorted(n for n in dep_names if n not in args.lower)
    print("\nFollow-up:")
    print(f"- ADR-001 row and xtask/src/check_deps.rs ALLOWED row for {crate}:")
    print(f"    {', '.join(externals)}")
    print(
        f"  ({', '.join(args.lower)} come from check_layers::EDGES, not from the row.)"
    )
    print(f'- xtask/src/check_layers.rs UI_SRCS: ("{crate}", "crates/{crate}/src")')
    if features:
        print(
            f"- features moved: {', '.join(features)}; check the source crate's [features]:"
        )
        for f in features:
            print(f"    {src_features[f].strip()}")
    for title, pattern in CHECKS:
        hits = [
            (f, len(pattern.findall(t))) for f, t in texts.items() if pattern.search(t)
        ]
        if hits:
            print(f"- {title}:")
            for f, n in hits:
                print(f"    {f.relative_to(src)} ({n})")
    print(
        f"- build: heavy-build cargo lclippy -p {crate} -p {source} -- -D warnings, then fix "
        "what crosses the new boundary (ADR-020 step 4)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
