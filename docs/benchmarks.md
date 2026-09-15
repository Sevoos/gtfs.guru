# Benchmarks

On the two large real-world feeds below, `gtfs.guru` is roughly **2.2–2.6× faster
than `gtfsvtor`** and **4.6–6.7× faster than the canonical Java validator**. On
small feeds, where JVM startup dominates instead of the validation work, the
gap against the canonical validator widens to a
[median of 88× and up to 196×](#small-feeds).

| Feed | Size | `gtfs.guru` | `gtfsvtor` 1.0.3 | canonical `gtfs-validator` 8.0.1 |
| :--- | ---: | ---: | ---: | ---: |
| MBTA Boston | 38 MB zip · 295 MB unpacked · 5.4M `stop_times.txt` rows | **2.32 s** (n=5) | 6.13 s (n=3) | 10.60 s (n=3) |
| OVapi NL 2026-06-09 | 198 MB zip · 1.27 GB unpacked · 16.0M `stop_times.txt` rows | **9.75 s** (n=5) | 21.66 s (n=3) | 65.18 s (n=3) |

!!! warning "What this does and does not measure"
    These are wall-clock times for each tool running its own full validation
    pipeline. Rule sets and report formats differ between the three validators,
    so this is **not** a per-rule apples-to-apples comparison.

## Small feeds

The table above is the case where the validation work dominates. On a small
feed nothing dominates but process startup, and that is where the gap is
widest: the canonical validator spends over a second on the JVM before it
reads a row, while `gtfs.guru` has already finished and written its report.

Measured across this repository's own corpus — every case in
`test-gtfs-feeds/` (295 feeds, median 1.6 KB, largest 440 KB), each run 3
times, median kept:

| | canonical `gtfs-validator` 8.0.1 | `gtfs.guru` |
| :--- | ---: | ---: |
| Median wall time | 1.274 s | **0.0158 s** |
| Range | 1.168–2.137 s | 0.0077–0.0646 s |

| Speed-up | min | p25 | median | p75 | p90 | max |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: |
| `gtfs.guru` vs canonical | 28× | 80× | **88×** | 149× | 158× | 196× |

126 of the 295 cases (43%) are at or above 100×. The widest is
`errors/stops/missing_stop_name` at 196× (1.625 s against 0.0083 s); the
narrowest is `real-world/boston_mbta_pathways` at 28×, the largest feed in the
corpus and the one where real work starts to outweigh startup.

This ratio says as much about the JVM as about either validator, which is
exactly the point for anyone validating a feed in a pre-commit hook, a
per-branch CI job, or an editor save action: the fixed cost is paid on every
run, and on a small feed it *is* the run.

## Reproducing the small-feed numbers

```bash
cargo build --release -p gtfs-guru
scripts/real_world_parity.py jar          # the pinned 8.0.1 baseline
scripts/benchmark_small_feeds.py --reps 3 --json /tmp/small-feeds.json
```

Both tools run their normal pipeline and write their normal report files;
stdout and stderr are discarded so terminal logging is not timed; and the Java
run passes `--skip_validator_update` so its online version check is not counted
as validation time. A case counts only when the Java baseline exits 0 —
`gtfs.guru`'s exit code is a severity gate, not a failure signal.

Numbers above were measured on a Linux x86-64 container with a warm page cache
and OpenJDK 21, so the absolute times differ from the Apple M3 Pro figures in
the large-feed table. The ratio is what travels between machines; the seconds
are not.

## Setup

Measured on an Apple M3 Pro with a warm page cache. Every tool validates the
feed end-to-end and writes its normal report files; stdout and stderr were
redirected to `/dev/null` so terminal progress logging does not dominate the
measurement.

| Tool | Version | Invocation |
| --- | --- | --- |
| `gtfs.guru` | built with `cargo build --release -p gtfs-guru` | `RAYON_NUM_THREADS=8`, `--threads 8`, `--skip_validator_update` |
| [`mecatran/gtfsvtor`](https://github.com/mecatran/gtfsvtor) | 1.0.3 | OpenJDK 21, `--numThreads 8`, `GTFSVTOR_OPTS=-Xmx6G` |
| [`MobilityData/gtfs-validator`](https://github.com/MobilityData/gtfs-validator) | 8.0.1 | OpenJDK 21, `--threads 8`, `--skip_validator_update`, `-Xmx6G` |

## Reproducing

```bash
curl -sL -o /tmp/mbta.zip https://cdn.mbta.com/MBTA_GTFS.zip
curl -L -o /tmp/NL-20260609.gtfs.zip https://gtfs.ovapi.nl/nl/NL-20260609.gtfs.zip
```

```bash
RAYON_NUM_THREADS=8 gtfs-guru \
  -i /tmp/NL-20260609.gtfs.zip \
  -o /tmp/gtfs-guru-nl \
  --skip_validator_update \
  --threads 8
```

```bash
java -Xmx6G -jar gtfs-validator-8.0.1-cli.jar \
  -i /tmp/NL-20260609.gtfs.zip \
  -o /tmp/gtfs-validator-nl \
  --skip_validator_update \
  --threads 8
```

```bash
GTFSVTOR_OPTS=-Xmx6G gtfsvtor \
  --numThreads 8 \
  --htmlOutput /tmp/gtfsvtor-nl.html \
  --jsonOutput /tmp/gtfsvtor-nl.json \
  /tmp/NL-20260609.gtfs.zip
```

Note that `--threads` is report metadata kept for Java compatibility; it is
`RAYON_NUM_THREADS` that sizes the thread pool. See
[Parallelism](usage.md#parallelism).
