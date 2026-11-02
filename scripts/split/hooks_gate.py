#!/usr/bin/env python3
"""Move tl-ui test-hook gates onto debug_assertions (ADR-020, "Test hooks").

Usage: scripts/split/hooks_gate.py [--check] PATH [PATH ...]

Each PATH is a .rs file or a directory (searched for .rs files). The script
rewrites these predicates when one is the first argument of cfg(...),
cfg_attr(...) or cfg!(...):

    feature = "test-hooks"
        -> any(debug_assertions, feature = "test-hooks")
    any(test, feature = "test-hooks")
        -> any(test, debug_assertions, feature = "test-hooks")
    not(feature = "test-hooks")
        -> not(any(debug_assertions, feature = "test-hooks"))
    not(any(test, feature = "test-hooks"))
        -> not(any(test, debug_assertions, feature = "test-hooks"))

The last line is the second form under not(), so its rewrite is the exact
negation of the second rewrite.

A cfg form may span several lines, as rustfmt writes a long cfg_attr. The
predicate may then have newlines and spaces between its tokens and a trailing
comma; the rewrite replaces the whole predicate with the one-line form.

Predicates already in the rewritten form are left as they are, so a second run
changes nothing. Any other code that names test-hooks (for example
all(feature = "test-hooks", ...), or a string outside a cfg predicate) is
printed with its line and the script exits 1 without writing any file.
Comments are never changed; the comment lines that name test-hooks are listed
so a person can reword them.

--check reports what would change and writes nothing.
"""

import re
import sys
from pathlib import Path

FEAT = 'feature = "test-hooks"'

# Token pieces with free whitespace between them, so a predicate that rustfmt
# split over lines still matches.
_W = r"\s*"
_FEAT = r'feature\s*=\s*"test-hooks"'
_END = r"\s*,?\s*\)"  # optional trailing comma, then the closing paren


def _any(*args):
    return r"any\(" + _W + (_W + "," + _W).join(args) + _END


def _not(inner):
    return r"not\(" + _W + inner + _END


# (regex, replacement). Replacement None means already converted: keep it.
# Longest forms first, so a shorter one never matches the start of a longer one.
_FORMS = [
    (_not(_any("test", "debug_assertions", _FEAT)), None),
    (_not(_any("debug_assertions", _FEAT)), None),
    (_any("test", "debug_assertions", _FEAT), None),
    (_any("debug_assertions", _FEAT), None),
    (_not(_any("test", _FEAT)), f"not(any(test, debug_assertions, {FEAT}))"),
    (_not(_FEAT), f"not(any(debug_assertions, {FEAT}))"),
    (_any("test", _FEAT), f"any(test, debug_assertions, {FEAT})"),
    (_FEAT, f"any(debug_assertions, {FEAT})"),
]
FORMS = [(re.compile(rx), new) for rx, new in _FORMS]

# A cfg form opens with one of these. The predicate is its first argument.
OPENER = re.compile(r"\b(?:cfg_attr\(|cfg!\(|cfg\()")
# After the predicate comes the next argument or the closing paren.
AFTER = re.compile(r"\s*[,)]")

_CHAR = re.compile(r"'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'\n])'")
_RAW = re.compile(r'b?r(#*)"')


def mask(text):
    """Return (code, skel), both the same length as text.

    code has every comment blanked (newlines kept). skel also blanks the inside
    of string and char literals, so parens in strings do not count.
    """
    code = list(text)
    skel = list(text)
    i, n = 0, len(text)

    def blank(a, b, which):
        for k in range(a, b):
            if text[k] != "\n":
                for buf in which:
                    buf[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j, (code, skel))
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j, (code, skel))
            i = j
        elif (m := _RAW.match(text, i)) and (i == 0 or not _ident(text[i - 1])):
            close = '"' + m.group(1)
            j = text.find(close, m.end())
            j = n if j < 0 else j
            blank(m.end(), j, (skel,))
            i = j + len(close)
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i + 1, min(j, n), (skel,))
            i = j + 1
        elif c == "'" and (m := _CHAR.match(text, i)):
            blank(i + 1, m.end() - 1, (skel,))
            i = m.end()
        else:
            i += 1
    return "".join(code), "".join(skel)


def _ident(ch):
    return ch.isalnum() or ch == "_"


def close_paren(skel, open_at):
    """Index just past the paren that closes the one at open_at."""
    depth = 0
    for k in range(open_at, len(skel)):
        if skel[k] == "(":
            depth += 1
        elif skel[k] == ")":
            depth -= 1
            if depth == 0:
                return k + 1
    return len(skel)


def convert_text(text):
    """Return (new_text, gates, errors). errors holds (offset, message)."""
    code, skel = mask(text)
    edits = []  # (start, end, replacement)
    ok_spans = []  # predicate spans that are fine after the rewrite
    cfg_spans = []  # whole cfg forms
    for m in OPENER.finditer(skel):
        cfg_spans.append((m.start(), close_paren(skel, m.end() - 1)))
        q = m.end()
        while q < len(code) and code[q].isspace():
            q += 1
        for rx, new in FORMS:
            pm = rx.match(code, q)
            if pm and AFTER.match(code, pm.end()):
                ok_spans.append((pm.start(), pm.end()))
                if new is not None:
                    edits.append((pm.start(), pm.end(), new))
                break

    errors = []
    for m in re.finditer("test-hooks", code):
        at = m.start()
        if any(a <= at < b for a, b in ok_spans):
            continue
        if any(a <= at < b for a, b in cfg_spans):
            msg = "test-hooks predicate is not a supported first argument of cfg, cfg_attr or cfg!"
        else:
            msg = "names test-hooks outside a cfg, cfg_attr or cfg! predicate"
        errors.append((at, msg))

    out = []
    last = 0
    for a, b, new in sorted(edits):
        out.append(text[last:a])
        out.append(new)
        last = b
    out.append(text[last:])
    return "".join(out), len(edits), errors


def comment_lines(text):
    """Line numbers and text of lines whose comments name test-hooks."""
    code, _ = mask(text)
    lines = text.splitlines()
    code_lines = code.splitlines()
    found = []
    for n, (line, cl) in enumerate(zip(lines, code_lines), 1):
        if line.count("test-hooks") > cl.count("test-hooks"):
            found.append((n, line.strip()))
    return found


def rs_files(arg):
    p = Path(arg)
    if p.is_dir():
        return sorted(q for q in p.rglob("*.rs") if q.is_file())
    if p.is_file():
        return [p]
    raise SystemExit(f"hooks_gate: no such file or directory: {arg}")


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
        new_text, n_gates, errs = convert_text(text)
        lines = text.splitlines()
        for at, msg in errs:
            n = text.count("\n", 0, at) + 1
            errors.append(f"{f}:{n}: {msg}: {lines[n - 1].strip()}")
        for n, line in comment_lines(text):
            comments.append(f"{f}:{n}: {line}")
        gates += n_gates
        if new_text != text:
            changed[f] = new_text

    if errors:
        for e in errors:
            print(e, file=sys.stderr)
        print(
            f"hooks_gate: {len(errors)} mention(s) not converted, no file written",
            file=sys.stderr,
        )
        return 1

    if not check:
        for f, t in changed.items():
            f.write_text(t)
    verb = "would rewrite" if check else "rewrote"
    print(
        f"hooks_gate: {verb} {gates} gate(s) in {len(changed)} of {len(files)} file(s)"
    )
    for c in comments:
        print(f"comment names test-hooks, check the wording: {c}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
