# Benchmarks

On the two large real-world feeds below, `gtfs.guru` is roughly **2.2–2.6× faster
than `gtfsvtor`** and **4.6–6.7× faster than the canonical Java validator**.

| Feed | Size | `gtfs.guru` | `gtfsvtor` 1.0.3 | canonical `gtfs-validator` 8.0.1 |
| :--- | ---: | ---: | ---: | ---: |
| MBTA Boston | 38 MB zip · 295 MB unpacked · 5.4M `stop_times.txt` rows | **2.32 s** (n=5) | 6.13 s (n=3) | 10.60 s (n=3) |
| OVapi NL 2026-06-09 | 198 MB zip · 1.27 GB unpacked · 16.0M `stop_times.txt` rows | **9.75 s** (n=5) | 21.66 s (n=3) | 65.18 s (n=3) |

!!! warning "What this does and does not measure"
    These are wall-clock times for each tool running its own full validation
    pipeline. Rule sets and report formats differ between the three validators,
    so this is **not** a per-rule apples-to-apples comparison.

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
