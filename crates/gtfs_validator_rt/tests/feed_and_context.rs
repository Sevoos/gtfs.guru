//! `RtFeed` provenance and the single-pass `RtSnapshotContext`.

use chrono::{TimeZone, Utc};
use gtfs_guru_rt::transit_realtime::*;
use gtfs_guru_rt::{ContentFingerprint, RtFeed, RtFeedError, RtSnapshotContext, RtSource};
use prost::Message;

fn header() -> FeedHeader {
    FeedHeader {
        gtfs_realtime_version: "2.0".to_string(),
        timestamp: Some(1_700_000_000),
        ..Default::default()
    }
}

fn entity(id: &str) -> FeedEntity {
    FeedEntity {
        id: id.to_string(),
        ..Default::default()
    }
}

fn trip_update(trip_id: &str) -> TripUpdate {
    TripUpdate {
        trip: TripDescriptor {
            trip_id: Some(trip_id.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn vehicle_position() -> VehiclePosition {
    VehiclePosition {
        position: Some(Position {
            latitude: 47.5,
            longitude: 8.5,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn encode(entities: Vec<FeedEntity>) -> Vec<u8> {
    FeedMessage {
        header: header(),
        entity: entities,
    }
    .encode_to_vec()
}

fn feed_from(entities: Vec<FeedEntity>) -> RtFeed {
    RtFeed::from_bytes(&encode(entities), RtSource::Bytes).expect("decodes")
}

fn observed_at() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 17, 19, 50, 33).unwrap()
}

// -- RtFeed ----------------------------------------------------------------

#[test]
fn records_size_and_source() {
    let bytes = encode(vec![entity("a")]);
    let feed = RtFeed::from_bytes(&bytes, RtSource::Bytes).expect("decodes");

    assert_eq!(feed.encoded_len(), bytes.len());
    assert_eq!(feed.source(), &RtSource::Bytes);
    assert_eq!(feed.entities().len(), 1);
}

/// The fingerprint covers the bytes as received, so it separates snapshots that
/// the decoded value cannot. Here the two inputs differ only in an unknown
/// field, which the compatibility boundary drops: the decoded messages are
/// equal, the fingerprints are not. Hashing a re-encoding would have called
/// these the same snapshot.
#[test]
fn fingerprint_distinguishes_what_decoding_discards() {
    let plain = encode(vec![entity("a")]);

    let mut with_unknown = plain.clone();
    with_unknown.extend_from_slice(&[0xf8, 0x3e, 0x2a]); // field 999, varint 42

    let first = RtFeed::from_bytes(&plain, RtSource::Bytes).expect("decodes");
    let second = RtFeed::from_bytes(&with_unknown, RtSource::Bytes).expect("decodes");

    assert_eq!(first.message(), second.message(), "decoded values agree");
    assert_ne!(
        first.content_fingerprint(),
        second.content_fingerprint(),
        "but the received bytes did not"
    );
    assert_eq!(
        second.message().encode_to_vec().len(),
        plain.len(),
        "the unknown field is gone after a round trip"
    );
}

#[test]
fn fingerprint_is_stable_and_hex_rendered() {
    let bytes = encode(vec![entity("a")]);
    let first = RtFeed::from_bytes(&bytes, RtSource::Bytes).expect("decodes");
    let second = RtFeed::from_bytes(&bytes, RtSource::Bytes).expect("decodes");

    assert_eq!(first.content_fingerprint(), second.content_fingerprint());
    assert_eq!(
        first.content_fingerprint(),
        ContentFingerprint::of(&bytes),
        "computed over the raw input"
    );

    let rendered = first.content_fingerprint().to_string();
    assert_eq!(rendered.len(), 64);
    assert!(rendered
        .chars()
        .all(|character| character.is_ascii_hexdigit()));
}

#[test]
fn oversized_input_is_refused_before_decoding() {
    let bytes = encode(vec![entity("a")]);

    let error =
        RtFeed::from_bytes_with_limit(&bytes, RtSource::Bytes, 4).expect_err("over the limit");

    match error {
        RtFeedError::TooLarge { actual, limit } => {
            assert_eq!(actual, bytes.len() as u64);
            assert_eq!(limit, 4);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn truncated_input_is_a_decode_error_not_a_notice() {
    let bytes = encode(vec![entity("a")]);
    let error = RtFeed::from_bytes(&bytes[..bytes.len() - 2], RtSource::Bytes)
        .expect_err("truncated input fails");

    match error {
        RtFeedError::Decode {
            input,
            encoded_len,
            content_fingerprint,
            ..
        } => {
            assert_eq!(input, RtSource::Bytes);
            assert_eq!(encoded_len, bytes.len() - 2);
            assert_eq!(
                content_fingerprint,
                ContentFingerprint::of(&bytes[..bytes.len() - 2])
            );
        }
        other => panic!("expected Decode, got {other:?}"),
    }
}

#[test]
fn reads_a_local_file_and_records_its_path() {
    let directory = std::env::temp_dir().join("gtfs-guru-rt-feed-test");
    std::fs::create_dir_all(&directory).expect("create temp dir");
    let path = directory.join("snapshot.pb");
    let bytes = encode(vec![entity("a")]);
    std::fs::write(&path, &bytes).expect("write fixture");

    let feed = RtFeed::from_path(&path).expect("decodes");
    assert_eq!(feed.source(), &RtSource::File(path.clone()));
    assert_eq!(feed.content_fingerprint(), ContentFingerprint::of(&bytes));

    let error = RtFeed::from_path_with_limit(&path, 4).expect_err("over the limit");
    assert!(
        matches!(error, RtFeedError::TooLarge { .. }),
        "got {error:?}"
    );

    std::fs::remove_file(&path).ok();
}

#[test]
fn missing_file_reports_its_path() {
    let error = RtFeed::from_path("/nonexistent/snapshot.pb").expect_err("no such file");
    match error {
        RtFeedError::Io { path, .. } => {
            assert_eq!(path, std::path::PathBuf::from("/nonexistent/snapshot.pb"))
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

// -- RtSnapshotContext -----------------------------------------------------

#[test]
fn payload_lists_follow_the_received_order() {
    let feed = feed_from(vec![
        FeedEntity {
            id: "one".into(),
            trip_update: Some(trip_update("t1")),
            ..Default::default()
        },
        FeedEntity {
            id: "two".into(),
            vehicle: Some(vehicle_position()),
            ..Default::default()
        },
        FeedEntity {
            id: "three".into(),
            trip_update: Some(trip_update("t2")),
            ..Default::default()
        },
    ]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.entity_count, 3);
    let indices: Vec<_> = context
        .trip_updates
        .iter()
        .map(|r| r.entity_index)
        .collect();
    assert_eq!(indices, vec![0, 2], "received order, not payload order");

    assert_eq!(context.trip_updates[0].entity_id, "one");
    assert_eq!(
        context.trip_updates[0].payload.trip.trip_id.as_deref(),
        Some("t1")
    );
    assert_eq!(context.vehicle_positions[0].entity_index, 1);
}

/// The rule GTF-11 is protecting: an entity with several payloads stays visible
/// in every list it belongs to, under one shared index.
#[test]
fn an_entity_with_several_payloads_appears_in_each_list() {
    let feed = feed_from(vec![FeedEntity {
        id: "multi".into(),
        trip_update: Some(trip_update("t1")),
        vehicle: Some(vehicle_position()),
        alert: Some(Alert::default()),
        ..Default::default()
    }]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.entity_count, 1);
    assert_eq!(context.trip_updates.len(), 1);
    assert_eq!(context.vehicle_positions.len(), 1);
    assert_eq!(context.alerts.len(), 1);

    assert_eq!(context.trip_updates[0].entity_index, 0);
    assert_eq!(context.vehicle_positions[0].entity_index, 0);
    assert_eq!(context.alerts[0].entity_index, 0);
    assert!(context.entities_without_payload.is_empty());
}

#[test]
fn duplicate_ids_are_reported_in_first_seen_order() {
    let feed = feed_from(vec![
        entity("b"),
        entity("a"),
        entity("b"),
        entity("c"),
        entity("a"),
        entity("b"),
    ]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    let duplicates: Vec<_> = context
        .duplicate_entity_ids
        .iter()
        .map(|duplicate| (duplicate.entity_id, duplicate.indices.clone()))
        .collect();

    assert_eq!(
        duplicates,
        vec![("b", vec![0, 2, 5]), ("a", vec![1, 4])],
        "first-seen order, every occurrence, ascending"
    );
}

/// Nothing in the output may depend on hash iteration order.
#[test]
fn duplicate_reporting_is_deterministic() {
    let entities: Vec<FeedEntity> = (0..200)
        .map(|index| entity(&format!("id-{}", index % 50)))
        .collect();
    let feed = feed_from(entities);

    let first = RtSnapshotContext::new(&feed, observed_at());
    let baseline: Vec<_> = first
        .duplicate_entity_ids
        .iter()
        .map(|duplicate| (duplicate.entity_id.to_string(), duplicate.indices.clone()))
        .collect();
    assert_eq!(baseline.len(), 50);

    for _ in 0..20 {
        let repeat = RtSnapshotContext::new(&feed, observed_at());
        let observed: Vec<_> = repeat
            .duplicate_entity_ids
            .iter()
            .map(|duplicate| (duplicate.entity_id.to_string(), duplicate.indices.clone()))
            .collect();
        assert_eq!(observed, baseline);
    }
}

/// Java accepts an explicitly present empty required string. Those entities are
/// listed separately rather than grouped as duplicates of the empty string;
/// an absent id is rejected before a context can be built.
#[test]
fn entities_with_an_empty_id_are_listed_not_treated_as_duplicates() {
    let feed = feed_from(vec![entity(""), entity("a"), entity(""), entity("a")]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.entities_without_id, vec![0, 2]);
    assert_eq!(context.duplicate_entity_ids.len(), 1);
    assert_eq!(context.duplicate_entity_ids[0].entity_id, "a");
    assert_eq!(context.duplicate_entity_ids[0].indices, vec![1, 3]);
}

#[test]
fn entities_carrying_no_payload_are_listed() {
    let feed = feed_from(vec![
        entity("empty-1"),
        FeedEntity {
            id: "has-one".into(),
            alert: Some(Alert::default()),
            ..Default::default()
        },
        entity("empty-2"),
    ]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.entities_without_payload, vec![0, 2]);
}

#[test]
fn combined_feed_rules_are_gated_on_both_entity_types() {
    let trip_only = feed_from(vec![FeedEntity {
        id: "t".into(),
        trip_update: Some(trip_update("t1")),
        ..Default::default()
    }]);
    assert!(
        !RtSnapshotContext::new(&trip_only, observed_at()).has_combined_trip_and_vehicle_entities()
    );

    let both = feed_from(vec![
        FeedEntity {
            id: "t".into(),
            trip_update: Some(trip_update("t1")),
            ..Default::default()
        },
        FeedEntity {
            id: "v".into(),
            vehicle: Some(vehicle_position()),
            ..Default::default()
        },
    ]);
    assert!(RtSnapshotContext::new(&both, observed_at()).has_combined_trip_and_vehicle_entities());
}

/// Observation time is carried, never sourced from the clock.
#[test]
fn observation_time_is_supplied_by_the_caller() {
    let feed = feed_from(vec![entity("a")]);
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.observed_at, observed_at());
    assert_eq!(context.header_timestamp(), Some(1_700_000_000));
}

#[test]
fn timestamps_use_the_signed_long_view_exposed_by_java() {
    let bytes = FeedMessage {
        header: FeedHeader {
            gtfs_realtime_version: "2.0".to_string(),
            timestamp: Some(1_u64 << 63),
            ..Default::default()
        },
        entity: Vec::new(),
    }
    .encode_to_vec();
    let feed = RtFeed::from_bytes(&bytes, RtSource::Bytes).expect("decodes");
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.header_timestamp(), Some(i64::MIN));
}

#[test]
fn an_empty_message_yields_an_empty_context() {
    let feed = feed_from(Vec::new());
    let context = RtSnapshotContext::new(&feed, observed_at());

    assert_eq!(context.entity_count, 0);
    assert!(context.trip_updates.is_empty());
    assert!(context.vehicle_positions.is_empty());
    assert!(context.alerts.is_empty());
    assert!(context.duplicate_entity_ids.is_empty());
}
