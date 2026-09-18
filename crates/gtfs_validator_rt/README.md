# gtfs-guru-rt

GTFS-Realtime validation foundations for [GTFS Guru](https://gtfs.guru).

The crate currently provides Java-compatible loading for one GTFS-Realtime
`FeedMessage` and an ordered snapshot context for Trip Updates, Vehicle
Positions, and Service Alerts. RT rules and adapter wiring are not implemented
yet. Schedule validation lives in `gtfs-guru-core`; this crate depends on it,
never the reverse, so consumers who only validate a zip do not build protobuf.

The current protobuf schema generates the Rust model. The schema bundled with
Java bindings 0.0.4 supplies a compatibility descriptor so the default profile
matches the pinned canonical validator's loading behavior. Both are vendored,
not fetched; see [`proto/UPSTREAM.md`](proto/UPSTREAM.md),
[`proto/java-0.0.4/UPSTREAM.md`](proto/java-0.0.4/UPSTREAM.md), and
`spec_baseline.json` for provenance.

The Rust implementation is licensed under Apache-2.0. The vendored Java 0.0.4
compatibility schema is licensed under CC BY 3.0 with attribution retained in
the file and its `UPSTREAM.md`. The package includes both license texts in
`LICENSE-APACHE` and `LICENSE-CC-BY-3.0`.
