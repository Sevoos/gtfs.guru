#!/usr/bin/env python3
"""Catalogue parity: gtfs.guru against gtfs-validator on the mdb-50 set.

The 12-feed corpus in real_world_parity.py is the per-change gate. This is
the wider check a release runs: fifty MobilityDatabase feeds, both validators
on the same zips with the same date and country code, then a per-feed diff of
notice codes and totals. Every difference must be either absent or explained
by an entry in the expected-deltas file, in the same format the corpus uses,
so the answer "how far are we from the canon?" is a number, not a diff you
read yourself.

Commands:
  run      validate every manifest feed with guru and/or java
  compare  diff guru against java per feed, apply expected deltas, write JSON
  fetch    download latest.zip for manifest feeds that are missing locally

Layout under --dir (default benchmark-feeds/mdb-50):
  zips/<feed_id>.zip            input, never committed
  parity/<tool>/<feed_id>/      report.json, system_errors.json, report.html
  parity/comparison.json        output of `compare`

Exit codes: 0 pass, 1 usage or infrastructure failure, 2 unexplained deltas.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import real_world_parity as rwp  # noqa: E402

REPO = HERE.parent
MANIFEST_JSON = HERE / "real_world" / "mdb50.json"
DELTAS_JSON = HERE / "real_world" / "mdb50_expected_deltas.json"
DEFAULT_DIR = REPO / "benchmark-feeds" / "mdb-50"


def manifest_feeds(patterns: list[str] | None) -> tuple[dict, list[dict]]:
    manifest = rwp.load_json(MANIFEST_JSON, "mdb-50 manifest")
    feeds = manifest["feeds"]
    if patterns:
        import fnmatch

        feeds = [
            feed
            for feed in feeds
            if any(fnmatch.fnmatch(feed["feed_id"], pat) for pat in patterns)
        ]
        if not feeds:
            sys.exit(f"no manifest feed matches {patterns}")
    return manifest, feeds


def parity_dir(explicit: str | None) -> Path:
    return Path(explicit).expanduser() if explicit else DEFAULT_DIR


# --------------------------------------------------------------------------- #
# fetch
# --------------------------------------------------------------------------- #
def cmd_fetch(args: argparse.Namespace) -> int:
    import urllib.error
    import urllib.request

    manifest, feeds = manifest_feeds(args.feed)
    zips = parity_dir(args.dir) / "zips"
    zips.mkdir(parents=True, exist_ok=True)
    template = manifest["latest_url_template"]
    failures = 0
    for feed in feeds:
        dest = zips / f"{feed['feed_id']}.zip"
        if dest.exists() and not args.force:
            continue
        url = template.format(feed_id=feed["feed_id"])
        request = urllib.request.Request(url, headers={"User-Agent": "gtfs.guru-mdb-parity/1"})
        try:
            with urllib.request.urlopen(request, timeout=args.timeout) as response:
                tmp = dest.with_suffix(".part")
                with tmp.open("wb") as out:
                    shutil.copyfileobj(response, out)
                tmp.replace(dest)
        except (urllib.error.URLError, OSError) as exc:
            failures += 1
            print(f"FAIL {feed['feed_id']}: {exc}", file=sys.stderr)
            continue
        digest = rwp.sha256_file(dest)
        note = "" if digest == feed["sha256"] else "  (differs from manifest snapshot)"
        print(f"ok   {feed['feed_id']}  {dest.stat().st_size} bytes{note}")
    return 1 if failures else 0


# --------------------------------------------------------------------------- #
# run
# --------------------------------------------------------------------------- #
def cmd_run(args: argparse.Namespace) -> int:
    manifest, feeds = manifest_feeds(args.feed)
    config = rwp.load_json(rwp.GATE_JSON, "gate config")
    pin = config["java_baseline"]
    tools = [t.strip() for t in args.tools.split(",") if t.strip()]
    unknown = set(tools) - {"guru", "java"}
    if unknown:
        sys.exit(f"unknown tool(s): {sorted(unknown)}")

    directory = parity_dir(args.dir)
    out_root = directory / "parity"
    date = args.date or manifest["date"]

    guru_base = rwp.guru_binary(args.bin) if "guru" in tools else None
    jar = rwp.ensure_jar(pin, args.jar) if "java" in tools else None
    java_bin = rwp.resolve_java(args.java_bin) if "java" in tools else None
    java_xmx = args.java_xmx or pin["java_xmx"]

    env = dict(os.environ)
    env["RAYON_NUM_THREADS"] = str(args.threads)

    results = {
        "schema": 1,
        "git_sha": rwp.git_sha(),
        "date": date,
        "tools": tools,
        "threads": args.threads,
        "java_baseline": {
            "release_tag": pin["release_tag"],
            "asset": pin["asset"],
            "sha256": pin["sha256"],
            "java_runtime": rwp.java_runtime_version(java_bin),
        }
        if jar
        else None,
        "feeds": [],
    }

    failures = 0
    for feed in feeds:
        zip_path = directory / "zips" / f"{feed['feed_id']}.zip"
        entry: dict = {
            "feed_id": feed["feed_id"],
            "provider": feed.get("provider"),
            "country_code": feed["country_code"],
        }
        if not zip_path.exists():
            entry["corpus_error"] = f"missing {zip_path}"
            print(f"MISSING  {feed['feed_id']}: {zip_path}", file=sys.stderr)
            failures += 1
            results["feeds"].append(entry)
            continue
        digest = rwp.sha256_file(zip_path)
        entry["sha256"] = digest
        if digest != feed["sha256"]:
            entry["snapshot_differs"] = True
            print(
                f"NOTE     {feed['feed_id']}: local zip differs from the manifest "
                "snapshot; expected deltas were triaged against the manifest bytes",
                file=sys.stderr,
            )

        run_feed = {**feed, "date": date}
        for tool in tools:
            out = out_root / tool / feed["feed_id"]
            if out.exists():
                shutil.rmtree(out)
            out.mkdir(parents=True, exist_ok=True)
            log = out_root / tool / f"{feed['feed_id']}.log"
            if tool == "guru":
                cmd = rwp.guru_command(guru_base, run_feed, zip_path, out, args.threads)
                timeout = config["timeouts_seconds"]["guru"]
            else:
                cmd = rwp.java_command(
                    java_bin, jar, run_feed, zip_path, out, args.threads, java_xmx
                )
                timeout = config["timeouts_seconds"]["java"]
            measured = rwp.run_measured(cmd, timeout, log, env=env)
            facts = rwp.read_outputs(out)
            entry[tool] = {**measured, **facts, "command": cmd}
            status = "crash" if measured["crashed"] else "ok"
            print(
                f"{tool:<5} {feed['feed_id']:<26} {status:<5} "
                f"{measured['wall_seconds']:>8.2f}s "
                f"E={facts['errors']} W={facts['warnings']} I={facts['infos']} "
                f"codes={len(facts['fingerprint'])}"
            )
        results["feeds"].append(entry)

    out_file = out_root / "results.json"
    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print(f"\nwrote {out_file}")
    return 1 if failures else 0


# --------------------------------------------------------------------------- #
# compare
# --------------------------------------------------------------------------- #
def classify_feed(feed_id: str, guru: dict, java: dict, index: dict) -> tuple[dict, set]:
    """One feed's guru-vs-java diff, split into explained and unexplained."""
    used: set[tuple[str, str]] = set()
    verdict: dict = {
        "feed_id": feed_id,
        "guru_status": "ok" if guru["report_read"] else "unreadable",
        "java_status": "ok" if java["report_read"] else "unreadable",
        "shared_codes": sorted(set(guru["fingerprint"]) & set(java["fingerprint"])),
        "count_diff": [],
        "severity_diff": [],
        "guru_only": [],
        "java_only": [],
        "explained": [],
        "unexplained": [],
    }
    for diff in rwp.compare_to_java(guru, java):
        code = diff["code"]
        before, after = diff["before"], diff["after"]  # java, guru
        entry = index.get((feed_id, code))
        row = {"code": code, "severity": diff["severity"], "java": before, "guru": after}
        if before is None:
            verdict["guru_only"].append(row)
        elif after is None:
            verdict["java_only"].append(row)
        else:
            g_sev = guru["fingerprint"][code]["severity"]
            j_sev = java["fingerprint"][code]["severity"]
            if g_sev != j_sev:
                verdict["severity_diff"].append({**row, "guru_severity": g_sev, "java_severity": j_sev})
            if before != after:
                verdict["count_diff"].append(row)
        approved = False
        if entry is not None:
            wanted = {
                "guru": rwp.expected_total(entry.get("guru_total", after)),
                "java": rwp.expected_total(entry.get("java_total", before)),
            }
            approved = wanted["guru"] == after and wanted["java"] == before
            if approved:
                used.add((feed_id, code))
        (verdict["explained"] if approved else verdict["unexplained"]).append(
            {**row, "reason": entry.get("reason") if entry and approved else None}
        )
    verdict["exact"] = not rwp.compare_to_java(guru, java)
    verdict["exact_on_shared"] = not verdict["unexplained"]
    return verdict, used


