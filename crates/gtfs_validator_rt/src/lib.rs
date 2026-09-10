//! GTFS-Realtime validation for GTFS Guru.
//!
//! Validates a single GTFS-Realtime `FeedMessage`, optionally cross-checked
//! against the GTFS Schedule feed it references. The dependency runs one way --
//! this crate depends on `gtfs-guru-core`, never the reverse -- so Schedule-only
//! consumers never build protobuf.

#![forbid(unsafe_code)]

pub mod context;
pub mod feed;

pub use context::{DeferredPayloadCounts, DuplicateEntityId, RtEntityRef, RtSnapshotContext};
pub use feed::{ContentFingerprint, RtFeed, RtFeedError, RtSource};

/// Types generated from the vendored GTFS-Realtime schema.
///
/// The schema declares `package transit_realtime`, so `prost` writes
/// `transit_realtime.rs`; the module below mirrors that package name. The file
/// is generated into `OUT_DIR` at build time by `build.rs` and is not committed.
pub mod transit_realtime {
    // The schema's own comments become doc comments verbatim, and their prose
    // indentation is not rustdoc's. Nothing here is ours to reformat.
    #![allow(clippy::doc_lazy_continuation)]
    #![allow(clippy::doc_overindented_list_items)]

    include!(concat!(env!("OUT_DIR"), "/transit_realtime.rs"));
}

/// The pinned upstream state this build validates against.
///
/// Embedded with `include_str!` so a normal build stays hermetic and a stored
/// report can quote exactly which schema and canonical baseline produced it.
/// `proto/UPSTREAM.md` documents the vendored schema; moving either pin is a
/// deliberate act, not a routine refresh.
pub const RT_SPEC_BASELINE_JSON: &str = include_str!("../spec_baseline.json");

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// The generated types exist, decode, and preserve proto2 optionality.
    #[test]
    fn decodes_a_minimal_feed_message() {
        let header = transit_realtime::FeedHeader {
            gtfs_realtime_version: "2.0".to_string(),
            ..Default::default()
        };
        let message = transit_realtime::FeedMessage {
            header,
            entity: Vec::new(),
        };

        let encoded = message.encode_to_vec();
        let decoded = transit_realtime::FeedMessage::decode(encoded.as_slice())
            .expect("round-trip a minimal FeedMessage");

        assert_eq!(decoded.header.gtfs_realtime_version, "2.0");
        // Absent proto2 optional fields stay distinguishable from zero.
        assert!(decoded.header.timestamp.is_none());
        assert!(decoded.entity.is_empty());
    }

    #[test]
    fn spec_baseline_is_embedded_and_parses() {
        let baseline: serde_json::Value =
            serde_json::from_str(RT_SPEC_BASELINE_JSON).expect("baseline is valid JSON");
        assert_eq!(
            baseline["specRevision"]["commit"],
            "474750a163088673df718838d4a1bb093391f9af"
        );
    }
}
