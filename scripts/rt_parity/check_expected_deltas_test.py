#!/usr/bin/env python3
"""Offline tests for `check_expected_deltas.py`.

Each test takes a ledger the checker should accept and injects one fault, so a
check that stopped checking fails here rather than going quiet while the ledger
rots. Run with `python3 scripts/rt_parity/check_expected_deltas_test.py`.
"""

from __future__ import annotations

import copy
import json
import pathlib
import subprocess
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import check_expected_deltas as checker  # noqa: E402

RT_BASELINE = {
    "specRevision": {"commit": "262ae1e46e3f66099284fb8e4f976dfec788501f"},
    "canonicalBaseline": {
        "commit": "7041fa3fcaf674bf730e17325c179d329cdff6f2",
        "jarSha256": "31b31b4b5f2d18562f5dcdd6dc407e4c1f1c9c07a9b63de65f0d37492a363ae8",
    },
    "canonicalBindingsSchema": {
        "commit": "c2ab4841effc5626889376b34b63e5fef1136c40",
        "sha256": "09a04b89995ddcbfce722baefc5be4a8dcfd61637d949cbf3998991dd219b26e",
    },
}

VALID_DELTA = {
    "id": "a-difference",
    "status": "proposed",
    "blockedOn": "a pending product decision",
    "canonicalRuleId": None,
    "noticeCode": None,
    "fixture": "a fixture",
    "javaResult": "rejected",
    "rustResult": "accepted",
    "reason": "because",
    "specReference": "the reference",
    "expected": {"java": "0", "rust": "1"},
    "removalCondition": "when it stops happening",
    "recordedAt": "2026-09-10",
}

VALID_LEDGER = {
    "schemaVersion": 1,
    "baseline": {
        "javaCommit": RT_BASELINE["canonicalBaseline"]["commit"],
        "javaJarSha256": RT_BASELINE["canonicalBaseline"]["jarSha256"],
        "rtSchemaCommit": RT_BASELINE["specRevision"]["commit"],
        "bindingsSchemaCommit": RT_BASELINE["canonicalBindingsSchema"]["commit"],
        "bindingsSchemaSha256": RT_BASELINE["canonicalBindingsSchema"]["sha256"],
    },
    "deltas": [copy.deepcopy(VALID_DELTA)],
}


def ledger(**overrides) -> dict:
    document = copy.deepcopy(VALID_LEDGER)
    document.update(overrides)
    return document


class CheckerCase(unittest.TestCase):
    def assertProblem(self, problems: list[str], fragment: str) -> None:
        self.assertTrue(
            any(fragment in problem for problem in problems),
            f"expected a problem containing {fragment!r}, got {problems}",
        )

    def test_a_valid_ledger_passes(self) -> None:
        self.assertEqual(checker.check(ledger(), RT_BASELINE), [])

    def test_the_committed_ledger_matches_the_committed_baseline(self) -> None:
        result = subprocess.run(
            [sys.executable, str(HERE / "check_expected_deltas.py")],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_wrong_schema_version_is_caught(self) -> None:
        problems = checker.check(ledger(schemaVersion=2), RT_BASELINE)
        self.assertProblem(problems, "schemaVersion")

    def test_duplicate_ids_are_caught(self) -> None:
        document = ledger()
        document["deltas"].append(copy.deepcopy(VALID_DELTA))
        self.assertProblem(checker.check(document, RT_BASELINE), "duplicate id")

    def test_a_proposed_delta_must_say_what_it_waits_on(self) -> None:
        document = ledger()
        del document["deltas"][0]["blockedOn"]
        self.assertProblem(checker.check(document, RT_BASELINE), "blockedOn")

    def test_an_approved_delta_must_not_still_be_blocked(self) -> None:
        document = ledger()
        document["deltas"][0]["status"] = "approved"
        self.assertProblem(checker.check(document, RT_BASELINE), "must not still be blockedOn")

    def test_a_missing_required_field_is_caught(self) -> None:
        document = ledger()
        del document["deltas"][0]["removalCondition"]
        self.assertProblem(checker.check(document, RT_BASELINE), "removalCondition")

    def test_a_nullable_field_must_still_be_present(self) -> None:
        document = ledger()
        del document["deltas"][0]["canonicalRuleId"]
        self.assertProblem(checker.check(document, RT_BASELINE), "may be null")

    def test_expected_must_carry_both_sides(self) -> None:
        document = ledger()
        document["deltas"][0]["expected"] = {"java": "0"}
        self.assertProblem(checker.check(document, RT_BASELINE), "both a java and a rust")

    def test_a_ledger_describing_another_oracle_is_caught(self) -> None:
        """The whole point: an approval is about one binary and one schema."""
        document = ledger()
        document["baseline"]["javaJarSha256"] = "0" * 64
        self.assertProblem(checker.check(document, RT_BASELINE), "different oracle")

    def test_a_moved_schema_pin_is_caught(self) -> None:
        document = ledger()
        document["baseline"]["rtSchemaCommit"] = "f" * 40
        self.assertProblem(checker.check(document, RT_BASELINE), "different oracle")

    def test_a_moved_bindings_schema_is_caught(self) -> None:
        document = ledger()
        document["baseline"]["bindingsSchemaCommit"] = "e" * 40
        self.assertProblem(checker.check(document, RT_BASELINE), "different oracle")

    def test_an_edited_bindings_schema_is_caught(self) -> None:
        document = ledger()
        document["baseline"]["bindingsSchemaSha256"] = "d" * 64
        self.assertProblem(checker.check(document, RT_BASELINE), "different oracle")

    def test_an_unknown_status_is_caught(self) -> None:
        document = ledger()
        document["deltas"][0]["status"] = "maybe"
        self.assertProblem(checker.check(document, RT_BASELINE), "status is")


if __name__ == "__main__":
    unittest.main(verbosity=2)