def cmd_compare(args: argparse.Namespace) -> int:
    manifest, feeds = manifest_feeds(args.feed)
    # The corpus approvals apply to the feeds shared with the 12-feed corpus;
    # the mdb-50 file adds the rest and wins on a duplicate key.
    deltas = {"entries": []}
    for path in (rwp.DELTAS_JSON, DELTAS_JSON):
        if path.exists():
            deltas["entries"].extend(rwp.load_json(path, "expected deltas")["entries"])
    index = rwp.expected_delta_index(deltas)
    directory = parity_dir(args.dir)
    out_root = directory / "parity"
    results_path = out_root / "results.json"
    results = json.loads(results_path.read_text(encoding="utf-8")) if results_path.exists() else {}

    report = {
        "schema": 1,
        "date": dt.date.today().isoformat(),
        "run_date": results.get("date"),
        "git_sha": rwp.git_sha(),
        "java": (results.get("java_baseline") or {}).get("release_tag"),
        "guru_mode": "default (no --thorough, no --google_rules)",
        "feeds": [],
    }
    used_all: set[tuple[str, str]] = set()
    missing = 0
    unexplained_feeds = 0
    for feed in feeds:
        feed_id = feed["feed_id"]
        guru_dir = out_root / "guru" / feed_id
        java_dir = out_root / "java" / feed_id
        if not guru_dir.exists() or not java_dir.exists():
            missing += 1
            report["feeds"].append({"feed_id": feed_id, "missing": [
                t for t, d in (("guru", guru_dir), ("java", java_dir)) if not d.exists()
            ]})
            continue
        guru = rwp.read_outputs(guru_dir)
        java = rwp.read_outputs(java_dir)
        verdict, used = classify_feed(feed_id, guru, java, index)
        verdict["country_code"] = feed["country_code"]
        verdict["provider"] = feed.get("provider")
        used_all |= used
        if not verdict["exact_on_shared"]:
            unexplained_feeds += 1
        report["feeds"].append(verdict)

    stale = [
        f"{e['feed_id']}/{e['code']}"
        for e in deltas.get("entries", [])
        if (e["feed_id"], e["code"]) not in used_all
        and any(f["feed_id"] == e["feed_id"] for f in feeds)
        and not any(f.get("missing") for f in report["feeds"] if f["feed_id"] == e["feed_id"])
    ]
    compared = [f for f in report["feeds"] if "missing" not in f]
    report["summary"] = {
        "feeds": len(feeds),
        "compared": len(compared),
        "missing_outputs": missing,
        "exact": sum(1 for f in compared if f["exact"]),
        "exact_on_shared": sum(1 for f in compared if f["exact_on_shared"]),
        "explained_deltas": sum(len(f["explained"]) for f in compared),
        "unexplained_deltas": sum(len(f["unexplained"]) for f in compared),
        "stale_expected_deltas": stale,
    }

    out_file = out_root / "comparison.json"
    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")

    s = report["summary"]
    print(f"{'feed':<26} {'cc':<3} {'exact':<6} {'shared':<7} deltas")
    for f in report["feeds"]:
        if "missing" in f:
            print(f"{f['feed_id']:<26} {'':<3} {'-':<6} {'-':<7} missing {','.join(f['missing'])}")
            continue
        notes = [f"{d['code']} {d['java']}->{d['guru']}" for d in f["unexplained"]]
        notes += [f"({d['code']} {d['java']}->{d['guru']})" for d in f["explained"]]
        print(
            f"{f['feed_id']:<26} {f['country_code']:<3} "
            f"{'yes' if f['exact'] else 'no':<6} "
            f"{'yes' if f['exact_on_shared'] else 'NO':<7} {'; '.join(notes)}"
        )
    print(
        f"\n{s['exact']}/{s['compared']} exact, {s['exact_on_shared']}/{s['compared']} "
        f"exact-on-shared, {s['explained_deltas']} explained, "
        f"{s['unexplained_deltas']} unexplained; {s['missing_outputs']} without outputs"
    )
    if stale:
        print(f"stale expected deltas (no longer observed): {', '.join(stale)}")
    print(f"wrote {out_file}")
    if unexplained_feeds or (stale and args.strict_stale):
        return 2
    return 0


