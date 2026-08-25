# GTFS-Realtime support: MVP plan

Repo-facing planning note. Excluded from the published site.

GTFS Guru validates static feeds only. The canonical GTFS-Realtime validator
([MobilityData/gtfs-realtime-validator](https://github.com/MobilityData/gtfs-realtime-validator))
is a separate Java service, so today an agency needs two tools and gets two
unrelated reports. Validating both halves of a feed against each other in one
run is the point of this work; raw speed is not, because RT messages are a few
megabytes and every implementation parses them quickly.

## Scope

In: a single-shot validation of one RT message, from a `.pb` file or a URL,
optionally cross-checked against a static feed. 21 RT-only rules plus 12
cross-reference rules.

Out: the monitoring mode and the four rules that compare successive snapshots
(E018, W003, W007, W008); the remaining 15 cross-reference rules; RT in the
browser, which CORS makes impractical; the desktop GUI.

The canonical validator has 61 rules (52 errors, 9 warnings). This MVP covers
33 of them.

## Crate layout

A new crate rather than a module inside the core. `gtfs_validator_core` is
already around 40k lines, and protobuf dependencies have no business in the
build of everyone who only validates a zip.

```
crates/gtfs_validator_rt/
  build.rs                     # protox -> prost codegen
  proto/gtfs-realtime.proto    # vendored official schema
  src/
    lib.rs
    feed.rs                    # RtFeed
    validator.rs               # RtValidator, RtValidatorRunner
    index.rs                   # static-feed indexes for cross-checks
    rules/
      mod.rs
      header.rs
      timestamps.rs
      trip_updates.rs
      vehicle_positions.rs
      alerts.rs
      cross_static.rs
```

The dependency runs one way: `gtfs_validator_rt` depends on
`gtfs_validator_core`, never the reverse. The core stays unaware of RT.

### Protobuf bindings

Generate them rather than depending on the [`gtfs-rt`](https://crates.io/crates/gtfs-rt)
crate. That crate's last release is 0.5.0 from March 2024 — two years without
an update while the RT spec kept moving.

`gtfs-realtime.proto` is a single official file. Compiling it with `protox`
(a pure-Rust protobuf compiler) instead of `prost-build` keeps `protoc` out of
the CI image. `prost` generates safe code, so `#![forbid(unsafe_code)]` holds.

## Types

```rust
pub struct RtFeed {
    pub message: FeedMessage,       // prost-generated
    pub source: RtSource,           // File(PathBuf) | Url(String)
    pub fetched_at: DateTime<Utc>,  // for freshness rules
}

pub trait RtValidator: Send + Sync {
    fn name(&self) -> &'static str;
    fn validate(
        &self,
        rt: &RtFeed,
        static_feed: Option<&GtfsFeed>,
        notices: &mut NoticeContainer,
    );
}
```

`static_feed` is an `Option` on purpose: an RT feed alone must still yield the
21 RT-only rules. When it is absent the cross-checks are skipped, and the
report has to say so in as many words rather than falling silent.

The runner is a fresh, sequential one. The rayon fan-out, panic catching, and
timing collection in `ValidatorRunner` (`crates/gtfs_validator_core/src/validator.rs`)
exist for feeds with millions of `stop_times.txt` rows; 33 rules over a few
megabytes finish in milliseconds. Making the existing runner generic over the
feed type would touch all 110 static rules for no gain.

## Notices

`NoticeContainer` and `ValidationNotice` are reused unchanged.
`ValidationNotice` (`crates/gtfs_validator_core/src/notice.rs`) already keeps
`file`, `row`, and `field` optional and carries a free-form `context` map, which
is exactly the shape RT needs:

- `file` — the source name
- `row` — left empty
- `context` — `entityId`, `tripId`, `vehicleId`, `stopId`, `stopSequence`,
  `timestamp`

Notice codes follow the existing snake_case convention (`service_never_active`,
`pathway_loop`), not the canonical `E001` form. The canonical ID goes into
`notice_metadata.json` as a separate field so anyone migrating from the
MobilityData validator can map the two sets.

This means every RT notice needs a `notice_metadata.json` entry and a
`NOTICE_SCHEMA_ENTRIES` row, or `--export-notices-schema` comes out incomplete.
33 entries by hand.

## Indexes for cross-checks

`GtfsFeed` (`crates/gtfs_validator_core/src/feed.rs`) already maintains
`stop_times_by_trip`, which covers the expensive half. `index.rs` builds the
rest once per run:

- `trip_id` → `&Trip`
- `stop_id` → `&Stop`, including `location_type` so RT references to a station
  instead of a platform can be caught
- `route_id` → `&Route`
- the set of `trip_id`s appearing in `frequencies.txt`, since frequency-based
  trips validate differently

## Rules

### RT-only (21)

Timestamp handling (POSIX form, not in the future, present in both the header
and the entities), `gtfs_realtime_version` in the header, `incrementality`,
`is_deleted` inside a `FULL_DATASET`, `stop_time_update` ordering by
`stop_sequence`, departure preceding arrival, presence of `stop_id` or
`stop_sequence`, agreement between `schedule_relationship` and the presence of
times, coordinate and bearing ranges, and `vehicle.id` presence and uniqueness.

### Cross-reference with static (12)

1. RT `trip_id` missing from `trips.txt`
2. RT `route_id` missing from `routes.txt`
3. RT `route_id` disagrees with the trip's `route_id` in the static feed
4. RT `stop_id` missing from `stops.txt`
5. RT `stop_id` points at a station rather than a platform (`location_type != 0`)
6. RT `stop_sequence` does not exist on that trip
7. the `stop_id`/`stop_sequence` pair contradicts the schedule
8. RT `direction_id` disagrees with the static feed
9. `start_date` falls outside the trip's service period
10. `start_time` disagrees with the schedule for a non-frequency trip
11. a frequency-based trip carries no `start_time`
12. an Alert `informed_entity` references entities that do not exist

Pin each rule to its canonical E-code against
[RULES.md](https://github.com/MobilityData/gtfs-realtime-validator/blob/master/RULES.md)
during implementation. Do not map them from memory.

## Surfaces

### CLI

A subcommand, not a flag on the default run:

```bash
gtfs-guru rt --rt feed.pb -i static.zip -o ./out
gtfs-guru rt --rt-url https://example.com/tripupdates.pb --no-static
```

`crates/gtfs_validator_cli/src/main.rs` is already 1869 lines in a single file.
The subcommand goes in its own module, which is a reasonable place to start
breaking that file up.

`--fail-on`, `--sarif`, `--stdout`, and the JSON report are reused as they are.
The RT report keeps the existing structure with its own section.

### MCP

One new tool, `validate_gtfs_rt`, answering in the same shape as
`validate_gtfs`: exact grouped totals plus up to three concrete examples per
code and severity. URL fetching stays behind the existing `--allow-url`.

### Python

`gtfs_guru.validate_rt(rt_path, static_path=None)`, returning the same report
object as `validate`.

### WASM and GUI

Untouched in the MVP.

## Tests

1. Unit fixtures: build a `FeedMessage` in code, assert the rule fires, then
   assert it stays quiet on the valid variant. Two tests per rule, 66 total.
2. Golden test: the demo feed from `scripts/build_demo_feed.py` paired with a
   constructed RT message carrying known defects, against a checked-in JSON
   report.
3. Parity: run 10 to 15 live RT feeds from the Mobility Database through both
   this and the canonical Java validator and reconcile the differences one by
   one. This is what earns trust in the rule set.

## Schedule

Roughly two to three weeks for one developer.

| Week | Work |
| :--- | :--- |
| 1 | Crate, protox codegen, `RtFeed`, `RtValidator` and its runner, CLI subcommand, report section. One rule wired end to end, from parsing through to HTML and SARIF. |
| 2 | The 21 RT-only rules, with tests. |
| 3 | Indexes, the 12 cross-reference rules, 33 `notice_metadata.json` entries, MCP tool, Python binding, parity run, documentation. |

Week 1 carries the risk. It is about threading a new kind of entity through
reporting machinery built around CSV rows. Once a single rule reaches HTML and
SARIF, the rest is mechanical.

## Decisions to make before starting

**Freshness without a monitoring mode.** "The header timestamp is too old"
needs only the message and the current time, so it fits the MVP, but the
threshold is arbitrary. Proposal: a hard default of 90 seconds, overridable
with `--rt-max-age`.

**How to express "no static feed".** Either an explicit `--no-static` flag or
simply omitting `-i`. The second is quieter, but a user can then miss that 12
rules never ran. Proposal: require the flag, and print a line in the report —
"12 cross-checks skipped: no static feed given".
