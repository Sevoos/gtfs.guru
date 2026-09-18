# Java 0.0.4 compatibility schema

`gtfs-realtime.proto` in this directory is the schema used to generate
`com.google.transit:gtfs-realtime-bindings:0.0.4`, the protobuf implementation
inside the pinned canonical validator JAR.

It does not generate GTFS Guru's public Rust model. `build.rs` compiles it to a
descriptor used only at the loading boundary, where GTFS Guru reproduces the
old Java binding's required-field, unknown-field, enum, and wire-type behavior.

Nothing in this directory may be edited locally.

## Provenance

| | |
|---|---|
| Repository | [`MobilityData/gtfs-realtime-bindings`](https://github.com/MobilityData/gtfs-realtime-bindings) |
| Tag | `gtfs-realtime-bindings-java-0.0.4` |
| Commit | `c2ab4841effc5626889376b34b63e5fef1136c40` |
| Tagged | 2015-02-27T05:42:48Z |
| Upstream path | `gtfs-realtime.proto` |
| SHA-256 | `09a04b89995ddcbfce722baefc5be4a8dcfd61637d949cbf3998991dd219b26e` |
| Size | 26616 bytes |
| Syntax | proto2, package `transit_realtime` |
| License | CC BY 3.0 (header retained in the file) |

Attribution: **GTFS-realtime protocol schema**, Copyright 2011 Google Inc,
retrieved unmodified from the immutable source below. Licensed under
[Creative Commons Attribution 3.0](https://creativecommons.org/licenses/by/3.0/).

Immutable source URL:

```text
https://raw.githubusercontent.com/google/gtfs-realtime-bindings/c2ab4841effc5626889376b34b63e5fef1136c40/gtfs-realtime.proto
```

The artifact POM names this exact tag in its SCM metadata. Re-verify with:

```bash
sha256sum crates/gtfs_validator_rt/proto/java-0.0.4/gtfs-realtime.proto
```