# --------------------------------------------------------------------------- #
def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    fetch = sub.add_parser("fetch", help="download missing latest.zip files")
    fetch.add_argument("--dir")
    fetch.add_argument("--feed", action="append", help="feed_id glob, repeatable")
    fetch.add_argument("--force", action="store_true")
    fetch.add_argument("--timeout", type=int, default=300)
    fetch.set_defaults(func=cmd_fetch)

    run = sub.add_parser("run", help="validate manifest feeds with guru and/or java")
    run.add_argument("--dir")
    run.add_argument("--feed", action="append", help="feed_id glob, repeatable")
    run.add_argument("--tools", default="guru,java")
    run.add_argument("--bin", help="gtfs-guru binary (default target/release or GTFS_VALIDATOR_BIN)")
    run.add_argument("--jar", help="gtfs-validator jar (default: pinned, downloaded if absent)")
    run.add_argument("--java-bin")
    run.add_argument("--java-xmx")
    run.add_argument("--threads", type=int, default=4)
    run.add_argument("--date", help="validation date YYYY-MM-DD (default: manifest date)")
    run.set_defaults(func=cmd_run)

    compare = sub.add_parser("compare", help="diff guru against java and apply expected deltas")
    compare.add_argument("--dir")
    compare.add_argument("--feed", action="append", help="feed_id glob, repeatable")
    compare.add_argument("--strict-stale", action="store_true", help="fail on unused expected deltas")
    compare.set_defaults(func=cmd_compare)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
