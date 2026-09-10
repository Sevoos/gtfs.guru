# Vendored GTFS-Realtime schema

`gtfs-realtime.proto` in this directory is a verbatim copy of the official
schema. It is committed rather than fetched because `build.rs` compiles it at
build time (protox -> prost-build), and a build must not depend on the network.

Nothing in this directory may be edited. A local change would silently move
GTFS Guru off the official wire format, and the checksum below is what proves
it has not happened.

## Provenance

| | |
|---|---|
| Repository | [`google/transit`](https://github.com/google/transit) |
| Commit | `474750a163088673df718838d4a1bb093391f9af` |
| Commit subject | Mark images in Service Alerts as final (#651) |
| Committed | 2026-08-17T19:50:33Z |
| Upstream path | `gtfs-realtime/proto/gtfs-realtime.proto` |
| SHA-256 | `8feff2c5499e0ff08777e203e49ef702e07be0a8376b8bb2126add311a709299` |
| Size | 65237 bytes |
| Syntax | proto2, package `transit_realtime` |
| License | Apache-2.0, held by The GTFS Specifications Authors (header retained in the file) |

Immutable source URL:

```
https://raw.githubusercontent.com/google/transit/474750a163088673df718838d4a1bb093391f9af/gtfs-realtime/proto/gtfs-realtime.proto
```

The URL pins a commit SHA, not a branch, so it keeps resolving to these exact
bytes no matter what `master` does later.

## Re-verifying

```bash
sha256sum crates/gtfs_validator_rt/proto/gtfs-realtime.proto
# 8feff2c5499e0ff08777e203e49ef702e07be0a8376b8bb2126add311a709299
```

To confirm the copy still matches upstream at the pinned commit:

```bash
SHA=474750a163088673df718838d4a1bb093391f9af
curl -fsSL "https://raw.githubusercontent.com/google/transit/$SHA/gtfs-realtime/proto/gtfs-realtime.proto" \
  | diff - crates/gtfs_validator_rt/proto/gtfs-realtime.proto && echo "matches upstream"
```

## Semantics live elsewhere

The `.proto` carries the wire format. The prose that says what the fields
*mean* -- which proto2-optional fields GTFS-Realtime v2.0 treats as required,
freshness expectations, enum semantics -- is `gtfs-realtime/spec/en/reference.md`
in the same repository at the same commit. It is pinned in
`../spec_baseline.json` but deliberately not vendored: nothing compiles against
it, and a second copy of a long prose document would only drift.

Read it at the pinned revision:
<https://github.com/google/transit/blob/474750a163088673df718838d4a1bb093391f9af/gtfs-realtime/spec/en/reference.md>

## Updating

Moving to a newer schema is a deliberate act, not a routine refresh. Replace
the file, update every row of the provenance table and the checksum in
`../spec_baseline.json`, then re-run the decoder and parity suites: a schema
bump can change generated Rust types, and the pinned Java validator stays on
`gtfs-realtime-bindings:0.0.4`, so newer fields will have no canonical
counterpart and need classifying as current-spec behavior.
