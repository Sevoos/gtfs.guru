# GTFS-Realtime support

Repo-facing note. Excluded from the published site.

[GTF-11](https://linear.app/abasis/issue/GTF-11/add-gtfs-realtime-validation-to-gtfs-guru)
is the plan of record: scope, the rule matrix, phase order, acceptance criteria.
This file holds only what the repository can say and the issue cannot — what
exists on disk, what was measured here, and the local facts the work runs into.
Where the two disagree, GTF-11 wins.

Reconciled against GTF-11 as of its 2026-09-01 revision. An earlier draft of
this file predated that revision and has been corrected; the notes below flag
where it had been wrong, because those errors were the kind that produce work
which then has to be undone.

## Status

Phase 0 (freeze contracts and establish truth): five of ten items done — both
upstream baselines pinned, the canonical JAR built and hashed, the official
schema vendored and hashed, and prost's decoding behaviour verified. The five
remaining are blocked on GTF-11's open questions, except the canonical
performance baselines and the expected-delta record format.

Phase 1 (vertical slice): the crate, the generated bindings, `RtFeed` and
`RtSnapshotContext` exist. The next two items — one header rule, and CLI/JSON
output — are blocked, on the frozen rule matrix and the report contract
respectively.

No rule is implemented, and nothing is wired into any surface.

## What is built

```text
crates/gtfs_validator_rt/
  Cargo.toml
  build.rs                     # protox -> prost codegen, no system protoc
  spec_baseline.json           # pinned schema + canonical Java revisions
  proto/
    gtfs-realtime.proto        # vendored official schema
    UPSTREAM.md                # provenance and re-verification
  src/
    lib.rs
    feed.rs                    # RtFeed, RtSource, ContentFingerprint
    context.rs                 # RtSnapshotContext, RtEntityRef
  tests/
    decoder.rs                 # prost behaviour under hostile input
    feed_and_context.rs
```

GTF-11's layout adds `validator.rs`, `index.rs`, `notice_schema.rs`, and
`rules/` as their phases arrive.

The dependency runs one way — `gtfs-guru-rt` depends on `gtfs-guru-core`, never
the reverse — so consumers who only validate a zip never build protobuf.

### Pinned baselines

`spec_baseline.json` pins the schema revision, the vendored file's SHA-256, and
the canonical Java commit; `proto/UPSTREAM.md` carries the provenance and the
commands to re-verify. Generated code is never committed: it is reproduced from
the vendored schema on every build, so the schema stays the single source of
truth.

One caveat on the canonical JAR. Its SHA-256 is recorded, but a Maven shade
build stamps every archive entry with the build time, so the same commit rebuilt
yields a different digest. The hash identifies one oracle binary — enough to
prove two parity runs used the same one — and cannot be reproduced from the
commit alone. `spec_baseline.json` records this as
`"jarReproducibleFromCommit": false`. GTF-11's acceptance criterion asks for the
baselines to be "pinned reproducibly", which the schema satisfies and the JAR
does not; closing that gap needs either a reproducible Maven build or a digest
over class contents rather than the archive.

### RtFeed

Carries the decoded message, its source, `encoded_len`, and a SHA-256
`ContentFingerprint` **taken over the bytes as received, before decoding**. The
raw buffer is not retained.

GTF-11 left open whether raw bytes or a fingerprint belong here, to be settled
by the Phase 0 extension and equality tests. They settled it: prost keeps no
unknown-field set, so a re-encoding does not reproduce the input and two
snapshots differing only in extension data compare equal as decoded values. A
fingerprint over the raw input is what the duplicate-detection rules (E017,
Phase 7) actually need, and it satisfies the issue's instruction not to retain
an extra full input buffer without a demonstrated need.

Size bounding lives here, at the edge where bytes enter, because the decoder
imposes no ceiling of its own. `DEFAULT_MAX_RT_BYTES` is 256 MiB, overridable
with `GTFS_VALIDATOR_MAX_RT_BYTES` to match the Schedule reader's convention;
`from_path` checks metadata before reading, so an oversized file is refused
without allocating the payload the limit exists to reject. `*_with_limit`
variants let the CLI and URL adapters pass their own bound.

`RtFeedError` separates decode failure from validation. Only structurally
impossible input lands there — truncation, a lying length prefix, a wrong wire
type. Input that is merely invalid decodes successfully and belongs to rules.

### RtSnapshotContext

Built once, in one pass, in the producer's entity order, and never rebuilt. The
canonical Java validator re-scans the entity list once per validator — seven of
its nine scan unconditionally — and this is what avoids that.

An entity carrying several payloads appears in *each* matching list under one
shared `entity_index`. The protobuf uses independent optional fields, so a
malformed entity really can populate more than one, and `tests/decoder.rs` pins
that such input decodes. Collapsing the payload into an exclusive enum would
hide it from the rules meant to report it.

Alongside the payload lists it carries the shared facts GTF-11 anticipates:
`duplicate_entity_ids` in first-seen order with every occurrence,
`entities_without_id`, `entities_without_payload`, and counts of the payload
types no candidate MVP rule reads (shapes, stops, trip modifications), so a
message of only those is not mistaken for an empty one. Nothing iterates a hash map, so no
output depends on hash order.

`observed_at` is supplied by the caller and never read from the system clock, so
freshness and future-timestamp rules give the same answer for a local file, a
recorded fixture, and an archived snapshot. The struct is `#[non_exhaustive]`
with a `new()` constructor, so Phase 3 can add `static_index` without a breaking
change to a published crate.

## Phase 0 decoder findings (measured 2026-09-10)

Executable answers to GTF-11's Phase 0 requirement to verify prost's
required-field, unknown-field, extension, and equality behaviour before rules
are written. Pinned by `crates/gtfs_validator_rt/tests/decoder.rs`; the Java
column comes from replaying identical bytes through
`gtfs-realtime-bindings:0.0.4` with `scripts/rt_parity/decoder_java_check.java`.

| Input | prost | Java 0.0.4 |
| :--- | :--- | :--- |
| Empty | decodes to defaults | rejected: missing required `header` |
| No `header` | decodes | rejected |
| No `header.gtfs_realtime_version` | decodes | rejected |
| No `trip_update.trip` | decodes | rejected |
| Unknown field | decodes, **dropped** | parsed, **retained**, round-trip identical |
| Extension field (1000-1999) | decodes, **dropped** | parsed, **retained**, round-trip identical |
| Unknown enum value | `Some(99)` — present, invalid | `hasIncrementality()==false` — absent |
| Truncated | `BufferUnderflow`, names the field | n/a |

Four consequences.

**prost does not enforce proto2 `required`.** Every required-field presence
check must be an explicit rule; the decoder will never raise one. Worse, because
`required` generates a non-`Option` field, an absent `gtfs_realtime_version` and
one explicitly set to `""` decode to the same value, so no rule can separate
them from the decoded message alone. This is the largest parity divergence
found: Java rejects four of these fixtures outright where GTFS Guru decodes and
continues.

**Content identity must fingerprint the raw bytes**, as described under `RtFeed`
above.

**Extension data is invisible.** MTA/NYCT-style feeds decode without error, but
their extension payloads are unreachable, so no selected rule may depend on
them.

**Nothing bounds message size.** A 50k-entity message decodes with allocation
tracking the input.

Two of these need a decision rather than documentation: whether the
required-field divergence is reported as one decode-failure notice (Java-like)
or per-field notices (Rust-like), and which reading of an unknown enum value is
canonical. Both are approved-delta material under GTF-11's expected-delta
lifecycle.

## Local facts for the work ahead

**RT notice metadata stays separate.** GTF-11 requires
`gtfs_validator_rt/rt_notice_metadata.json` and `build_rt_notice_schema_map()`,
because adding RT codes to the Schedule notice schema would make them part of
the Schedule specification surface. An earlier draft of this file said the
opposite — that each RT notice needs a `notice_metadata.json` entry and a
`NOTICE_SCHEMA_ENTRIES` row. It does not.

**Notice context.** RT notices have no CSV row. `ValidationNotice`
(`crates/gtfs_validator_core/src/notice.rs`) already keeps `file`, `row`, and
`field` optional and carries a free-form context map, which is the shape RT
needs. `entityIndex` is the primary locator; GTF-11 lists the full field set.

**A dedicated, sequential runner.** The rayon fan-out, panic catching, and
timing collection in `ValidatorRunner`
(`crates/gtfs_validator_core/src/validator.rs`) exist for feeds with millions of
`stop_times.txt` rows. GTF-11 says not to generalise it, since it is tied to
`GtfsFeed`, the Schedule validation context, and Rayon; locally, making it
generic over the feed type would touch every static rule for no gain on a few
megabytes of protobuf.

**`StringPool` has no non-inserting lookup.**
`crates/gtfs_validator_core/src/string_pool.rs` exposes only `new`, `intern`,
and `resolve`. Phase 3 needs `lookup(&str) -> Option<StringId>`; interning every
unknown RT identifier would let a long-running monitor grow the Schedule pool
without bound. Small and additive — it can land on its own.

**`stop_times_by_trip` already exists** on `GtfsFeed`
(`crates/gtfs_validator_core/src/feed.rs`), covering the expensive half of the
cross-reference indexes.

**The CLI entry point is one 1908-line file.**
`crates/gtfs_validator_cli/src/main.rs`. The `rt` subcommand should go in its own
module, which is a reasonable place to start breaking that file up.

**Golden fixtures.** `scripts/build_demo_feed.py` produces the deterministic
Schedule feed to pair with constructed RT messages. Parity fixtures must be
recorded snapshots, hashed and paired with an exact Schedule dataset — not live
feeds, which cannot be replayed in CI.

**MCP URL fetching** already sits behind `--allow-url`
(`crates/gtfs_validator_mcp/src/lib.rs`), which the RT tool should reuse.

**The RT baseline is unwatched.** `scripts/spec_watch.py` hardcodes
`crates/gtfs_validator_core/spec_baseline.json`, so nothing detects drift in the
RT pin. It will also need to handle two independent pins into `google/transit`:
the Schedule baseline is at `3215f98f`, the RT baseline at `262ae1e4`.

## Open decisions

GTF-11's "Decisions Required Before Implementation" is the list — nine questions
for Igor, none answered as of that issue's 2026-09-01 revision. No competing
list is kept here.

One superseded proposal worth naming, since it appeared in the earlier draft of
this file: a 90-second freshness default. The canonical W008 threshold is 65
seconds, and GTF-11's question 6 asks whether that is the default. Whatever is
chosen, a configurable override must be reported as a non-default validation
profile.
