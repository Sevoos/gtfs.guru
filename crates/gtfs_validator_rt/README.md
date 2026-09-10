# gtfs-guru-rt

GTFS-Realtime validation for [GTFS Guru](https://gtfs.guru).

Validates one GTFS-Realtime `FeedMessage` -- Trip Updates, Vehicle Positions,
and Service Alerts -- on its own, or cross-checked against the GTFS Schedule
feed it refers to. Schedule validation lives in `gtfs-guru-core`; this crate
depends on it, never the reverse, so consumers who only validate a zip do not
build protobuf.

The protobuf schema is vendored, not fetched: see
[`proto/UPSTREAM.md`](proto/UPSTREAM.md) for its provenance and
`spec_baseline.json` for the pinned specification and canonical-validator
revisions.

Licensed under Apache-2.0.
