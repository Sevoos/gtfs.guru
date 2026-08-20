#!/usr/bin/env python3
"""Fetch, verify and drift-check the pinned real-world GTFS corpus.

The corpus is a set of immutable MobilityData dataset snapshots. Feeds are
pinned by ``dataset_id`` and verified by sha256, so a run is reproducible on any
machine. The .zip files stay out of git; see docs/real-world-parity.md.

Commands:
  fetch          download every missing or mismatched feed into the corpus dir
  verify         hash what is on disk against the manifest, download nothing
  check-updates  ask MobilityData whether a newer dataset exists for each feed
  resolve        find the current dataset_id for a feed id, for re-pinning
  describe       print the feature/size coverage matrix behind the selection
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import os
import sys
import urllib.error
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor
from datetime import timedelta, timezone
from email.utils import parsedate_to_datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORPUS_JSON = HERE / "real_world" / "corpus.json"
DEFAULT_DIR = HERE.parent / "benchmark-feeds" / "real-world"
USER_AGENT = "gtfs.guru-real-world-corpus/1"
CHUNK = 1 << 20


# --------------------------------------------------------------------------- #
# manifest helpers
# --------------------------------------------------------------------------- #
def load_manifest(path: Path = CORPUS_JSON) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        sys.exit(f"corpus manifest not found: {path}")
    except json.JSONDecodeError as exc:
        sys.exit(f"corpus manifest is not valid JSON: {path}: {exc}")


def corpus_dir(explicit: str | None = None) -> Path:
    if explicit:
        return Path(explicit).expanduser()
    env = os.environ.get("GTFS_GURU_CORPUS_DIR")
    if env:
        return Path(env).expanduser()
    return DEFAULT_DIR


def select_feeds(manifest: dict, patterns: list[str] | None) -> list[dict]:
    feeds = manifest["feeds"]
    if not patterns:
        return feeds
    import fnmatch

    chosen = [
        feed
        for feed in feeds
        if any(
            fnmatch.fnmatch(feed["feed_id"], pat)
            or fnmatch.fnmatch(feed["dataset_id"], pat)
            for pat in patterns
        )
    ]
    if not chosen:
        sys.exit(f"no corpus feed matches {patterns}")
    return chosen


def dataset_url(manifest: dict, feed: dict) -> str:
    return manifest["dataset_url_template"].format(
        feed_id=feed["feed_id"], dataset_id=feed["dataset_id"]
    )


def latest_url(manifest: dict, feed_id: str) -> str:
    return manifest["latest_url_template"].format(feed_id=feed_id)


def feed_path(directory: Path, feed: dict) -> Path:
    return directory / f"{feed['dataset_id']}.zip"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(CHUNK), b""):
            digest.update(block)
    return digest.hexdigest()


# --------------------------------------------------------------------------- #
# network helpers
# --------------------------------------------------------------------------- #
def request(url: str, method: str = "GET", timeout: int = 120):
    req = urllib.request.Request(url, method=method, headers={"User-Agent": USER_AGENT})
    return urllib.request.urlopen(req, timeout=timeout)  # noqa: S310 - fixed https hosts


def head(url: str, timeout: int = 60) -> dict:
    """Return size/etag/last-modified for a URL without downloading its body."""
    with request(url, method="HEAD", timeout=timeout) as response:
        headers = response.headers
        return {
            "status": response.status,
            "bytes": int(headers.get("Content-Length") or 0),
            "etag": (headers.get("ETag") or "").strip('"'),
            "last_modified": headers.get("Last-Modified") or "",
        }


def download(url: str, target: Path, timeout: int = 900) -> str:
    """Stream a URL to ``target`` atomically and return the sha256 of the body."""
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(target.suffix + ".part")
    digest = hashlib.sha256()
    with request(url, timeout=timeout) as response, tmp.open("wb") as handle:
        while True:
            block = response.read(CHUNK)
            if not block:
                break
            handle.write(block)
            digest.update(block)
    tmp.replace(target)
    return digest.hexdigest()


def fetch_catalog(url: str, timeout: int = 180) -> dict[str, dict]:
    """Load the public MobilityData catalog CSV, keyed by feed id."""
    with request(url, timeout=timeout) as response:
        raw = response.read()
    text = raw.decode("utf-8-sig", errors="replace")
    return {row["id"]: row for row in csv.DictReader(io.StringIO(text)) if row.get("id")}


# --------------------------------------------------------------------------- #
# fetch / verify
# --------------------------------------------------------------------------- #
def state_of(directory: Path, feed: dict) -> tuple[str, str | None]:
    """Classify the on-disk copy of a feed: missing, mismatch or ok."""
    path = feed_path(directory, feed)
    if not path.exists():
        return "missing", None
    if path.stat().st_size != feed["zip_bytes"]:
        return "mismatch", None
    actual = sha256_file(path)
    if actual != feed["sha256"]:
        return "mismatch", actual
    return "ok", actual


def cmd_fetch(args: argparse.Namespace) -> int:
    manifest = load_manifest()
    feeds = select_feeds(manifest, args.feed)
    directory = corpus_dir(args.dir)
    directory.mkdir(parents=True, exist_ok=True)

    def work(feed: dict) -> tuple[dict, str, str]:
        state, actual = ("missing", None) if args.force else state_of(directory, feed)
        if state == "ok":
            return feed, "cached", ""
        path = feed_path(directory, feed)
        url = dataset_url(manifest, feed)
        try:
            got = download(url, path, timeout=args.timeout)
        except (urllib.error.URLError, OSError) as exc:
            return feed, "error", f"{type(exc).__name__}: {exc}"
        if got != feed["sha256"]:
            path.unlink(missing_ok=True)
            return feed, "error", (
                f"sha256 mismatch: manifest {feed['sha256']}, downloaded {got}. "
                "The pinned dataset should be immutable; re-pin deliberately with "
                "'resolve' if MobilityData replaced it."
            )
        note = "re-downloaded" if state == "mismatch" else "downloaded"
        return feed, note, ""

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(work, feeds))

    failed = 0
    for feed, state, detail in results:
        mib = feed["zip_bytes"] / (1 << 20)
        line = f"{state:<13} {feed['dataset_id']:<30} {mib:>7.2f} MiB"
        if state == "error":
            failed += 1
            print(f"{line}\n              {detail}", file=sys.stderr)
        else:
            print(line)
    total = sum(feed["zip_bytes"] for feed in feeds) / (1 << 20)
    print(f"\n{len(feeds)} feeds, {total:.1f} MiB, corpus dir {directory}")
    if failed:
        print(f"{failed} feed(s) failed", file=sys.stderr)
    return 1 if failed else 0


def cmd_verify(args: argparse.Namespace) -> int:
    manifest = load_manifest()
    feeds = select_feeds(manifest, args.feed)
    directory = corpus_dir(args.dir)
    bad = 0
    for feed in feeds:
        state, actual = state_of(directory, feed)
        if state == "ok":
            print(f"ok           {feed['dataset_id']}")
            continue
        bad += 1
        path = feed_path(directory, feed)
        if state == "missing":
            print(f"missing      {feed['dataset_id']} ({path})", file=sys.stderr)
        else:
            print(
                f"mismatch     {feed['dataset_id']} expected {feed['sha256']} "
                f"got {actual or 'size differs'}",
                file=sys.stderr,
            )
    if bad:
        print(
            f"\n{bad} of {len(feeds)} feeds unusable; run "
            "'scripts/real_world_corpus.py fetch' to repair",
            file=sys.stderr,
        )
    return 1 if bad else 0


# --------------------------------------------------------------------------- #
# update detection
# --------------------------------------------------------------------------- #
def cmd_check_updates(args: argparse.Namespace) -> int:
    """Compare each pinned dataset against the feed's current 'latest' alias.

    Both URLs live in the same bucket, so a HEAD on each is enough: identical
    ETags mean the pin is still the newest dataset. Nothing is downloaded.
    """
    manifest = load_manifest()
    feeds = select_feeds(manifest, args.feed)
    catalog: dict[str, dict] = {}
    if args.catalog:
        try:
            catalog = fetch_catalog(manifest["catalog_csv"])
        except (urllib.error.URLError, OSError) as exc:
            print(f"catalog unavailable ({exc}); continuing without it", file=sys.stderr)

    def work(feed: dict) -> dict:
        row: dict = {"feed_id": feed["feed_id"], "dataset_id": feed["dataset_id"]}
        try:
            pinned = head(dataset_url(manifest, feed), timeout=args.timeout)
            newest = head(latest_url(manifest, feed["feed_id"]), timeout=args.timeout)
        except (urllib.error.URLError, OSError) as exc:
            row["state"] = "error"
            row["detail"] = f"{type(exc).__name__}: {exc}"
            return row
        row["state"] = "current" if pinned["etag"] == newest["etag"] else "stale"
        row["pinned_bytes"] = pinned["bytes"]
        row["latest_bytes"] = newest["bytes"]
        row["latest_last_modified"] = newest["last_modified"]
        if row["state"] == "stale":
            row["suggested_dataset_id"] = resolve_dataset_id(
                manifest, feed["feed_id"], newest["last_modified"], args.timeout
            )
        entry = catalog.get(feed["feed_id"])
        if entry is not None:
            labels = [x for x in (entry.get("features") or "").split("|") if x]
            row["catalog_status"] = entry.get("status")
            row["catalog_features"] = labels
            row["catalog_redirect"] = entry.get("redirect.id") or ""
        return row

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        rows = list(pool.map(work, feeds))

    stale = [r for r in rows if r["state"] == "stale"]
    errors = [r for r in rows if r["state"] == "error"]
    for row in rows:
        if row["state"] == "current":
            print(f"current      {row['dataset_id']}")
        elif row["state"] == "stale":
            delta = row["latest_bytes"] - row["pinned_bytes"]
            print(
                f"stale        {row['dataset_id']} -> "
                f"{row.get('suggested_dataset_id') or 'unknown'} "
                f"({row['latest_bytes']} bytes, {delta:+d}) "
                f"published {row['latest_last_modified']}"
            )
        else:
            print(f"error        {row['dataset_id']} {row['detail']}", file=sys.stderr)
        for key, label in (("catalog_status", "status"), ("catalog_redirect", "redirect")):
            value = row.get(key)
            if value and value not in {"active"}:
                print(f"             catalog {label}: {value}")

    print(
        f"\n{len(rows) - len(stale) - len(errors)} current, {len(stale)} stale, "
        f"{len(errors)} unreachable"
    )
    if stale:
        print(
            "A stale pin is not a failure: the corpus is deliberately frozen so "
            "notice baselines stay comparable. Re-pin when you want the newer data, "
            "and expect the baseline to move in the same commit."
        )
    if args.json:
        Path(args.json).write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    if errors:
        return 1
    return 2 if (stale and args.fail_on_stale) else 0


def resolve_dataset_id(
    manifest: dict, feed_id: str, last_modified: str, timeout: int = 60
) -> str | None:
    """Find the immutable dataset_id behind a feed's 'latest' alias.

    Dataset ids are ``<feed_id>-<YYYYMMDDHHMM>`` stamped when the ingestion run
    started, which is within a couple of minutes of the object's mtime but not
    always equal to it, so probe outwards from the mtime.
    """
    if not last_modified:
        return None
    try:
        stamp = parsedate_to_datetime(last_modified).astimezone(timezone.utc)
    except (TypeError, ValueError):
        return None
    for offset in (0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5):
        candidate = f"{feed_id}-{(stamp + timedelta(minutes=offset)):%Y%m%d%H%M}"
        url = manifest["dataset_url_template"].format(
            feed_id=feed_id, dataset_id=candidate
        )
        try:
            if head(url, timeout=timeout)["status"] == 200:
                return candidate
        except urllib.error.HTTPError:
            continue
        except (urllib.error.URLError, OSError):
            return None
    return None


def cmd_resolve(args: argparse.Namespace) -> int:
    """Print manifest-ready fields for the newest dataset of each feed id."""
    manifest = load_manifest()
    directory = corpus_dir(args.dir)
    exit_code = 0
    for feed_id in args.feed_id:
        try:
            newest = head(latest_url(manifest, feed_id), timeout=args.timeout)
        except (urllib.error.URLError, OSError) as exc:
            print(f"{feed_id}: unreachable ({exc})", file=sys.stderr)
            exit_code = 1
            continue
        dataset_id = resolve_dataset_id(
            manifest, feed_id, newest["last_modified"], args.timeout
        )
        if not dataset_id:
            print(
                f"{feed_id}: could not resolve an immutable dataset id near "
                f"{newest['last_modified']}",
                file=sys.stderr,
            )
            exit_code = 1
            continue
        stamp = dataset_id.rsplit("-", 1)[1]
        entry = {
            "feed_id": feed_id,
            "dataset_id": dataset_id,
            "date": f"{stamp[0:4]}-{stamp[4:6]}-{stamp[6:8]}",
            "zip_bytes": newest["bytes"],
        }
        if args.download:
            feed = {"feed_id": feed_id, "dataset_id": dataset_id}
            path = feed_path(directory, feed)
            entry["sha256"] = download(dataset_url(manifest, feed), path, args.timeout)
            with zipfile.ZipFile(path) as zf:
                entry["unpacked_bytes"] = sum(i.file_size for i in zf.infolist())
        print(json.dumps(entry, indent=2))
    return exit_code


# --------------------------------------------------------------------------- #
# describe
# --------------------------------------------------------------------------- #
IN_SCOPE = {
    "Flexible services": [
        "Continuous Stops",
        "Booking Rules",
        "Fixed-Stops Demand Responsive Transit",
        "Zone-Based Demand Responsive Services",
        "Predefined Routes with Deviation",
    ],
    "Fares (v2)": [
        "Fare Products",
        "Fare Media",
        "Rider Categories",
        "Zone-Based Fares",
        "Time-Based Fares",
        "Route-Based Fares",
        "Fare Transfers",
        "Contactless EMV Support",
    ],
    "Pathways": [
        "Pathway Connections",
        "Pathway Details",
        "Pathway Signs",
        "Levels",
        "In-station Traversal Time",
    ],
}


def observed_features(feed_id: str) -> list[str]:
    """Features the committed baseline recorded for a feed, if there is one."""
    path = HERE / "real_world" / "baseline" / f"{feed_id}.json"
    if not path.exists():
        return []
    try:
        return json.loads(path.read_text(encoding="utf-8")).get("features", [])
    except json.JSONDecodeError:
        return []


def cmd_describe(args: argparse.Namespace) -> int:
    manifest = load_manifest()
    feeds = manifest["feeds"]
    coverage: dict[str, list[str]] = {}
    for feed in feeds:
        seen = observed_features(feed["feed_id"]) or feed.get("covers", [])
        for name in seen:
            coverage.setdefault(name, []).append(feed["feed_id"])

    if args.format == "json":
        print(json.dumps({"coverage": coverage, "feeds": feeds}, indent=2))
        return 0

    print("| Feed | Provider | Country | Unpacked | Subfeatures it is here for |")
    print("| --- | --- | --- | --- | --- |")
    for feed in sorted(feeds, key=lambda f: f["unpacked_bytes"]):
        mib = feed["unpacked_bytes"] / (1 << 20)
        covers = ", ".join(feed.get("covers", [])) or "-"
        print(
            f"| `{feed['feed_id']}` | {feed['provider']} | {feed['country_code']} "
            f"| {mib:,.2f} MiB | {covers} |"
        )
    missing: list[str] = []
    print("\n| Feature | Subfeature | Covered by |")
    print("| --- | --- | --- |")
    for group, names in IN_SCOPE.items():
        for name in names:
            holders = coverage.get(name, [])
            if not holders:
                missing.append(f"{group} / {name}")
            cell = ", ".join(f"`{h}`" for h in holders) or "**not covered**"
            print(f"| {group} | {name} | {cell} |")
    total = sum(len(v) for v in IN_SCOPE.values())
    print(f"\nIn-scope subfeature coverage: {total - len(missing)}/{total}")
    for item in missing:
        print(f"  uncovered: {item}")
    return 1 if (missing and args.strict) else 0


# --------------------------------------------------------------------------- #
def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    sub = parser.add_subparsers(dest="command", required=True)

    def add_common(sp, jobs_default=4):
        sp.add_argument(
            "--dir",
            help="corpus directory (default $GTFS_GURU_CORPUS_DIR or "
            "benchmark-feeds/real-world)",
        )
        sp.add_argument(
            "--feed",
            action="append",
            metavar="GLOB",
            help="limit to feed ids or dataset ids matching GLOB (repeatable)",
        )
        sp.add_argument("--jobs", type=int, default=jobs_default)

    fetch = sub.add_parser("fetch", help="download missing or mismatched feeds")
    add_common(fetch)
    fetch.add_argument("--force", action="store_true", help="re-download everything")
    fetch.add_argument("--timeout", type=int, default=900)
    fetch.set_defaults(func=cmd_fetch)

    verify = sub.add_parser("verify", help="hash on-disk feeds against the manifest")
    add_common(verify, jobs_default=1)
    verify.set_defaults(func=cmd_verify)

    updates = sub.add_parser(
        "check-updates", help="report feeds whose pinned dataset is no longer newest"
    )
    add_common(updates)
    updates.add_argument("--timeout", type=int, default=60)
    updates.add_argument(
        "--catalog",
        action="store_true",
        help="also read the MobilityData catalog for status and feature labels",
    )
    updates.add_argument("--json", metavar="PATH", help="write the findings as JSON")
    updates.add_argument(
        "--fail-on-stale",
        action="store_true",
        help="exit 2 when a newer dataset exists (for a scheduled reminder job)",
    )
    updates.set_defaults(func=cmd_check_updates)

    resolve = sub.add_parser(
        "resolve", help="print manifest fields for a feed's newest dataset"
    )
    resolve.add_argument("feed_id", nargs="+")
    resolve.add_argument("--dir")
    resolve.add_argument("--timeout", type=int, default=120)
    resolve.add_argument(
        "--download",
        action="store_true",
        help="also download it so sha256 and unpacked_bytes can be filled in",
    )
    resolve.set_defaults(func=cmd_resolve)

    describe = sub.add_parser("describe", help="print the coverage matrix")
    describe.add_argument("--format", choices=("md", "json"), default="md")
    describe.add_argument(
        "--strict", action="store_true", help="exit 1 if an in-scope subfeature is bare"
    )
    describe.set_defaults(func=cmd_describe)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
