#!/usr/bin/env python3
"""Real-world parity harness: gtfs.guru against MobilityData/gtfs-validator.

The golden suite pins byte-exact output on controlled fixtures. This harness
answers a different question: does a change alter what gtfs.guru reports on
real feeds from the ecosystem, and does it still agree with the canonical Java
validator there? It records, per feed, crash and parse-failure status, the full
notice fingerprint (code -> severity + total), error/warning/info totals, wall
time and peak RSS, then classifies every difference as approved or as a
regression.

Commands:
  jar              ensure the pinned Java baseline jar is present, or check for newer
  run              validate the corpus and write a results file
  gate             classify a results file against the committed baseline
  report           render a results file as markdown or JSON
  update-baseline  adopt a results file as the new committed baseline
  impact           summarise how the baseline moved since a git ref

Exit codes: 0 pass, 1 usage or infrastructure failure, 2 gate failure.
"""
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
DATA = HERE / "real_world"
CORPUS_JSON = DATA / "corpus.json"
GATE_JSON = DATA / "gate.json"
DELTAS_JSON = DATA / "expected_deltas.json"
BASELINE_DIR = DATA / "baseline"
USER_AGENT = "gtfs.guru-real-world-parity/1"

# Codes that mean the validator itself could not read the input, as opposed to
# the feed being invalid. Everything in system_errors.json counts too.
PARSE_FAILURE_CODES = {
    "csv_parsing_failed",
    "i_o_error",
    "runtime_exception_in_loader_error",
    "runtime_exception_in_validator_error",
    "thread_execution_error",
    "u_r_i_syntax_error",
    "fatal_internal_error",
}

CHECKS = ("crash_free", "parse_clean", "notices_stable", "features_stable", "java_parity")


# --------------------------------------------------------------------------- #
# configuration
# --------------------------------------------------------------------------- #
def load_json(path: Path, what: str) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        sys.exit(f"{what} not found: {path}")
    except json.JSONDecodeError as exc:
        sys.exit(f"{what} is not valid JSON: {path}: {exc}")


def corpus_feeds(patterns: list[str] | None) -> tuple[dict, list[dict]]:
    manifest = load_json(CORPUS_JSON, "corpus manifest")
    feeds = manifest["feeds"]
    if patterns:
        feeds = [
            feed
            for feed in feeds
            if any(fnmatch.fnmatch(feed["feed_id"], pat) for pat in patterns)
        ]
        if not feeds:
            sys.exit(f"no corpus feed matches {patterns}")
    return manifest, feeds


def corpus_dir(explicit: str | None = None) -> Path:
    if explicit:
        return Path(explicit).expanduser()
    env = os.environ.get("GTFS_GURU_CORPUS_DIR")
    if env:
        return Path(env).expanduser()
    return REPO / "benchmark-feeds" / "real-world"


def git_sha() -> str:
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        )
        return out.stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return "unknown"


# --------------------------------------------------------------------------- #
# measured process execution
# --------------------------------------------------------------------------- #
def peak_rss_mb(ru_maxrss: int) -> float:
    """ru_maxrss is kilobytes on Linux and bytes on macOS."""
    divisor = 1024 * 1024 if sys.platform == "darwin" else 1024
    return round(ru_maxrss / divisor, 1)


