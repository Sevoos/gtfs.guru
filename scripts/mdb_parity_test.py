#!/usr/bin/env python3
"""Offline tests for `scripts/mdb_parity.py`.

Only `classify_feed()` is exercised: it decides whether a guru-vs-java
difference is approved, and it is pure. Each case is a way the catalogue
comparison could have said "exact-on-shared" about a feed nobody triaged.

Run with `python3 scripts/mdb_parity_test.py`. No network, no feeds.
"""

from __future__ import annotations

import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

_spec = importlib.util.spec_from_file_location(
    "mdb_parity", ROOT / "scripts" / "mdb_parity.py"
)
mdb_parity = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(mdb_parity)


def facts(fingerprint: dict) -> dict:
    return {"report_read": True, "fingerprint": fingerprint}


def code(severity: str, total: int) -> dict:
    return {"severity": severity, "total": total}


def delta(feed_id: str, code_: str, guru, java, reason: str = "triaged") -> dict:
    return {
        "feed_id": feed_id,
        "code": code_,
        "guru_total": guru,
        "java_total": java,
        "reason": reason,
    }


class ClassifyFeedTest(unittest.TestCase):
    def test_identical_fingerprints_are_exact(self):
        fp = {"unknown_column": code("INFO", 2)}
        verdict, used = mdb_parity.classify_feed("f", facts(fp), facts(dict(fp)), {})
        self.assertTrue(verdict["exact"])
        self.assertTrue(verdict["exact_on_shared"])
        self.assertEqual(verdict["unexplained"], [])
        self.assertEqual(used, set())

    def test_guru_only_code_without_approval_is_unexplained(self):
        guru = facts({"duplicate_key": code("ERROR", 2)})
        java = facts({})
        verdict, _ = mdb_parity.classify_feed("f", guru, java, {})
        self.assertFalse(verdict["exact_on_shared"])
        self.assertEqual(verdict["guru_only"][0]["code"], "duplicate_key")
        self.assertEqual(verdict["unexplained"][0]["guru"], 2)
        self.assertIsNone(verdict["unexplained"][0]["java"])

    def test_approval_with_matching_totals_explains_the_delta(self):
        guru = facts({"duplicate_key": code("ERROR", 2)})
        java = facts({})
        index = {("f", "duplicate_key"): delta("f", "duplicate_key", 2, "absent", "skip")}
        verdict, used = mdb_parity.classify_feed("f", guru, java, index)
        self.assertTrue(verdict["exact_on_shared"])
        self.assertFalse(verdict["exact"])
        self.assertEqual(verdict["explained"][0]["reason"], "skip")
        self.assertEqual(used, {("f", "duplicate_key")})

    def test_approval_with_stale_total_does_not_apply(self):
        # The approval pinned 2; the run now shows 3. That is a new difference.
        guru = facts({"duplicate_key": code("ERROR", 3)})
        java = facts({})
        index = {("f", "duplicate_key"): delta("f", "duplicate_key", 2, "absent")}
        verdict, used = mdb_parity.classify_feed("f", guru, java, index)
        self.assertFalse(verdict["exact_on_shared"])
        self.assertEqual(used, set())

    def test_count_difference_on_shared_code_is_unexplained(self):
        guru = facts({"non_ascii_or_non_printable_char": code("WARNING", 6000)})
        java = facts({"non_ascii_or_non_printable_char": code("WARNING", 5720)})
        verdict, _ = mdb_parity.classify_feed("f", guru, java, {})
        self.assertEqual(verdict["count_diff"][0]["guru"], 6000)
        self.assertEqual(verdict["count_diff"][0]["java"], 5720)
        self.assertFalse(verdict["exact_on_shared"])

    def test_severity_difference_is_reported_even_with_equal_totals(self):
        guru = facts({"x": code("ERROR", 1)})
        java = facts({"x": code("WARNING", 1)})
        verdict, _ = mdb_parity.classify_feed("f", guru, java, {})
        self.assertEqual(verdict["severity_diff"][0]["guru_severity"], "ERROR")
        self.assertFalse(verdict["exact_on_shared"])

    def test_approval_for_another_feed_does_not_leak(self):
        guru = facts({"duplicate_key": code("ERROR", 2)})
        java = facts({})
        index = {("other", "duplicate_key"): delta("other", "duplicate_key", 2, "absent")}
        verdict, used = mdb_parity.classify_feed("f", guru, java, index)
        self.assertFalse(verdict["exact_on_shared"])
        self.assertEqual(used, set())

    def test_java_only_code_is_unexplained_until_approved(self):
        guru = facts({})
        java = facts({"leading_or_trailing_whitespaces": code("WARNING", 33)})
        verdict, _ = mdb_parity.classify_feed("f", guru, java, {})
        self.assertEqual(verdict["java_only"][0]["java"], 33)
        self.assertFalse(verdict["exact_on_shared"])
        index = {
            ("f", "leading_or_trailing_whitespaces"): delta(
                "f", "leading_or_trailing_whitespaces", "absent", 33
            )
        }
        verdict, _ = mdb_parity.classify_feed("f", guru, java, index)
        self.assertTrue(verdict["exact_on_shared"])


if __name__ == "__main__":
    unittest.main()
