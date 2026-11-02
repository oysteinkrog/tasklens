#!/usr/bin/env python3
"""Move tl-ui test-hook gates onto debug_assertions (ADR-020, "Test hooks").

Usage: scripts/split/hooks_gate.py [--check] PATH [PATH ...]

Each PATH is a .rs file or a directory (searched for .rs files). The script
rewrites these three predicates, wherever they appear inside cfg(...),
cfg_attr(...) or cfg!(...):

    feature = "test-hooks"
        -> any(debug_assertions, feature = "test-hooks")
    any(test, feature = "test-hooks")
        -> any(test, debug_assertions, feature = "test-hooks")
    not(feature = "test-hooks")
        -> not(any(debug_assertions, feature = "test-hooks"))

Lines already in the rewritten form are left as they are, so a second run
changes nothing. Any other code line that names test-hooks (for example
all(feature = "test-hooks", ...) or not(any(test, feature = "test-hooks")))
is printed and the script exits 1 without writing any file. Comment lines are
never changed; the ones that name test-hooks are listed so a person can
reword them.

--check reports what would change and writes nothing.
"""

import re
import sys
from pathlib import Path

FEAT = 'feature = "test-hooks"'
NEW_PLAIN = f"any(debug_assertions, {FEAT})"
NEW_ANY_TEST = f"any(test, debug_assertions, {FEAT})"
NEW_NOT = f"not({NEW_PLAIN})"

# Where a predicate may start: right after the opening paren of a cfg form.
OPENERS = r"(?P<open>\bcfg\(|\bcfg!\(|\bcfg_attr\()"

# Already converted forms. They are masked before the rewrite and checked
# against the accepted list afterwards.
DONE = [NEW_NOT, NEW_ANY_TEST, NEW_PLAIN]

REWRITES = [
    # (pattern after an opener, replacement)
    (re.escape(f"any(test, {FEAT})"), NEW_ANY_TEST),
    (re.escape(f"not({FEAT})"), NEW_NOT),
    (re.escape(FEAT), NEW_PLAIN),
]


def rs_files(arg):
    p = Path(arg)
    if p.is_dir():
        return sorted(q for q in p.rglob("*.rs") if q.is_file())
    if p.is_file():
        return [p]
    raise SystemExit(f"hooks_gate: no such file or directory: {arg}")


def is_comment(line):
    s = line.lstrip()
    return s.startswith("//") or s.startswith("/*") or s.startswith("*")


def convert_line(line):
    """Return (new_line, error). error is None when the line is fine."""
    if "test-hooks" not in line:
        return line, None
    if is_comment(line):
        return line, None

    out = line
    for pat, new in REWRITES:
        # Only rewrite a predicate that is the first argument of a cfg form,
        # and skip predicates that are already converted.
        rx = re.compile(
            OPENERS + r"(?!" + "|".join(re.escape(d) for d in DONE) + r")" + pat
        )
        out = rx.sub(lambda m, new=new: m.group("open") + new, out)

    # Every remaining mention must sit inside an accepted, converted form.
    rest = out
    for done in DONE:
        rest = rest.replace(done, "")
    if "test-hooks" in rest:
        return line, "unsupported test-hooks form"
    # A converted form must start right after a cfg opener.
    for m in re.finditer(re.escape(FEAT), out):
        start = out.rfind("(", 0, m.start())
        prefix = out[: start + 1]
        if not re.search(r"(cfg\(|cfg!\(|cfg_attr\()(not\()?(any\()?$", prefix):
            return (
                line,
                "test-hooks predicate is not the first argument of cfg, cfg_attr or cfg!",
            )
    return out, None


def main(argv):
    check = False
    args = []
    for a in argv:
        if a == "--check":
            check = True
        else:
            args.append(a)
    if not args:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    files = []
    for a in args:
        files.extend(rs_files(a))

    errors = []
    comments = []
    changed = {}
    gates = 0
    for f in files:
        text = f.read_text()
        new_lines = []
        for n, line in enumerate(text.splitlines(keepends=True), 1):
            new, err = convert_line(line)
            if err:
                errors.append(f"{f}:{n}: {err}: {line.strip()}")
            if "test-hooks" in line and is_comment(line):
                comments.append(f"{f}:{n}: {line.strip()}")
            if new != line:
                gates += 1
            new_lines.append(new)
        new_text = "".join(new_lines)
        if new_text != text:
            changed[f] = new_text

    if errors:
        for e in errors:
            print(e, file=sys.stderr)
        print(
            f"hooks_gate: {len(errors)} line(s) not converted, no file written",
            file=sys.stderr,
        )
        return 1

    if not check:
        for f, t in changed.items():
            f.write_text(t)
    verb = "would rewrite" if check else "rewrote"
    print(
        f"hooks_gate: {verb} {gates} gate line(s) in {len(changed)} of {len(files)} file(s)"
    )
    for c in comments:
        print(f"comment names test-hooks, check the wording: {c}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
