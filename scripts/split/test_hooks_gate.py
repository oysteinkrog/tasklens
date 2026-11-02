"""Tests for hooks_gate.py. Run: python3 scripts/split/test_hooks_gate.py"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from hooks_gate import convert_text  # noqa: E402

PLAIN = 'any(debug_assertions, feature = "test-hooks")'
ANY_TEST = 'any(test, debug_assertions, feature = "test-hooks")'
NOT = f"not({PLAIN})"
NOT_ANY_TEST = f"not({ANY_TEST})"


def conv(src):
    out, gates, errors = convert_text(src)
    return out, gates, [msg for _, msg in errors]


class OneLine(unittest.TestCase):
    def test_plain(self):
        out, gates, errs = conv('#[cfg(feature = "test-hooks")]\nfn a() {}\n')
        self.assertEqual(out, f"#[cfg({PLAIN})]\nfn a() {{}}\n")
        self.assertEqual((gates, errs), (1, []))

    def test_any_test(self):
        out, _, errs = conv('#[cfg(any(test, feature = "test-hooks"))]\n')
        self.assertEqual(out, f"#[cfg({ANY_TEST})]\n")
        self.assertEqual(errs, [])

    def test_not(self):
        out, _, errs = conv('#[cfg(not(feature = "test-hooks"))]\n')
        self.assertEqual(out, f"#[cfg({NOT})]\n")
        self.assertEqual(errs, [])

    def test_not_any_test(self):
        out, _, errs = conv('#[cfg(not(any(test, feature = "test-hooks")))]\n')
        self.assertEqual(out, f"#[cfg({NOT_ANY_TEST})]\n")
        self.assertEqual(errs, [])

    def test_cfg_attr_and_macro(self):
        src = (
            '#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]\n'
            'if cfg!(feature = "test-hooks") {}\n'
        )
        out, gates, errs = conv(src)
        self.assertEqual(
            out, f"#[cfg_attr({NOT}, allow(dead_code))]\nif cfg!({PLAIN}) {{}}\n"
        )
        self.assertEqual((gates, errs), (2, []))

    def test_second_run_changes_nothing(self):
        src = (
            f"#[cfg({PLAIN})]\n#[cfg({ANY_TEST})]\n#[cfg({NOT})]\n"
            f"#[cfg({NOT_ANY_TEST})]\n#[cfg_attr({NOT}, allow(dead_code))]\n"
        )
        self.assertEqual(conv(src), (src, 0, []))


class MultiLine(unittest.TestCase):
    def test_chart_cfg_attr(self):
        # The form rustfmt writes in chart/element.rs.
        src = (
            "    #[cfg_attr(\n"
            '        not(any(test, feature = "test-hooks")),\n'
            '        allow(dead_code, reason = "read by the bench paint hook only")\n'
            "    )]\n"
            "    newest_ns: u64,\n"
        )
        out, gates, errs = conv(src)
        self.assertEqual(errs, [])
        self.assertEqual(gates, 1)
        self.assertEqual(
            out,
            "    #[cfg_attr(\n"
            f"        {NOT_ANY_TEST},\n"
            '        allow(dead_code, reason = "read by the bench paint hook only")\n'
            "    )]\n"
            "    newest_ns: u64,\n",
        )

    def test_each_form_split_inside(self):
        cases = [
            ('feature\n    = "test-hooks"', PLAIN),
            ('any(\n    test,\n    feature = "test-hooks",\n)', ANY_TEST),
            ('not(\n    feature = "test-hooks"\n)', NOT),
            (
                'not(\n    any(\n        test,\n        feature = "test-hooks",\n    ),\n)',
                NOT_ANY_TEST,
            ),
        ]
        for old, new in cases:
            with self.subTest(old=old):
                out, gates, errs = conv(
                    f"#[cfg_attr(\n    {old},\n    allow(dead_code)\n)]\n"
                )
                self.assertEqual(errs, [])
                self.assertEqual(gates, 1)
                self.assertEqual(
                    out, f"#[cfg_attr(\n    {new},\n    allow(dead_code)\n)]\n"
                )

    def test_split_converted_form_is_kept(self):
        src = (
            "#[cfg_attr(\n"
            "    not(any(\n"
            "        test,\n"
            "        debug_assertions,\n"
            '        feature = "test-hooks"\n'
            "    )),\n"
            "    allow(dead_code)\n"
            ")]\n"
        )
        self.assertEqual(conv(src), (src, 0, []))


class Refused(unittest.TestCase):
    def test_all(self):
        _, _, errs = conv('#[cfg(all(unix, feature = "test-hooks"))]\n')
        self.assertEqual(len(errs), 1)

    def test_all_multiline(self):
        src = '#[cfg(all(\n    unix,\n    feature = "test-hooks",\n))]\n'
        _, _, errs = conv(src)
        self.assertEqual(len(errs), 1)

    def test_not_first_argument(self):
        _, _, errs = conv('#[cfg_attr(test, cfg(any(unix, feature = "test-hooks")))]\n')
        self.assertEqual(len(errs), 1)

    def test_string_outside_cfg(self):
        _, _, errs = conv('#[allow(unused_mut, reason = "test-hooks appends pages")]\n')
        self.assertEqual(len(errs), 1)

    def test_refusal_writes_nothing_for_whole_file(self):
        src = '#[cfg(feature = "test-hooks")]\n#[cfg(all(unix, feature = "test-hooks"))]\n'
        out, _, errs = conv(src)
        self.assertEqual(len(errs), 1)
        # convert_text still returns the rewrite; main() writes no file on error.
        self.assertIn(PLAIN, out)


class Comments(unittest.TestCase):
    def test_comments_untouched(self):
        src = (
            '// cfg(feature = "test-hooks") in a comment\n'
            '/* cfg(feature = "test-hooks")\n   still a comment */\n'
            '/// docs: cfg(feature = "test-hooks")\n'
        )
        self.assertEqual(conv(src), (src, 0, []))

    def test_url_in_string_is_not_a_comment(self):
        src = 'const U: &str = "http://x";\n#[cfg(feature = "test-hooks")]\n'
        out, gates, errs = conv(src)
        self.assertEqual((gates, errs), (1, []))
        self.assertIn(PLAIN, out)


if __name__ == "__main__":
    unittest.main()
