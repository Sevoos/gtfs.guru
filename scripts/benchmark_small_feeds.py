#!/usr/bin/env python3
"""Time both validators over the small-feed corpus in `test-gtfs-feeds/`.

docs/benchmarks.md measures two large real-world feeds, where the validation
work dominates. This measures the opposite end: feeds of a few kilobytes, where
the JVM's startup dominates and the gap is widest. Without it the site's
small-feed speed claim had no source in the repository.

Method matches docs/benchmarks.md as closely as a small feed allows. Both tools
run their normal pipeline and write their normal report files; stdout and
stderr are discarded so terminal logging is not measured; the Java run passes
--skip_validator_update so its online version check is not counted as
validation time. Each case is run REPS times and the median is kept.

    scripts/benchmark_small_feeds.py --jar benchmark-feeds/gtfs-validator-8.0.1-cli.jar

The jar is the same pinned asset the parity suites use
(scripts/real_world/gate.json -> java_baseline); fetch it with
`scripts/real_world_parity.py jar`.
"""

from __future__ import annotations

import argparse
import json
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def discover_cases(corpus: Path) -> list[Path]:
    """Every zipped feed plus every directory that looks like an unpacked one."""
    cases = [path for path in sorted(corpus.rglob("*.zip"))]
    for path in sorted(corpus.rglob("*")):
        if path.is_dir() and any(child.suffix == ".txt" for child in path.iterdir()):
            cases.append(path)
    return cases


def run_once(cmd: list[str], out_dir: Path, timeout: int) -> tuple[float, int]:
    # A fresh output directory per run: neither tool should be timed while
    # deciding what to do about last run's files.
    shutil.rmtree(out_dir, ignore_errors=True)
    out_dir.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    proc = subprocess.run(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=timeout,
    )
    return time.perf_counter() - started, proc.returncode


def percentile(values: list[float], fraction: float) -> float:
    return values[min(int(len(values) * fraction), len(values) - 1)]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--jar",
        type=Path,
        default=REPO / "benchmark-feeds/gtfs-validator-8.0.1-cli.jar",
        help="canonical Java validator jar",
    )
    parser.add_argument(
        "--bin",
        type=Path,
        default=REPO / "target/release/gtfs-guru",
        help="release gtfs-guru binary",
    )
    parser.add_argument("--corpus", type=Path, default=REPO / "test-gtfs-feeds")
    parser.add_argument("--reps", type=int, default=3)
    parser.add_argument("--limit", type=int, default=0, help="first N cases only")
    parser.add_argument("--java-xmx", default="8G")
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--json", type=Path, help="write per-case results here")
    args = parser.parse_args()

    for path, what in ((args.jar, "jar"), (args.bin, "binary"), (args.corpus, "corpus")):
        if not path.exists():
            sys.exit(f"missing {what}: {path}")

    work = REPO / "target/benchmark-small-feeds"
    cases = discover_cases(args.corpus)
    if args.limit:
        cases = cases[: args.limit]
    print(f"{len(cases)} cases, {args.reps} reps each", file=sys.stderr)

    rows = []
    for index, case in enumerate(cases, 1):
        name = case.relative_to(args.corpus).as_posix()
        java_times, guru_times = [], []
        java_rc = guru_rc = None
        for _ in range(args.reps):
            duration, java_rc = run_once(
                [
                    "java",
                    f"-Xmx{args.java_xmx}",
                    "-jar",
                    str(args.jar),
                    "--input",
                    str(case),
                    "--output_base",
                    str(work / "java"),
                    "--skip_validator_update",
                ],
                work / "java",
                args.timeout,
            )
            java_times.append(duration)
            duration, guru_rc = run_once(
                [str(args.bin), "--input", str(case), "--output", str(work / "guru")],
                work / "guru",
                args.timeout,
            )
            guru_times.append(duration)

        java = statistics.median(java_times)
        guru = statistics.median(guru_times)
        rows.append(
            {
                "case": name,
                "java_seconds": java,
                "guru_seconds": guru,
                "speedup": java / guru if guru else None,
                "java_returncode": java_rc,
                "guru_returncode": guru_rc,
            }
        )
        print(
            f"[{index}/{len(cases)}] {name}: java {java:.3f}s "
            f"guru {guru:.4f}s -> {java / guru:.0f}x",
            file=sys.stderr,
        )

    # A case only counts when the Java baseline actually validated it; the
    # gtfs-guru exit code is a severity gate, not a failure signal.
    usable = [row for row in rows if row["speedup"] and row["java_returncode"] == 0]
    if not usable:
        sys.exit("no case produced a comparable pair of runs")
    speedups = sorted(row["speedup"] for row in usable)

    summary = {
        "cases": len(usable),
        "reps": args.reps,
        "java_median_seconds": statistics.median(r["java_seconds"] for r in usable),
        "guru_median_seconds": statistics.median(r["guru_seconds"] for r in usable),
        "speedup_min": speedups[0],
        "speedup_p50": statistics.median(speedups),
        "speedup_p90": percentile(speedups, 0.90),
        "speedup_max": speedups[-1],
        "cases_at_or_above_100x": sum(1 for value in speedups if value >= 100),
    }

    if args.json:
        args.json.write_text(json.dumps({"summary": summary, "cases": rows}, indent=2))

    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
