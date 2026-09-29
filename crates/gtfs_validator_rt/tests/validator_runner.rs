//! The Realtime runner itself: that registration reaches the rules, and that a
//! panicking rule is reported rather than losing the rest of the run.

use chrono::{TimeZone, Utc};
use gtfs_guru_core::NoticeContainer;
use gtfs_guru_rt::rules::header::HeaderValidator;
use gtfs_guru_rt::transit_realtime::{FeedHeader, FeedMessage};
use gtfs_guru_rt::validator::{default_rt_runner, RtValidator, RtValidatorRunner};
use gtfs_guru_rt::{RtFeed, RtSnapshotContext, RtSource};
use prost::Message;

/// A feed carrying nothing but a version. That is all these tests need: the
/// header rule turns one into exactly one predictable notice, so what is being
/// asserted is the runner's behavior and not the rule's.
///
/// Deliberately not shared with `rules_header.rs`: each file under `tests/`
/// compiles as its own crate, and a `tests/common/mod.rs` costs more than these
/// few lines are worth.
fn feed_of(version: &str) -> RtFeed {
    let message = FeedMessage {
        header: FeedHeader {
            gtfs_realtime_version: version.to_string(),
            ..Default::default()
        },
        entity: Vec::new(),
    };
    RtFeed::from_bytes(&message.encode_to_vec(), RtSource::Bytes).expect("fixture decodes")
}

fn observed_at() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

struct PanickingValidator;

impl RtValidator for PanickingValidator {
    fn name(&self) -> &'static str {
        "panicking"
    }
    fn validate(&self, _: &RtSnapshotContext<'_>, _: &mut NoticeContainer) {
        panic!("boom");
    }
}

#[test]
fn default_runner_registers_the_header_rule() {
    // A v2.0 feed with no incrementality: the header rule reports E049 for it,
    // so seeing that notice proves the rule was registered and actually ran.
    let feed = feed_of("2.0");
    let context = RtSnapshotContext::new(&feed, observed_at());
    let mut notices = NoticeContainer::new();
    default_rt_runner().run(&context, &mut notices);

    let codes: Vec<&str> = notices.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(codes, ["missing_header_incrementality"]);
}

#[test]
fn a_panicking_validator_is_reported_and_the_run_continues() {
    let feed = feed_of("abcd");
    let context = RtSnapshotContext::new(&feed, observed_at());

    let mut runner = RtValidatorRunner::new();
    runner.register(PanickingValidator);
    runner.register(HeaderValidator);

    let mut notices = NoticeContainer::new();
    runner.run(&context, &mut notices);

    // The panic is reported, and the validator registered after it still ran.
    let codes: Vec<&str> = notices.iter().map(|n| n.code.as_str()).collect();
    assert_eq!(
        codes,
        [
            "runtime_exception_in_rt_validator_error",
            "invalid_realtime_version"
        ]
    );

    // The payload is what says *what* went wrong, so it must survive the catch.
    let panic_notice = notices.iter().next().expect("the panic notice");
    assert_eq!(panic_notice.context["validator"], "panicking");
    assert_eq!(panic_notice.context["message"], "boom");
}
