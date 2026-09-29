use chrono::{TimeZone, Utc};
use gtfs_guru_core::NoticeContainer;
use gtfs_guru_rt::rules::header::HeaderValidator;
use gtfs_guru_rt::transit_realtime::*;
use gtfs_guru_rt::validator::RtValidator;
use gtfs_guru_rt::{RtFeed, RtSnapshotContext, RtSource};
use prost::Message;

fn msg(version: &str, incrementality: Option<i32>, is_deleted: Option<bool>) -> FeedMessage {
    let mut entity = Vec::new();
    if let Some(deleted) = is_deleted {
        entity.push(FeedEntity {
            id: "e1".to_string(),
            is_deleted: Some(deleted),
            ..Default::default()
        });
    }
    FeedMessage {
        header: FeedHeader {
            gtfs_realtime_version: version.to_string(),
            incrementality,
            ..Default::default()
        },
        entity,
    }
}

fn feed_of(message: FeedMessage) -> RtFeed {
    RtFeed::from_bytes(&message.encode_to_vec(), RtSource::Bytes).expect("fixture decodes")
}
fn observed_at() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

fn run(message: FeedMessage) -> NoticeContainer {
    let feed = feed_of(message);
    let context = RtSnapshotContext::new(&feed, observed_at());
    let mut notices = NoticeContainer::new();
    HeaderValidator.validate(&context, &mut notices);
    notices
}

#[test]
fn an_unparseable_version_suppresses_e049_as_java_does() {
    let notices = run(msg("abcd", None, None));
    let codes: Vec<&str> = notices.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(codes, ["invalid_realtime_version"]);
}

#[test]
fn e039_records_the_entity_position_and_value() {
    let notices = run(msg("1.0", Some(0), Some(false)));
    let notice = notices.iter().next().expect("one notice");
    assert_eq!(notice.context["entityIndex"], 0);
    assert_eq!(notice.context["entityId"], "e1");
    assert_eq!(notice.context["isDeleted"], false);
}

#[test]
fn e038_records_the_offending_version() {
    let notices = run(msg("3.0", Some(0), None));
    let notice = notices.iter().next().expect("one notice");
    assert_eq!(notice.context["fieldPath"], "header.gtfs_realtime_version");
    assert_eq!(notice.context["gtfsRealtimeVersion"], "3.0");
}

#[test]
fn e049_records_the_version_that_required_incrementality() {
    let notices = run(msg("2.0", None, None));
    let notice = notices.iter().next().expect("one notice");
    assert_eq!(notice.context["fieldPath"], "header.incrementality");
    assert_eq!(notice.context["gtfsRealtimeVersion"], "2.0");
}

#[test]
fn header_rules_match_the_pinned_java_validator() {
    for (label, message, expected) in [
        (
            "v2.0, no incrementality",
            msg("2.0", None, None),
            vec!["missing_header_incrementality"],
        ),
        ("v2.0, FULL_DATASET", msg("2.0", Some(0), None), vec![]),
        (
            "suffixed version",
            msg("2.0f", None, None),
            vec!["invalid_realtime_version", "missing_header_incrementality"],
        ),
        (
            "absent incrementality with is_deleted",
            msg("2.0", None, Some(true)),
            vec![
                "full_dataset_entity_is_deleted",
                "missing_header_incrementality",
            ],
        ),
        ("v1.0, no incrementality", msg("1.0", None, None), vec![]),
        (
            "empty version",
            msg("", None, None),
            vec!["invalid_realtime_version"],
        ),
        (
            "FULL_DATASET is_deleted=true",
            msg("1.0", Some(0), Some(true)),
            vec!["full_dataset_entity_is_deleted"],
        ),
        (
            "FULL_DATASET is_deleted=false",
            msg("1.0", Some(0), Some(false)),
            vec!["full_dataset_entity_is_deleted"],
        ),
        (
            "DIFFERENTIAL is_deleted=true",
            msg("1.0", Some(1), Some(true)),
            vec![],
        ),
    ] {
        let notices = run(message);
        let codes: Vec<&str> = notices.iter().map(|n| n.code.as_str()).collect();
        assert_eq!(codes, expected, "{label}");
    }
}

/// Ignored by default: CI has no JDK, and this writes files.
///
/// Writes the cases asserted above as encoded messages, so the canonical
/// validator can be run over the same bytes rather than over a second Java-side
/// reconstruction of them.
///
/// ```text
/// cargo test -p gtfs-guru-rt --test rules_header -- --ignored dump_fixtures
/// java -cp "$GTFS_RT_VALIDATOR_JAR" scripts/rt_parity/header_java_check.java \
///      target/rt-header-fixtures
/// ```
#[test]
#[ignore = "writes fixture files for the optional Java cross-check"]
fn dump_fixtures() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/rt-header-fixtures");
    std::fs::create_dir_all(&dir).expect("create fixture directory");

    for (name, message) in [
        (
            "absent_incrementality_is_deleted",
            msg("2.0", None, Some(true)),
        ),
        (
            "differential_is_deleted_true",
            msg("1.0", Some(1), Some(true)),
        ),
        ("empty_version", msg("", None, None)),
        (
            "full_dataset_is_deleted_false",
            msg("1.0", Some(0), Some(false)),
        ),
        (
            "full_dataset_is_deleted_true",
            msg("1.0", Some(0), Some(true)),
        ),
        ("suffixed_version", msg("2.0f", None, None)),
        ("unparseable_version", msg("abcd", None, None)),
        ("v1_0_no_incrementality", msg("1.0", None, None)),
        ("v2_0_full_dataset", msg("2.0", Some(0), None)),
        ("v2_0_no_incrementality", msg("2.0", None, None)),
    ] {
        std::fs::write(dir.join(format!("{name}.pb")), message.encode_to_vec())
            .expect("write fixture");
    }
}
