# GTFS-Realtime support

Repo-facing note. Excluded from the published site.

[GTF-11](https://linear.app/abasis/issue/GTF-11/add-gtfs-realtime-validation-to-gtfs-guru)
is the plan of record: scope, the rule matrix, phase order, acceptance criteria.
This file holds only what the repository can say and the issue cannot — what
exists on disk, what was measured here, and the local facts the work runs into.
Where the two disagree, GTF-11 wins.

Reconciled against GTF-11 after Igor resolved its twelve implementation
questions on 2026-09-16. The default profile follows the pinned Java executable;
current-spec corrections require a separate profile.

## Status

Phase 0's product decisions are resolved. Both upstream baselines and the
canonical JAR are pinned, the delta format exists, and the 33-rule matrix is
frozen. Canonical cold, warm, and memory measurements remain.

Phase 1: the crate, current generated bindings, Java 0.0.4 compatibility
descriptor, Java-compatible loading boundary, bounded file read, `RtFeed`, and
`RtSnapshotContext` exist. E038, E039, E049, CLI/report wiring, and the first
end-to-end differential slice remain.

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
    java-0.0.4/
      gtfs-realtime.proto      # schema embedded in Java bindings 0.0.4
      UPSTREAM.md
  src/
    canonical_decode.rs        # Java-compatible wire normalization
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

`spec_baseline.json` pins the current schema, the schema used by Java bindings
0.0.4, their SHA-256 values, and the canonical Java commit. The two
`UPSTREAM.md` files carry provenance and re-verification commands. Generated
code and the old-schema descriptor are reproduced at build time.

One caveat on the canonical JAR. Its SHA-256 is recorded, but a Maven shade
build stamps every archive entry with the build time, so the same commit rebuilt
yields a different digest. The hash identifies one oracle binary — enough to
prove two parity runs used the same one — and cannot be reproduced from the
commit alone. `spec_baseline.json` records this as
`"jarReproducibleFromCommit": false`. Igor accepted the binary digest as the
oracle identity for the MVP; reproducible rebuilding is deferred.

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

Size bounding lives here, at the edge where bytes enter, because raw `prost`
decoding imposes no input ceiling of its own. `DEFAULT_MAX_RT_BYTES` is 64 MiB,
overridable with `GTFS_VALIDATOR_MAX_RT_BYTES`. `from_path` rejects an
already-large file from metadata, then wraps the actual read in
`take(limit + 1)` so growth between metadata and I/O cannot bypass the limit.

`RtFeedError` separates load failure from validation. Malformed wire data and
missing old-schema proto2 required fields reject the message before ordinary
rules run, matching Java. The error can name all missing field paths.

### RtSnapshotContext

Built once, in one pass, in the producer's entity order, and never rebuilt. The
canonical Java validator re-scans the entity list once per validator — seven of
its nine scan unconditionally — and this is what avoids that.

An entity carrying several payloads appears in *each* matching list under one
shared `entity_index`. The protobuf uses independent optional fields, so a
malformed entity really can populate more than one, and `tests/decoder.rs` pins
that such input decodes. Collapsing the payload into an exclusive enum would
hide it from the rules meant to report it.

Alongside the payload lists it carries `duplicate_entity_ids` in first-seen
order with every occurrence, `entities_without_id`, and
`entities_without_payload`. Current-only payload fields are removed by the Java
compatibility boundary, so they cannot change default-profile results. Nothing
iterates a hash map, so no output depends on hash order.

`observed_at` is supplied by the caller and never read from the system clock, so
freshness and future-timestamp rules give the same answer for a local file, a
recorded fixture, and an archived snapshot. The struct is `#[non_exhaustive]`
with a `new()` constructor, so Phase 3 can add `static_index` without a breaking
change to a published crate.

## Decoder compatibility (measured 2026-09-10, implemented 2026-09-17)

Raw `prost` accepts absent proto2 required fields, exposes unknown enum numbers,
rejects known fields with the wrong wire type, rejects invalid UTF-8 strings,
and drops unknown data. Java 0.0.4 rejects missing required fields, treats
unknown enum and wrong-wire occurrences as unknown data, retains an earlier
recognized enum value, and exposes invalid string bytes with replacement
characters.

`canonical_decode.rs` closes these differences before rules run. It walks the
wire message using a descriptor compiled from the exact Java 0.0.4 schema,
removes fields and enum values unavailable to Java, ignores wrong-wire
occurrences, reproduces protobuf 2.6.1 varint and malformed-UTF-8 behavior, and
recursively checks required fields after message merging. It enforces Java's
64-level recursion ceiling and writes normalization into one bounded buffer
before decoding the result into the current Rust model.

The original bytes are still fingerprinted before normalization. This preserves
content identity for future E017 behavior even though extensions and other
unknown data are intentionally invisible to default-profile rules.

The fixtures are pinned in `crates/gtfs_validator_rt/tests/decoder.rs` and
replayed against the exact JAR with
`scripts/rt_parity/decoder_java_check.java`. The two proposed decoder deltas were
removed after parity was implemented.

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

**`StringPool` has a non-inserting lookup.** Phase 3 can resolve RT identifiers
without interning every unknown value into the Schedule pool.

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

**The RT baseline is watched.** `scripts/spec_watch.py` tracks the independent
Schedule and Realtime pins; moving either remains an explicit baseline update.

## Resolved decisions

GTF-11 contains Igor's twelve answers and is the contract. The default profile
matches Java, the matrix is frozen, W008 uses 65 seconds, future timestamps use
Java's 60-second tolerance, and MCP/Python remain in MVP scope. No competing
decision list is kept here.
