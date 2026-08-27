#!/usr/bin/env python3
"""Offline tests for the parity gate in `scripts/real_world_parity.py`.

Only `evaluate()` is exercised: it is the function that decides whether a run
is a regression, and it is pure -- results, gate config and approvals in,
verdict out. Every case below is a way the gate could have said "pass" about a
run nobody actually checked.

Run with `python3 scripts/real_world_parity_test.py`. No network, no corpus.
"""

from __future__ import annotations

import importlib.util
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]

_spec = importlib.util.spec_from_file_location(
    "real_world_parity", ROOT / "scripts" / "real_world_parity.py"
)
parity = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(parity)

CONFIG = {
    "thresholds": {
        "wall_seconds_factor": 3.0,
        "wall_seconds_floor": 2.0,
        "peak_rss_factor": 1.5,
        "peak_rss_floor_mb": 128.0,
    },
    "gate": {
        "min_score": 100.0,
        "weights": {
            "crash_free": 3,
            "parse_clean": 3,
            "notices_stable": 2,
            "features_stable": 1,
            "java_parity": 2,
        },
    },
}

NO_DELTAS: dict = {"entries": []}


def guru_result(**overrides) -> dict:
    base = {
        "crashed": False,
        "timed_out": False,
        "signal": None,
        "exit_code": 0,
        "wall_seconds": 1.0,
        "peak_rss_mb": 100.0,
        "report_read": True,
        "parse_failures": 0,
        "fingerprint": {},
        "features": [],
    }
    base.update(overrides)
    return base


def results(feeds: list[dict], tools: list[str] | None = None) -> dict:
    return {"tools": ["guru"] if tools is None else tools, "feeds": feeds}


class GateCase(unittest.TestCase):
    def setUp(self) -> None:
        # No committed baselines in a temp-free test: load_baseline reads the
        # repository, so point it at a directory that has none.
        self._real_baseline_dir = parity.BASELINE_DIR
        parity.BASELINE_DIR = ROOT / "scripts" / "real_world" / "does-not-exist"
        self.addCleanup(setattr, parity, "BASELINE_DIR", self._real_baseline_dir)

    def test_a_clean_run_passes(self) -> None:
        report = parity.evaluate(
            results([{"feed_id": "mdb-84", "guru": guru_result()}]), CONFIG, NO_DELTAS
        )
        self.assertTrue(report["passed"])
        self.assertEqual(report["score"], 100.0)

    def test_a_requested_tool_that_never_ran_fails(self) -> None:
        """The silent skip: guru was asked for and no result came back."""
        report = parity.evaluate(
            results([{"feed_id": "mdb-84"}]), CONFIG, NO_DELTAS
        )
        self.assertFalse(report["passed"])
        self.assertIn(
            "produced no result", " ".join(report["feeds"][0]["reasons"])
        )

    def test_a_java_only_run_still_skips_the_missing_guru_result(self) -> None:
        """`--tools java` legitimately has no guru result; that is not a failure."""
        report = parity.evaluate(
            results([{"feed_id": "mdb-84"}], tools=["java"]), CONFIG, NO_DELTAS
        )
        self.assertTrue(report["passed"])
        self.assertEqual(report["feeds"][0]["reasons"], [])

    def test_a_stale_approval_fails_the_run(self) -> None:
        """An approval that no longer matches a real difference must not linger."""
        feeds = [
            {
                "feed_id": "mdb-84",
                "guru": guru_result(fingerprint={}),
                "java": guru_result(fingerprint={}),
            }
        ]
        deltas = {
            "entries": [
                {"feed_id": "mdb-84", "code": "a_difference_we_since_fixed"}
            ]
        }
        report = parity.evaluate(results(feeds, ["guru", "java"]), CONFIG, deltas)
        self.assertEqual(len(report["stale_expected_deltas"]), 1)
        self.assertFalse(report["passed"])

    def test_a_near_miss_does_not_pass(self) -> None:
        """One bad feed among many still fails a min_score of 100.

        The published score is rounded and the decision is not, so a ratio
        fractionally under the minimum cannot present itself as a perfect run.
        """
        many = [
            {"feed_id": f"feed-{index}", "guru": guru_result()}
            for index in range(999)
        ]
        many.append({"feed_id": "feed-bad", "guru": guru_result(parse_failures=1)})
        report = parity.evaluate(results(many), CONFIG, NO_DELTAS)
        self.assertFalse(report["passed"])
        self.assertLess(report["score"], 100.0)

    def test_a_crash_fails_and_is_explained(self) -> None:
        report = parity.evaluate(
            results(
                [
                    {
                        "feed_id": "mdb-84",
                        "guru": guru_result(crashed=True, timed_out=True),
                    }
                ]
            ),
            CONFIG,
            NO_DELTAS,
        )
        self.assertFalse(report["passed"])
        self.assertIn("timed out", " ".join(report["feeds"][0]["reasons"]))

    def test_an_unusable_corpus_entry_fails(self) -> None:
        report = parity.evaluate(
            results([{"feed_id": "mdb-84", "corpus_error": "missing zip"}]),
            CONFIG,
            NO_DELTAS,
        )
        self.assertFalse(report["passed"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
