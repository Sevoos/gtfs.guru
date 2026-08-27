# Usage

## Command Line Interface (CLI)

### Basic Usage

```bash
# Validate local file

gtfs-guru --input /path/to/gtfs.zip --output_base ./report

# Validate from URL
gtfs-guru --url https://example.com/gtfs.zip --output_base ./report
```

Add `--storage_directory /tmp/gtfs` to keep a downloaded feed around instead of
re-fetching it.

### Output files

Written into `--output_base`:

| File | Written |
| --- | --- |
| `report.json` | always (rename with `--validation_report_name`) |
| `report.html` | always (rename with `--html_report_name`) |
| `system_errors.json` | always (rename with `--system_errors_report_name`) |
| the name given to `--sarif` | with `--sarif` |
| `notice_schema.json` | with `--export_notices_schema` |

`--sarif` takes a file name resolved inside `--output_base`. Badges are the
exception: they go to the exact path you name — see
[Status badges](#status-badges).

With `--stdout`, the JSON report goes to standard output and no report files are
written.

### CLI Options

| Option | Short | Description |
|--------|-------|-------------|
| `--input <PATH>` | `-i` | Path to GTFS zip file or directory |
| `--url <URL>` | `-u` | URL to download GTFS feed |
| `--output_base <DIR>` | `-o` | Output directory for reports (required unless `--stdout`) |
| `--stdout` | | Write only the JSON validation report to stdout |
| `--country_code <CODE>` | `-c` | ISO country code (e.g., US, RU, DE) |
| `--date <DATE>` | `-d` | Validation date (YYYY-MM-DD) |
| `--pretty` | `-p` | Format JSON output |
| `--export_notices_schema` | `-n` | Export notice schema to JSON |
| `--storage_directory <DIR>` | `-s` | Save downloaded feed to directory |
| `--validation_report_name <NAME>` | `-v` | Custom name for JSON report |
| `--html_report_name <NAME>` | `-r` | Custom name for HTML report |
| `--system_errors_report_name <NAME>` | `-e` | Custom name for system errors report |
| `--skip_validator_update` | | Skip validator update check |
| `--validated-at <TIMESTAMP>` | | Override `validated_at` in report metadata |
| `--threads <N>` | | Number recorded in report metadata; does not size the thread pool |
| `--google_rules` | | Enable Google-specific rules |
| `--sarif <NAME>` | | Write a SARIF report for CI/CD into `--output_base` |
| `--fail-on <LEVEL>` | | `none` (default), `error`, or `warning`; exit 2 at that severity |
| `--badge <PATH>` | | Write a shields.io endpoint descriptor for a README badge |
| `--badge-svg <PATH>` | | Write a self-contained SVG badge |
| `--badge-label <TEXT>` | | Left-hand badge text (default `GTFS`) |
| `--fix-dry-run` | | List suggested auto-fixes without modifying files |
| `--fix` | | Write a repaired copy with the safe fixes applied |
| `--fix-unsafe` | | Like `--fix`, but also applies confirm-level and unsafe fixes |
| `--fix-output <PATH>` | | Destination for the repaired feed (default `<input>.fixed.<ext>`) |
| `--thorough` | | Enable thorough validation (recommended fields) |
| `--timing` | | Print timing breakdown |
| `--timing-json` | | Print timing report as JSON |
| `--version` | | Print the validator version |

`--fix` writes a new feed and never changes the input. It safely normalizes
supported field values, trims declared GTFS fields, and sorts `stop_times.txt`
by trip and `stop_sequence`. `--fix-unsafe` can additionally delete rows whose
foreign key references a missing parent. After writing the copy, the CLI
validates it again and reports resolved, remaining, and introduced notices.

Ambiguous values are deliberately left alone — `01-05-2026` (day-first or
month-first?) and `1,500` (1.5 or 1500?) get no suggestion at all. See
[Fixes](llm.md#fixes) for the notice-by-notice repair table and its safety
levels.

### Subcommands

#### `diff` — compare two feed versions

```bash
gtfs-guru diff old.zip new.zip
gtfs-guru diff old.zip new.zip --json diff.json --markdown diff.md --fail-on-new-errors
```

The diff covers agencies, routes, stops, route-level trip and frequency
aggregates, and validation notice deltas. Under `--fail-on-new-errors` it exits
`2` when the new feed adds error occurrences. `--no-validation` gives a faster
structural-only comparison.

#### `profile` and `explain` — deterministic feed facts

```bash
gtfs-guru profile -i feed.zip --date 2026-07-27 --pretty
gtfs-guru explain -i feed.zip --date 2026-07-27
gtfs-guru explain -i feed.zip --json --pretty
```

`profile` reports unique entity counts, route types, completeness facts, seven
actual service dates with calendar exceptions applied, and exact grouped
validation totals. `explain` is derived from nothing but that profile, so every
statement can be checked and no feed is sent to an LLM provider.

#### `spec-surface` — what this build answers for

```bash
gtfs-guru spec-surface --pretty
```

Prints the files, fields, enum values, and notice codes the build supports,
along with the specification revision and canonical validator release it was
checked against.

### Which standard a report answers for

Every JSON report's `summary` states the upstream revisions the build was
aligned with, alongside the validator's own version:

```bash
gtfs-guru -i feed.zip --stdout | jq '.summary | {validatorVersion, specRevision, canonicalBaseline}'
```

```json
{
  "validatorVersion": "1.0.0",
  "specRevision": "google/transit@3215f98f26615f1b925dca1bf2205311b747e308",
  "canonicalBaseline": "MobilityData/gtfs-validator@v8.0.1"
}
```

`specRevision` is the GTFS specification commit, and `canonicalBaseline` the
release of the canonical Java validator, that this build was checked against.
Both are extensions to the canonical report schema. `gtfs-guru spec-surface`
prints the files, fields, enum values, and notice codes that follow from them.

### Parallelism

`--threads` is report metadata retained for Java compatibility. Set Rayon's
environment variable to control actual parallelism:

```bash
RAYON_NUM_THREADS=8 gtfs-guru -i feed.zip -o ./report
```

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Validation completed |
| `1` | The run failed (invalid arguments, unreadable input, or I/O failure) |
| `2` | The feed did not meet `--fail-on` |

Use `--fail-on error` in CI. Without it, a completed validation exits 0 even
when the feed contains validation errors.

### Status badges

`--badge` writes a [shields.io endpoint][shields-endpoint] descriptor describing
the run:

```bash
gtfs-guru -i feed.zip -o ./report --fail-on none --badge badge/gtfs.json
```

```json
{
  "schemaVersion": 1,
  "label": "GTFS",
  "message": "0 errors, 3 warnings",
  "color": "yellow"
}
```

Publish that file (a `gh-pages` branch, an object store, anywhere reachable) and
reference it from a README:

```markdown
![GTFS](https://img.shields.io/endpoint?url=https://example.org/badge/gtfs.json)
```

The message is `valid` on a clean feed, `0 errors, N warnings` when only
warnings remain, and `N errors` otherwise; the colour follows. `--badge-svg`
writes a self-contained SVG for places that cannot reach shields.io, and
`--badge-label` replaces the `GTFS` on the left with, say, a feed name.

Pair it with `--fail-on none` when the badge is the point: a workflow that
aborts on the first error never gets to write one.

Both paths are taken as given rather than resolved against `--output_base`,
so a badge can be written straight into the directory a workflow publishes,
and they work with `--stdout` too.

[shields-endpoint]: https://shields.io/badges/endpoint-badge

### Continuous integration

#### GitHub Actions

The repository ships a composite action that installs a checksum-verified
binary, runs it, uploads SARIF to code scanning, and fails the job on a bad
feed:

```yaml
jobs:
  validate:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      security-events: write   # so SARIF reaches the Security tab
    steps:
      - uses: actions/checkout@v4
      - uses: abasis-ltd/gtfs.guru/action@v1
        with:
          feed: feed.zip
          fail-on: error
```

See [`action/README.md`](https://github.com/abasis-ltd/gtfs.guru/blob/main/action/README.md)
for every input, the outputs it sets, and the badge-publishing recipe.

Or drive the CLI yourself:

```yaml
jobs:
  validate:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install gtfs-guru
        run: |
          curl -fsSL https://raw.githubusercontent.com/abasis-ltd/gtfs.guru/main/scripts/install.sh | bash
          echo "$HOME/.local/bin" >> $GITHUB_PATH
      - name: Run validation
        run: gtfs-guru -i feed.zip -o out --fail-on error
```

#### GitLab CI

```yaml
validate:
  image: ubuntu:22.04
  before_script:
    - apt-get update && apt-get install -y ca-certificates curl
    - curl -fsSL https://raw.githubusercontent.com/abasis-ltd/gtfs.guru/main/scripts/install.sh | bash
    - export PATH="$HOME/.local/bin:$PATH"
  script:
    - gtfs-guru -i feed.zip -o out --fail-on error
```

## Web API

### Starting the Server

```bash
cargo run --release -p gtfs-guru-web
# Server starts at http://localhost:3000
```

### API Endpoints

- `GET /healthz` - Health check
- `GET /version` - Version info
- `GET /cors-proxy?url=...` - Same-origin remote feed fetch, restricted to public HTTP(S) addresses and bounded by rate, concurrency, timeout, redirect, and size limits
- `POST /create-job` - Create validation job
- `PUT /upload/{job_id}` - Upload GTFS file
- `GET /jobs/{job_id}/status` - Check status
- `GET /jobs/{job_id}/report.json` - JSON report
- `GET /jobs/{job_id}/report.html` - HTML report