def run_measured(cmd: list[str], timeout: float, log_path: Path, env=None) -> dict:
    """Run a command, capturing wall time and peak RSS of that process alone.

    subprocess reaps children itself, so rusage would be lost; fork/exec plus
    os.wait4 keeps it. The child gets its own process group so a timeout kills
    any grandchildren (the JVM's helper threads included).
    """
    log_path.parent.mkdir(parents=True, exist_ok=True)
    environ = dict(os.environ if env is None else env)
    started = time.monotonic()
    with log_path.open("wb") as log:
        pid = os.fork()
        if pid == 0:  # child
            try:
                os.setsid()
                os.dup2(log.fileno(), 1)
                os.dup2(log.fileno(), 2)
                os.execvpe(cmd[0], cmd, environ)
            except BaseException:  # noqa: BLE001 - must not raise past exec
                os._exit(127)

    finished = threading.Event()
    timed_out = threading.Event()

    def watchdog() -> None:
        if not finished.wait(timeout):
            timed_out.set()
            try:
                os.killpg(pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass

    watcher = threading.Thread(target=watchdog, daemon=True)
    watcher.start()
    _, status, usage = os.wait4(pid, 0)
    finished.set()
    watcher.join(timeout=5)

    exit_code = os.waitstatus_to_exitcode(status)
    killed_by = -exit_code if exit_code < 0 else None
    return {
        "exit_code": exit_code if killed_by is None else None,
        "signal": killed_by,
        "timed_out": timed_out.is_set(),
        "crashed": timed_out.is_set() or killed_by is not None or exit_code != 0,
        "wall_seconds": round(time.monotonic() - started, 3),
        "peak_rss_mb": peak_rss_mb(usage.ru_maxrss),
        "log": str(log_path),
    }


# --------------------------------------------------------------------------- #
# report parsing
# --------------------------------------------------------------------------- #
def notice_rows(document: dict) -> list[dict]:
    rows = document.get("notices")
    return rows if isinstance(rows, list) else []


def read_outputs(out_dir: Path) -> dict:
    """Turn a validator output directory into comparable facts."""
    facts: dict = {
        "report_read": False,
        "errors": None,
        "warnings": None,
        "infos": None,
        "fingerprint": {},
        "features": [],
        "system_errors": None,
        "parse_failures": 0,
        "validation_seconds": None,
        "validator_version": None,
    }
    report = out_dir / "report.json"
    if not report.exists():
        facts["parse_failures"] += 1
        facts["detail"] = f"no report.json in {out_dir}"
        return facts
    try:
        document = json.loads(report.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        facts["parse_failures"] += 1
        facts["detail"] = f"report.json unreadable: {exc}"
        return facts

    facts["report_read"] = True
    summary = document.get("summary") or {}
    facts["features"] = sorted(summary.get("gtfsFeatures") or [])
    facts["validation_seconds"] = summary.get("validationTimeSeconds")
    facts["validator_version"] = summary.get("validatorVersion")

    totals = {"ERROR": 0, "WARNING": 0, "INFO": 0}
    fingerprint: dict[str, dict] = {}
    for row in notice_rows(document):
        code = row.get("code")
        severity = row.get("severity")
        count = int(row.get("totalNotices") or 0)
        if not code:
            continue
        fingerprint[code] = {"severity": severity, "total": count}
        if severity in totals:
            totals[severity] += count
        if code in PARSE_FAILURE_CODES:
            facts["parse_failures"] += count
    facts["fingerprint"] = dict(sorted(fingerprint.items()))
    facts["errors"] = totals["ERROR"]
    facts["warnings"] = totals["WARNING"]
    facts["infos"] = totals["INFO"]

    system_errors = out_dir / "system_errors.json"
    if system_errors.exists():
        try:
            sys_doc = json.loads(system_errors.read_text(encoding="utf-8"))
            count = sum(int(r.get("totalNotices") or 0) for r in notice_rows(sys_doc))
            facts["system_errors"] = count
            facts["system_error_codes"] = sorted(
                r["code"] for r in notice_rows(sys_doc) if r.get("code")
            )
            facts["parse_failures"] += count
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            facts["system_errors"] = None
            facts["detail"] = f"system_errors.json unreadable: {exc}"
            facts["parse_failures"] += 1
    return facts


# --------------------------------------------------------------------------- #
# jar management (1.4: latest-version detection and re-download conditions)
# --------------------------------------------------------------------------- #
def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def github_json(url: str, timeout: int = 60) -> dict:
    headers = {"User-Agent": USER_AGENT, "Accept": "application/vnd.github+json"}
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as response:  # noqa: S310
        return json.loads(response.read().decode("utf-8"))


def latest_release(repo: str, timeout: int = 60) -> dict | None:
    try:
        return github_json(
            f"https://api.github.com/repos/{repo}/releases/latest", timeout
        )
    except (urllib.error.URLError, OSError, json.JSONDecodeError) as exc:
        print(f"could not query the latest {repo} release: {exc}", file=sys.stderr)
        return None


def jar_candidates(pin: dict, explicit: str | None) -> list[Path]:
    if explicit:
        return [Path(explicit).expanduser()]
    env = os.environ.get("GTFS_VALIDATOR_JAR")
    paths = [Path(env).expanduser()] if env else []
    bench = REPO / "benchmark-feeds"
    paths.append(bench / pin["asset"])
    paths.append(bench / "gtfs-validator.jar")
    return paths


def ensure_jar(pin: dict, explicit: str | None, download: bool = True) -> Path:
    """Return a jar whose sha256 equals the pin, downloading it only if needed.

    A cached jar is reused whenever its hash matches, so repeated runs and CI
    cache hits do no network I/O at all. A hash mismatch is never silently
    accepted: the pin is what the committed baseline was produced with.
    """
    for path in jar_candidates(pin, explicit):
        if path.exists() and sha256_file(path) == pin["sha256"]:
            return path
    stale = [p for p in jar_candidates(pin, explicit) if p.exists()]
    for path in stale:
        print(
            f"ignoring {path}: sha256 does not match the pinned "
            f"{pin['release_tag']} asset",
            file=sys.stderr,
        )
    if not download:
        sys.exit(
            f"no jar matching {pin['asset']} ({pin['sha256'][:12]}...); run "
            "'scripts/real_world_parity.py jar' first"
        )
    target = (REPO / "benchmark-feeds" / pin["asset"])
    target.parent.mkdir(parents=True, exist_ok=True)
    url = (
        f"https://github.com/{pin['repo']}/releases/download/"
        f"{pin['release_tag']}/{pin['asset']}"
    )
    print(f"downloading {url}")
    tmp = target.with_suffix(target.suffix + ".part")
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=900) as response:  # noqa: S310
            with tmp.open("wb") as handle:
                shutil.copyfileobj(response, handle, 1 << 20)
    except (urllib.error.URLError, OSError) as exc:
        tmp.unlink(missing_ok=True)
        sys.exit(f"could not download the Java baseline: {exc}")
    got = sha256_file(tmp)
    if got != pin["sha256"]:
        tmp.unlink(missing_ok=True)
        sys.exit(f"jar sha256 mismatch: expected {pin['sha256']}, got {got}")
    tmp.replace(target)
    return target


def cmd_jar(args: argparse.Namespace) -> int:
    config = load_json(GATE_JSON, "gate config")
    pin = config["java_baseline"]
    if args.adopt_latest:
        release = latest_release(pin["repo"])
        if release is None:
            return 1
        tag = release["tag_name"]
        asset = next(
            (a for a in release.get("assets", []) if a["name"].endswith("-cli.jar")),
            None,
        )
        if asset is None:
            print(f"{tag} publishes no -cli.jar asset", file=sys.stderr)
            return 1
        target = REPO / "benchmark-feeds" / asset["name"]
        target.parent.mkdir(parents=True, exist_ok=True)
        req = urllib.request.Request(
            asset["browser_download_url"], headers={"User-Agent": USER_AGENT}
        )
        with urllib.request.urlopen(req, timeout=900) as response:  # noqa: S310
            with target.open("wb") as handle:
                shutil.copyfileobj(response, handle, 1 << 20)
        pin.update(
            {
                "release_tag": tag,
                "asset": asset["name"],
                "sha256": sha256_file(target),
                "asset_bytes": target.stat().st_size,
            }
        )
        GATE_JSON.write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
        print(
            f"pinned {pin['repo']} {tag} ({pin['asset']}, {pin['sha256']}).\n"
            "Re-run 'run' plus 'update-baseline' so the committed baseline matches "
            "the new upstream release, and describe the move in the release notes."
        )
        return 0

    path = ensure_jar(pin, args.jar, download=not args.check_only)
    if not args.check_only:
        print(f"jar ready: {path} ({pin['release_tag']}, {pin['sha256'][:12]}...)")
    release = latest_release(pin["repo"]) if args.check_latest else None
    if release is not None:
        tag = release["tag_name"]
        if tag == pin["release_tag"]:
            print(f"{pin['repo']} latest release is {tag}: pin is up to date")
        else:
            print(
                f"{pin['repo']} published {tag}; the corpus baseline is pinned to "
                f"{pin['release_tag']}. Adopt it deliberately with --adopt-latest."
            )
            if args.fail_on_newer:
                return 2
    return 0


# --------------------------------------------------------------------------- #
# run
# --------------------------------------------------------------------------- #
def guru_binary(explicit: str | None) -> list[str]:
    if explicit:
        return [explicit]
    env = os.environ.get("GTFS_VALIDATOR_BIN")
    if env:
        return [env]
    built = REPO / "target" / "release" / "gtfs-guru"
    if built.exists():
        return [str(built)]
    sys.exit(
        "no gtfs-guru binary found. Build it with "
        "'cargo build --release -p gtfs-guru' or set GTFS_VALIDATOR_BIN. "
        "Falling back to 'cargo run' would build the multi-gigabyte debug tree."
    )


def guru_command(base: list[str], feed: dict, zip_path: Path, out: Path, threads: int):
    return base + [
        "-i",
        str(zip_path),
        "-o",
        str(out),
        "--skip_validator_update",
        "--date",
        feed["date"],
        "--country_code",
        feed["country_code"],
        "--validated-at",
        f"{feed['date']}T12:00:00Z",
        "--threads",
        str(threads),
    ]


def resolve_java(explicit: str | None = None) -> str:
    """Find a java binary, including JDKs that are installed but not on PATH.

    A distribution JDK under /usr/lib/jvm is often absent from PATH. Without
    this, exec would simply fail and every feed would be reported as a crash
    with an empty log, which reads like a validator bug rather than a missing
    runtime.
    """
    candidates: list[Path] = []
    for source in (explicit, os.environ.get("JAVA_BIN")):
        if source:
            path = Path(source).expanduser()
            if not (path.is_file() and os.access(path, os.X_OK)):
                sys.exit(f"JAVA_BIN does not point at an executable: {path}")
            return str(path)
    found = shutil.which("java")
    if found:
        return found
    java_home = os.environ.get("JAVA_HOME")
    if java_home:
        candidates.append(Path(java_home) / "bin" / "java")
    for pattern in (
        "/usr/lib/jvm/*/bin/java",
        "/usr/java/*/bin/java",
        "/Library/Java/JavaVirtualMachines/*/Contents/Home/bin/java",
    ):
        candidates.extend(sorted(Path("/").glob(pattern.lstrip("/"))))
    for path in candidates:
        if path.is_file() and os.access(path, os.X_OK):
            print(f"using java at {path} (not on PATH)")
            return str(path)
    sys.exit(
        "no java runtime found. Install a JDK, or point JAVA_BIN at one "
        "(for example JAVA_BIN=/usr/lib/jvm/<jdk>/bin/java). Looked at PATH, "
        "JAVA_HOME, /usr/lib/jvm, /usr/java and /Library/Java."
    )


def java_runtime_version(java: str) -> str | None:
    """Record which JVM produced the numbers; timings depend on it."""
    try:
        out = subprocess.run(
            [java, "-version"], capture_output=True, text=True, timeout=60
        )
    except (OSError, subprocess.SubprocessError):
        return None
    first = (out.stderr or out.stdout).strip().splitlines()
    return first[0].strip() if first else None


def java_command(
    java: str,
    jar: Path,
    feed: dict,
    zip_path: Path,
    out: Path,
    threads: int,
    xmx: str,
):
    return [
        java,
        f"-Xmx{xmx}",
        "-jar",
        str(jar),
        "-i",
        str(zip_path),
        "-o",
        str(out),
        "--skip_validator_update",
        "--date",
        feed["date"],
        "--country_code",
        feed["country_code"],
        "--threads",
        str(threads),
    ]


def cmd_run(args: argparse.Namespace) -> int:
    manifest, feeds = corpus_feeds(args.feed)
    config = load_json(GATE_JSON, "gate config")
    pin = config["java_baseline"]
    tools = [t.strip() for t in args.tools.split(",") if t.strip()]
    unknown = set(tools) - {"guru", "java"}
    if unknown:
        sys.exit(f"unknown tool(s): {sorted(unknown)}")

    directory = corpus_dir(args.dir)
    out_root = Path(args.out_dir).expanduser()
    out_root.mkdir(parents=True, exist_ok=True)

    guru_base = guru_binary(args.bin) if "guru" in tools else None
    jar = ensure_jar(pin, args.jar) if "java" in tools else None
    java_bin = resolve_java(args.java_bin) if "java" in tools else None
    java_xmx = args.java_xmx or pin["java_xmx"]

    # Pin thread counts so wall time stays comparable between runs and the two
    # implementations do the same amount of work.
    env = dict(os.environ)
    env["RAYON_NUM_THREADS"] = str(args.threads)

    results = {
        "schema": 1,
        "git_sha": git_sha(),
        "tools": tools,
        "threads": args.threads,
        "host": {
            "platform": sys.platform,
            "cpu_count": os.cpu_count(),
        },
        "java_baseline": {
            "repo": pin["repo"],
            "release_tag": pin["release_tag"],
            "asset": pin["asset"],
            "sha256": pin["sha256"],
            "java_bin": java_bin,
            "java_xmx": java_xmx,
            # Wall time and RSS depend on the JVM, so record which one ran.
            "java_runtime": java_runtime_version(java_bin),
        }
        if jar
        else None,
        "feeds": [],
    }

    failures = 0
    for feed in feeds:
        zip_path = directory / f"{feed['dataset_id']}.zip"
        entry: dict = {
            "feed_id": feed["feed_id"],
            "dataset_id": feed["dataset_id"],
            "provider": feed.get("provider"),
            "country_code": feed["country_code"],
            "unpacked_bytes": feed.get("unpacked_bytes"),
            "covers": feed.get("covers", []),
        }
        if not zip_path.exists():
            entry["corpus_error"] = f"missing {zip_path}"
            print(f"MISSING  {feed['feed_id']}: {zip_path}", file=sys.stderr)
            failures += 1
            results["feeds"].append(entry)
            continue
        if not args.skip_hash_check:
            digest = sha256_file(zip_path)
            if digest != feed["sha256"]:
                entry["corpus_error"] = (
                    f"sha256 mismatch: expected {feed['sha256']}, got {digest}"
                )
                print(f"MISMATCH {feed['feed_id']}: {entry['corpus_error']}", file=sys.stderr)
                failures += 1
                results["feeds"].append(entry)
                continue

        for tool in tools:
            out = out_root / feed["feed_id"] / tool
            if out.exists():
                shutil.rmtree(out)
            out.mkdir(parents=True, exist_ok=True)
            log = out_root / feed["feed_id"] / f"{tool}.log"
            if tool == "guru":
                cmd = guru_command(guru_base, feed, zip_path, out, args.threads)
                timeout = config["timeouts_seconds"]["guru"]
            else:
                cmd = java_command(
                    java_bin, jar, feed, zip_path, out, args.threads, java_xmx
                )
                timeout = config["timeouts_seconds"]["java"]
            measured = run_measured(cmd, timeout, log, env=env)
            facts = read_outputs(out)
            entry[tool] = {**measured, **facts, "command": cmd}
            status = "crash" if measured["crashed"] else "ok"
            print(
                f"{tool:<5} {feed['feed_id']:<11} {status:<5} "
                f"{measured['wall_seconds']:>8.2f}s "
                f"{measured['peak_rss_mb']:>8.1f} MB  "
                f"E={facts['errors']} W={facts['warnings']} I={facts['infos']} "
                f"codes={len(facts['fingerprint'])}"
            )
            if measured["crashed"]:
                tail = Path(log).read_text(encoding="utf-8", errors="replace")[-800:]
                print(f"      log tail: {tail}", file=sys.stderr)
        results["feeds"].append(entry)

    out_file = Path(args.results).expanduser()
    out_file.parent.mkdir(parents=True, exist_ok=True)
    out_file.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print(f"\nwrote {out_file}")
    return 1 if failures else 0


# --------------------------------------------------------------------------- #
# gate
# --------------------------------------------------------------------------- #
def baseline_path(feed_id: str) -> Path:
    return BASELINE_DIR / f"{feed_id}.json"


def load_baseline(feed_id: str) -> dict | None:
    path = baseline_path(feed_id)
    if not path.exists():
        return None
    return load_json(path, f"baseline for {feed_id}")


def fingerprint_diff(old: dict, new: dict) -> list[dict]:
    """Per-code differences between two notice fingerprints."""
    diffs = []
    for code in sorted(set(old) | set(new)):
        before = old.get(code)
        after = new.get(code)
        if before == after:
            continue
        diffs.append(
            {
                "code": code,
                "severity": (after or before or {}).get("severity"),
                "before": None if before is None else before.get("total"),
                "after": None if after is None else after.get("total"),
            }
        )
    return diffs


def expected_total(value):
    """Approvals spell a missing code "absent"; JSON has no other way to say it."""
    return None if value == "absent" else value


def expected_delta_index(document: dict) -> dict[tuple[str, str], dict]:
    index = {}
    for entry in document.get("entries", []):
        index[(entry["feed_id"], entry["code"])] = entry
    return index


def compare_to_java(guru: dict, java: dict) -> list[dict]:
    """Code-level differences between the two validators on one feed."""
    return fingerprint_diff(java["fingerprint"], guru["fingerprint"])


def perf_verdict(measured: dict, baseline: dict | None, thresholds: dict) -> dict:
    verdict = {"wall_ok": True, "rss_ok": True}
    if not baseline:
        return verdict
    limit_wall = max(
        thresholds["wall_seconds_floor"],
        baseline.get("wall_seconds", 0) * thresholds["wall_seconds_factor"],
    )
    limit_rss = max(
        thresholds["peak_rss_floor_mb"],
        baseline.get("peak_rss_mb", 0) * thresholds["peak_rss_factor"],
    )
    verdict["wall_limit"] = round(limit_wall, 2)
    verdict["rss_limit"] = round(limit_rss, 1)
    verdict["wall_ok"] = measured["wall_seconds"] <= limit_wall
    verdict["rss_ok"] = measured["peak_rss_mb"] <= limit_rss
    return verdict


def evaluate(results: dict, config: dict, deltas: dict) -> dict:
    """Classify a run: per-feed checks, a score, and the reasons behind it."""
    weights = config["gate"]["weights"]
    thresholds = config["thresholds"]
    index = expected_delta_index(deltas)
    used_deltas: set[tuple[str, str]] = set()
    feeds_out = []
    earned = 0.0
    possible = 0.0

    for entry in results["feeds"]:
        feed_id = entry["feed_id"]
        verdict: dict = {
            "feed_id": feed_id,
            "dataset_id": entry.get("dataset_id"),
            "checks": {},
            "reasons": [],
            "warnings": [],
        }
        if entry.get("corpus_error"):
            verdict["checks"] = {"crash_free": False, "parse_clean": False}
            verdict["passed"] = False
            verdict["reasons"].append(f"corpus unusable: {entry['corpus_error']}")
            possible += weights["crash_free"] + weights["parse_clean"]
            feeds_out.append(verdict)
            continue

        guru = entry.get("guru")
        if guru is None:
            verdict["warnings"].append("gtfs.guru did not run for this feed")
            feeds_out.append(verdict)
            continue

        baseline = load_baseline(feed_id)
        checks = verdict["checks"]

        checks["crash_free"] = not guru["crashed"]
        if guru["crashed"]:
            how = (
                "timed out"
                if guru["timed_out"]
                else f"killed by signal {guru['signal']}"
                if guru["signal"]
                else f"exit code {guru['exit_code']}"
            )
            verdict["reasons"].append(f"gtfs.guru {how}")

        checks["parse_clean"] = (guru["parse_failures"] or 0) == 0
        if not checks["parse_clean"]:
            codes = guru.get("system_error_codes") or []
            verdict["reasons"].append(
                f"{guru['parse_failures']} parse failure(s)"
                + (f" ({', '.join(codes)})" if codes else "")
                + (f": {guru['detail']}" if guru.get("detail") else "")
            )

        if baseline is None:
            verdict["warnings"].append(
                "no committed baseline; run update-baseline to adopt this result"
            )
        else:
            diffs = fingerprint_diff(baseline.get("notices", {}), guru["fingerprint"])
            checks["notices_stable"] = not diffs
            verdict["notice_diff"] = diffs
            if diffs:
                verdict["reasons"].append(
                    f"{len(diffs)} notice code(s) changed against the committed baseline"
                )
            feature_diff = sorted(
                set(baseline.get("features", [])) ^ set(guru["features"])
            )
            checks["features_stable"] = not feature_diff
            if feature_diff:
                verdict["feature_diff"] = feature_diff
                verdict["reasons"].append(
                    f"detected features changed: {', '.join(feature_diff)}"
                )
            verdict["perf"] = perf_verdict(guru, baseline.get("perf"), thresholds)
            verdict["perf_ok"] = (
                verdict["perf"]["wall_ok"] and verdict["perf"]["rss_ok"]
            )
            if not verdict["perf"]["wall_ok"]:
                verdict["warnings"].append(
                    f"wall time {guru['wall_seconds']}s exceeds "
                    f"{verdict['perf']['wall_limit']}s"
                )
            if not verdict["perf"]["rss_ok"]:
                verdict["warnings"].append(
                    f"peak RSS {guru['peak_rss_mb']} MB exceeds "
                    f"{verdict['perf']['rss_limit']} MB"
                )

        java = entry.get("java")
        if java is not None:
            if java["crashed"] or not java["report_read"]:
                verdict["warnings"].append(
                    "Java baseline did not produce a report; parity not assessed"
                )
            else:
                java_diffs = compare_to_java(guru, java)
                unexplained = []
                for diff in java_diffs:
                    approved = index.get((feed_id, diff["code"]))
                    if approved is None:
                        unexplained.append(diff)
                        continue
                    used_deltas.add((feed_id, diff["code"]))
                    # An approval pins the size of the difference: a total that
                    # moves is a new difference wearing an old approval. Omit a
                    # key to accept any value; use "absent" for "code not
                    # emitted at all".
                    mismatch = any(
                        key in approved
                        and expected_total(approved[key]) != diff[side]
                        for key, side in (("guru_total", "after"), ("java_total", "before"))
                    )
                    if mismatch:
                        diff["approved_as"] = {
                            key: approved.get(key)
                            for key in ("guru_total", "java_total")
                            if key in approved
                        }
                        unexplained.append(diff)
                verdict["java_diff"] = java_diffs
                verdict["java_diff_unexplained"] = unexplained
                checks["java_parity"] = not unexplained
                if unexplained:
                    tag = (results.get("java_baseline") or {}).get(
                        "release_tag", "the Java baseline"
                    )
                    verdict["reasons"].append(
                        f"{len(unexplained)} unapproved difference(s) against {tag}"
                    )

        for name, passed in checks.items():
            possible += weights[name]
            if passed:
                earned += weights[name]
        verdict["passed"] = all(checks.values()) if checks else True
        feeds_out.append(verdict)

    stale = [
        {"feed_id": feed, "code": code}
        for (feed, code) in sorted(index)
        if (feed, code) not in used_deltas
        and any(f["feed_id"] == feed and "java_diff" in f for f in feeds_out)
    ]
    score = 100.0 if possible == 0 else round(100.0 * earned / possible, 2)
    return {
        "score": score,
        "min_score": config["gate"]["min_score"],
        "passed": score >= config["gate"]["min_score"],
        "feeds": feeds_out,
        "stale_expected_deltas": stale,
        "checks_applied": sorted(
            {name for f in feeds_out for name in f.get("checks", {})}
        ),
    }


def cmd_gate(args: argparse.Namespace) -> int:
    results = load_json(Path(args.results), "results file")
    config = load_json(GATE_JSON, "gate config")
    deltas = load_json(DELTAS_JSON, "expected deltas")
    if args.min_score is not None:
        config["gate"]["min_score"] = args.min_score
    report = evaluate(results, config, deltas)

    for feed in report["feeds"]:
        checks = feed.get("checks", {})
        if not checks:
            state = "skipped"
        else:
            state = "pass" if feed["passed"] else "FAIL"
        print(f"{state:<8} {feed['feed_id']}")
        for reason in feed["reasons"]:
            print(f"         - {reason}")
        for diff in feed.get("notice_diff", [])[: args.max_diff]:
            print(
                f"           baseline {diff['code']}: "
                f"{diff['before']} -> {diff['after']} ({diff['severity']})"
            )
        for diff in feed.get("java_diff_unexplained", [])[: args.max_diff]:
            print(
                f"           vs java {diff['code']}: java {diff['before']}, "
                f"guru {diff['after']} ({diff['severity']})"
            )
        for warning in feed["warnings"]:
            print(f"         ! {warning}")

    for entry in report["stale_expected_deltas"]:
        print(
            f"!        stale expected delta {entry['feed_id']}/{entry['code']} no "
            "longer matches a real difference; remove it from expected_deltas.json"
        )

    print(
        f"\nparity regression gate: {report['score']:.2f}/100 "
        f"(minimum {report['min_score']}), checks: "
        f"{', '.join(report['checks_applied']) or 'none'}"
    )
    perf_warnings = sum(1 for f in report["feeds"] if f.get("perf_ok") is False)
    failed_perf = args.strict_perf and perf_warnings
    if perf_warnings:
        print(
            f"{perf_warnings} performance threshold warning(s)"
            + (" (fatal: --strict-perf)" if args.strict_perf else " (advisory)")
        )
    if args.json:
        Path(args.json).write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if args.summary:
        write_markdown(results, report, Path(args.summary))
    if not report["passed"] or failed_perf:
        return 2
    return 0


# --------------------------------------------------------------------------- #
# report
# --------------------------------------------------------------------------- #
def fmt(value, suffix="", nd=2):
    if value is None:
        return "-"
    if isinstance(value, float):
        return f"{value:,.{nd}f}{suffix}"
    return f"{value:,}{suffix}"


def signed(value):
    if value is None:
        return "-"
    return f"{value:+,}"


def markdown_report(results: dict, report: dict | None) -> str:
    lines: list[str] = []
    tools = results.get("tools", [])
    has_java = "java" in tools
    lines.append("## Real-world parity")
    lines.append("")
    java = results.get("java_baseline")
    lines.append(
        f"Commit `{results.get('git_sha', 'unknown')[:12]}`, "
        f"{len(results['feeds'])} pinned feeds, {results.get('threads')} thread(s), "
        f"{results['host'].get('cpu_count')} CPUs"
        + (
            f", baseline {java['repo']} {java['release_tag']}"
            if has_java and java
            else ", gtfs.guru only (no Java baseline in this run)"
        )
    )
    lines.append("")
    if report is not None:
        lines.append(
            f"**Parity regression gate: {report['score']:.2f}/100** "
            f"(minimum {report['min_score']}) - "
            f"{'PASS' if report['passed'] else 'FAIL'}"
        )
        lines.append("")

    header = ["Feed", "Unpacked", "Seconds", "Peak RSS", "Errors", "Warnings", "Codes"]
    if has_java:
        header = [
            "Feed",
            "Unpacked",
            "Seconds (guru/java)",
            "Peak RSS (guru/java)",
            "Errors (guru/java)",
            "Warnings (guru/java)",
            "Code diff",
        ]
    lines.append("| " + " | ".join(header) + " |")
    lines.append("| " + " | ".join(["---"] * len(header)) + " |")

    verdicts = {f["feed_id"]: f for f in (report or {}).get("feeds", [])}
    for entry in results["feeds"]:
        feed_id = entry["feed_id"]
        mib = (entry.get("unpacked_bytes") or 0) / (1 << 20)
        if entry.get("corpus_error"):
            lines.append(
                f"| `{feed_id}` | {mib:,.2f} MiB | corpus error: "
                f"{entry['corpus_error']} | | | | |"
            )
            continue
        guru = entry.get("guru") or {}
        row = [f"`{feed_id}`", f"{mib:,.2f} MiB"]
        if has_java:
            java_run = entry.get("java") or {}
            diff_count = len(verdicts.get(feed_id, {}).get("java_diff", []) or [])
            unexplained = len(
                verdicts.get(feed_id, {}).get("java_diff_unexplained", []) or []
            )
            row += [
                f"{fmt(guru.get('wall_seconds'))} / {fmt(java_run.get('wall_seconds'))}",
                f"{fmt(guru.get('peak_rss_mb'), nd=0)} / "
                f"{fmt(java_run.get('peak_rss_mb'), nd=0)}",
                f"{fmt(guru.get('errors'))} / {fmt(java_run.get('errors'))}",
                f"{fmt(guru.get('warnings'))} / {fmt(java_run.get('warnings'))}",
                f"{diff_count} ({unexplained} unapproved)"
                if diff_count
                else "identical",
            ]
        else:
            row += [
                fmt(guru.get("wall_seconds")),
                fmt(guru.get("peak_rss_mb"), nd=0),
                fmt(guru.get("errors")),
                fmt(guru.get("warnings")),
                fmt(len(guru.get("fingerprint") or {})),
            ]
        lines.append("| " + " | ".join(row) + " |")

    if report is not None:
        problems = [f for f in report["feeds"] if f.get("checks") and not f["passed"]]
        if problems:
            lines.append("")
            lines.append("### Regressions")
            for feed in problems:
                lines.append(f"- **`{feed['feed_id']}`**")
                for reason in feed["reasons"]:
                    lines.append(f"  - {reason}")
                for diff in feed.get("notice_diff", [])[:20]:
                    lines.append(
                        f"    - baseline `{diff['code']}` "
                        f"{diff['before']} -> {diff['after']}"
                    )
                for diff in feed.get("java_diff_unexplained", [])[:20]:
                    lines.append(
                        f"    - vs Java `{diff['code']}`: java {diff['before']}, "
                        f"guru {diff['after']}"
                    )
        notes = [
            (feed["feed_id"], warning)
            for feed in report["feeds"]
            for warning in feed["warnings"]
        ]
        if notes:
            lines.append("")
            lines.append("### Notes")
            for feed_id, warning in notes:
                lines.append(f"- `{feed_id}`: {warning}")
        if report["stale_expected_deltas"]:
            lines.append("")
            lines.append("### Stale approvals")
            for entry in report["stale_expected_deltas"]:
                lines.append(
                    f"- `{entry['feed_id']}` / `{entry['code']}` no longer differs; "
                    "drop it from `expected_deltas.json`"
                )
    lines.append("")
    return "\n".join(lines)


def write_markdown(results: dict, report: dict | None, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(markdown_report(results, report), encoding="utf-8")


def cmd_report(args: argparse.Namespace) -> int:
    results = load_json(Path(args.results), "results file")
    report = None
    if not args.no_gate:
        config = load_json(GATE_JSON, "gate config")
        deltas = load_json(DELTAS_JSON, "expected deltas")
        report = evaluate(results, config, deltas)
    if args.format == "json":
        payload = {"results": results, "gate": report}
        text = json.dumps(payload, indent=2) + "\n"
    else:
        text = markdown_report(results, report)
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(text, end="")
    return 0


# --------------------------------------------------------------------------- #
# update-baseline
# --------------------------------------------------------------------------- #
def cmd_update_baseline(args: argparse.Namespace) -> int:
    results = load_json(Path(args.results), "results file")
    if "guru" not in results.get("tools", []):
        sys.exit("that results file has no gtfs.guru run to adopt")
    BASELINE_DIR.mkdir(parents=True, exist_ok=True)
    written = 0
    for entry in results["feeds"]:
        feed_id = entry["feed_id"]
        if args.feed and not any(fnmatch.fnmatch(feed_id, p) for p in args.feed):
            continue
        guru = entry.get("guru")
        if guru is None or entry.get("corpus_error"):
            print(f"skipping {feed_id}: no usable gtfs.guru result", file=sys.stderr)
            continue
        if guru["crashed"] or not guru["report_read"]:
            print(
                f"refusing to baseline {feed_id}: the run crashed or produced no "
                "report",
                file=sys.stderr,
            )
            continue
        document = {
            "feed_id": feed_id,
            "dataset_id": entry["dataset_id"],
            "counts": {
                "errors": guru["errors"],
                "warnings": guru["warnings"],
                "infos": guru["infos"],
            },
            "features": guru["features"],
            "notices": guru["fingerprint"],
            "perf": {
                "wall_seconds": guru["wall_seconds"],
                "peak_rss_mb": guru["peak_rss_mb"],
                "threads": results.get("threads"),
                "note": "Recorded on one machine; thresholds in gate.json are "
                "deliberately loose because CI hardware varies.",
            },
            "last_change": {
                "reason": args.reason,
                "git_sha": results.get("git_sha"),
                "validator_version": guru.get("validator_version"),
                "host_platform": results["host"].get("platform"),
            },
        }
        baseline_path(feed_id).write_text(
            json.dumps(document, indent=2) + "\n", encoding="utf-8"
        )
        written += 1
        print(f"baselined {feed_id}")
    print(
        f"\n{written} baseline file(s) written. Commit them together with the change "
        "that moved them so the diff is reviewable."
    )
    return 0


# --------------------------------------------------------------------------- #
# impact (2.5: the pre-release report)
# --------------------------------------------------------------------------- #
def git_show(ref: str, rel_path: str) -> dict | None:
    try:
        out = subprocess.run(
            ["git", "-C", str(REPO), "show", f"{ref}:{rel_path}"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (subprocess.CalledProcessError, OSError):
        return None
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return None


def previous_tag() -> str | None:
    for cmd in (
        ["git", "-C", str(REPO), "describe", "--tags", "--abbrev=0", "HEAD^"],
        ["git", "-C", str(REPO), "describe", "--tags", "--abbrev=0"],
    ):
        try:
            out = subprocess.run(cmd, capture_output=True, text=True, check=True)
            tag = out.stdout.strip()
            if tag:
                return tag
        except (subprocess.CalledProcessError, OSError):
            continue
    return None


def cmd_impact(args: argparse.Namespace) -> int:
    """Diff the committed baselines between a git ref and the working tree.

    Because the baselines are committed, this needs no stored artefacts and no
    network: the release impact of a change is literally the diff of what the
    corpus reports.
    """
    ref = args.since or previous_tag()
    if not ref:
        sys.exit("no git ref to compare against; pass --since <tag>")
    manifest = load_json(CORPUS_JSON, "corpus manifest")
    old_manifest = git_show(ref, "scripts/real_world/corpus.json")
    config = load_json(GATE_JSON, "gate config")
    old_config = git_show(ref, "scripts/real_world/gate.json")

    lines = [f"## Real-world validation impact since `{ref}`", ""]

    old_pins = {f["feed_id"]: f for f in (old_manifest or {}).get("feeds", [])}
    new_pins = {f["feed_id"]: f for f in manifest["feeds"]}
    added = sorted(set(new_pins) - set(old_pins))
    removed = sorted(set(old_pins) - set(new_pins))
    repinned = sorted(
        fid
        for fid in set(new_pins) & set(old_pins)
        if new_pins[fid]["dataset_id"] != old_pins[fid]["dataset_id"]
    )
    if old_manifest is None:
        lines.append(f"- Corpus manifest did not exist at `{ref}`.")
    if added:
        lines.append(f"- Feeds added: {', '.join(f'`{f}`' for f in added)}")
    if removed:
        lines.append(f"- Feeds removed: {', '.join(f'`{f}`' for f in removed)}")
    for fid in repinned:
        lines.append(
            f"- `{fid}` re-pinned {old_pins[fid]['dataset_id']} -> "
            f"{new_pins[fid]['dataset_id']}"
        )
    old_tag = (old_config or {}).get("java_baseline", {}).get("release_tag")
    new_tag = config["java_baseline"]["release_tag"]
    if old_tag and old_tag != new_tag:
        lines.append(f"- Java baseline moved {old_tag} -> {new_tag}")

    changed_rows = []
    unchanged = 0
    for feed_id in sorted(new_pins):
        rel = f"scripts/real_world/baseline/{feed_id}.json"
        old = git_show(ref, rel)
        new = load_baseline(feed_id)
        if new is None:
            changed_rows.append((feed_id, "no baseline committed", [], None))
            continue
        if old is None:
            changed_rows.append(
                (
                    feed_id,
                    "new baseline",
                    fingerprint_diff({}, new.get("notices", {})),
                    new,
                )
            )
            continue
        diffs = fingerprint_diff(old.get("notices", {}), new.get("notices", {}))
        feature_diff = sorted(
            set(old.get("features", [])) ^ set(new.get("features", []))
        )
        if not diffs and not feature_diff:
            unchanged += 1
            continue
        changed_rows.append((feed_id, "changed", diffs, new))

    lines.append("")
    if not changed_rows:
        lines.append(
            f"No change: all {unchanged} pinned feeds report exactly what they "
            f"reported at `{ref}`."
        )
        lines.append("")
    else:
        lines.append(
            f"{len(changed_rows)} of {len(new_pins)} feeds changed; "
            f"{unchanged} identical."
        )
        lines.append("")
        lines.append("| Feed | Covers | Errors | Warnings | Notice codes changed |")
        lines.append("| --- | --- | --- | --- | --- |")
        for feed_id, state, diffs, new in changed_rows:
            if new is None:
                lines.append(f"| `{feed_id}` | | | | {state} |")
                continue
            old = git_show(ref, f"scripts/real_world/baseline/{feed_id}.json") or {}
            old_counts = old.get("counts", {})
            new_counts = new.get("counts", {})

            def delta(key):
                before, after = old_counts.get(key), new_counts.get(key)
                if before is None or after is None:
                    return fmt(after)
                if before == after:
                    return fmt(after)
                return f"{fmt(after)} ({signed(after - before)})"

            covers = ", ".join(new_pins[feed_id].get("covers", [])[:3]) or "-"
            lines.append(
                f"| `{feed_id}` | {covers} | {delta('errors')} | "
                f"{delta('warnings')} | {len(diffs)} |"
            )
        lines.append("")
        lines.append("### Per-code detail")
        for feed_id, state, diffs, new in changed_rows:
            if not diffs:
                continue
            lines.append(f"- **`{feed_id}`** ({state})")
            for diff in diffs[: args.max_codes]:
                before = "absent" if diff["before"] is None else f"{diff['before']:,}"
                after = "absent" if diff["after"] is None else f"{diff['after']:,}"
                lines.append(
                    f"  - `{diff['code']}` ({diff['severity']}): {before} -> {after}"
                )
            if len(diffs) > args.max_codes:
                lines.append(f"  - ... and {len(diffs) - args.max_codes} more")
            reason = (new.get("last_change") or {}).get("reason")
            if reason:
                lines.append(f"  - recorded reason: {reason}")
        lines.append("")

    text = "\n".join(lines)
    if args.out:
        Path(args.out).write_text(text, encoding="utf-8")
        print(f"wrote {args.out}")
    else:
        print(text, end="")
    return 0


# --------------------------------------------------------------------------- #
def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    sub = parser.add_subparsers(dest="command", required=True)

    jar = sub.add_parser("jar", help="ensure or check the pinned Java baseline")
    jar.add_argument("--jar", help="use this jar instead of the search path")
    jar.add_argument(
        "--check-only", action="store_true", help="do not download, only look"
    )
    jar.add_argument(
        "--check-latest",
        action="store_true",
        help="report whether upstream published a newer release",
    )
    jar.add_argument(
        "--fail-on-newer",
        action="store_true",
        help="exit 2 when a newer release exists (for a scheduled reminder job)",
    )
    jar.add_argument(
        "--adopt-latest",
        action="store_true",
        help="re-pin gate.json to the newest upstream release",
    )
    jar.set_defaults(func=cmd_jar)

    run = sub.add_parser("run", help="validate the corpus and write results")
    run.add_argument("--tools", default="guru", help="guru, java, or guru,java")
    run.add_argument("--bin", help="gtfs-guru binary (default $GTFS_VALIDATOR_BIN)")
    run.add_argument("--jar", help="Java baseline jar")
    run.add_argument(
        "--java-bin",
        help="java executable (default $JAVA_BIN, then PATH, then $JAVA_HOME "
        "and the usual JDK install locations)",
    )
    run.add_argument("--dir", help="corpus directory")
    run.add_argument("--feed", action="append", metavar="GLOB")
    run.add_argument("--threads", type=int, default=1)
    run.add_argument(
        "--java-xmx",
        help="override the Java heap from gate.json (for example 4g on a small host)",
    )
    run.add_argument(
        "--out-dir",
        default="real_world_actual",
        help="where validator output directories go (gitignored)",
    )
    run.add_argument(
        "--results",
        default="real_world_actual/results.json",
        help="results file to write",
    )
    run.add_argument(
        "--skip-hash-check",
        action="store_true",
        help="trust the corpus directory without re-hashing it",
    )
    run.set_defaults(func=cmd_run)

    gate = sub.add_parser("gate", help="classify a results file")
    gate.add_argument("results")
    gate.add_argument("--min-score", type=float)
    gate.add_argument(
        "--strict-perf",
        action="store_true",
        help="treat a time or memory threshold breach as a failure",
    )
    gate.add_argument("--json", help="write the verdict as JSON")
    gate.add_argument("--summary", help="write a markdown summary here")
    gate.add_argument("--max-diff", type=int, default=15)
    gate.set_defaults(func=cmd_gate)

    report = sub.add_parser("report", help="render a results file")
    report.add_argument("results")
    report.add_argument("--format", choices=("md", "json"), default="md")
    report.add_argument("--out")
    report.add_argument(
        "--no-gate", action="store_true", help="numbers only, no verdict"
    )
    report.set_defaults(func=cmd_report)

    update = sub.add_parser("update-baseline", help="adopt results as the baseline")
    update.add_argument("results")
    update.add_argument(
        "--reason",
        required=True,
        help="why the baseline moved; stored in the file and shown in reviews",
    )
    update.add_argument("--feed", action="append", metavar="GLOB")
    update.set_defaults(func=cmd_update_baseline)

    impact = sub.add_parser("impact", help="baseline movement since a git ref")
    impact.add_argument("--since", help="git ref (default: previous tag)")
    impact.add_argument("--out")
    impact.add_argument("--max-codes", type=int, default=25)
    impact.set_defaults(func=cmd_impact)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
