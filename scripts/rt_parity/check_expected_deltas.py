#!/usr/bin/env python3
"""Validate the GTFS-Realtime approved-deltas ledger.

`expected_deltas.json` records the differences between GTFS Guru and the pinned
canonical Java validator that are accepted rather than fixed. An allowlist
nobody enforces decays into standing permission for differences no one has
looked at since, so the format is checked mechanically:

* every delta records what GTF-11 requires of one, including which oracle
  binary and schema revision it was observed against;
* the ledger's baseline matches the RT baseline this build actually pins, so an
  approval cannot silently describe a different oracle;
* a proposed delta names what it is waiting on, and an approved one does not.

Offline. Run `python3 scripts/rt_parity/check_expected_deltas.py`.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_LEDGER = ROOT / "scripts" / "rt_parity" / "expected_deltas.json"
DEFAULT_RT_BASELINE = ROOT / "crates" / "gtfs_validator_rt" / "spec_baseline.json"

SCHEMA_VERSION = 1
STATUSES = ("proposed", "approved")

# The record GTF-11 requires of every approved delta. `canonicalRuleId` and
# `noticeCode` must be present but may be null: a divergence in the decoder
# belongs to no rule, and notice codes cannot exist before the matrix is frozen.
REQUIRED_TEXT_FIELDS = (
    "id",
    "fixture",
    "javaResult",
    "rustResult",
    "reason",
    "specReference",
    "removalCondition",
    "recordedAt",
)
NULLABLE_FIELDS = ("canonicalRuleId", "noticeCode")
INHERITED_FIELDS = ("javaCommit", "javaJarSha256", "rtSchemaCommit")


def check(ledger: dict, rt_baseline: dict) -> list[str]:
    problems: list[str] = []

    if ledger.get("schemaVersion") != SCHEMA_VERSION:
        problems.append(
            f"schemaVersion is {ledger.get('schemaVersion')!r}, expected {SCHEMA_VERSION}"
        )

    baseline = ledger.get("baseline") or {}
    for field in INHERITED_FIELDS:
        if not baseline.get(field):
            problems.append(f"baseline is missing {field}")

    # An approval describes a difference against one oracle binary and one
    # schema. If the build has moved on, every approval in the file is about
    # something else until it is re-observed.
    pinned = rt_baseline["canonicalBaseline"]
    expected = {
        "javaCommit": pinned["commit"],
        "javaJarSha256": pinned.get("jarSha256"),
        "rtSchemaCommit": rt_baseline["specRevision"]["commit"],
    }
    for field, want in expected.items():
        got = baseline.get(field)
        if want and got and got != want:
            problems.append(
                f"baseline {field} is {got[:12]}, but spec_baseline.json pins {want[:12]}: "
                "the ledger describes a different oracle than this build"
            )

    deltas = ledger.get("deltas")
    if not isinstance(deltas, list):
        problems.append("deltas is missing or not a list")
        return problems

    seen: set[str] = set()
    for index, delta in enumerate(deltas):
        label = delta.get("id") or f"deltas[{index}]"

        for field in REQUIRED_TEXT_FIELDS:
            value = delta.get(field)
            if not isinstance(value, str) or not value.strip():
                problems.append(f"{label}: {field} is missing or empty")

        for field in NULLABLE_FIELDS:
            if field not in delta:
                problems.append(f"{label}: {field} must be present, and may be null")

        identifier = delta.get("id")
        if isinstance(identifier, str):
            if identifier in seen:
                problems.append(f"{label}: duplicate id")
            seen.add(identifier)

        status = delta.get("status")
        if status not in STATUSES:
            problems.append(f"{label}: status is {status!r}, expected one of {STATUSES}")
        elif status == "proposed" and not delta.get("blockedOn"):
            problems.append(
                f"{label}: a proposed delta must name what it is waiting on in blockedOn"
            )
        elif status == "approved" and delta.get("blockedOn"):
            problems.append(
                f"{label}: an approved delta must not still be blockedOn "
                f"{delta['blockedOn']!r}"
            )

        expected_results = delta.get("expected")
        if not isinstance(expected_results, dict) or not {"java", "rust"} <= expected_results.keys():
            problems.append(f"{label}: expected must record both a java and a rust result")

        # Inherited from the ledger baseline unless overridden, so every delta
        # resolves to a commit and a digest without repeating them by hand.
        for field in INHERITED_FIELDS:
            if not (delta.get(field) or baseline.get(field)):
                problems.append(f"{label}: {field} resolves to nothing")

    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=pathlib.Path, default=DEFAULT_LEDGER)
    parser.add_argument("--rt-baseline", type=pathlib.Path, default=DEFAULT_RT_BASELINE)
    args = parser.parse_args(argv)

    ledger = json.loads(args.ledger.read_text(encoding="utf-8"))
    rt_baseline = json.loads(args.rt_baseline.read_text(encoding="utf-8"))

    problems = check(ledger, rt_baseline)
    if problems:
        print(f"{args.ledger.name}: {len(problems)} problem(s)")
        for problem in problems:
            print(f"  - {problem}")
        return 1

    counts: dict[str, int] = {}
    for delta in ledger["deltas"]:
        counts[delta["status"]] = counts.get(delta["status"], 0) + 1
    summary = ", ".join(f"{count} {status}" for status, count in sorted(counts.items())) or "empty"
    print(f"{args.ledger.name}: {summary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
