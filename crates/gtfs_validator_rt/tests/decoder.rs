//! What the generated bindings actually do with hostile and non-conforming
//! input.
//!
//! GTF-11 Phase 0 requires this evidence *before* rules are written, because
//! each answer decides what a rule can even observe. These tests pin measured
//! behaviour, not intended behaviour: where `prost` and the pinned Java
//! bindings disagree, the test documents the divergence rather than asserting
//! the behaviour we would have preferred.
//!
//! `scripts/rt_parity/decoder_java_check.java` replays the same bytes through
//! `gtfs-realtime-bindings:0.0.4`. Run `dump_fixtures` (ignored by default) to
//! regenerate its inputs.

use gtfs_guru_rt::transit_realtime::*;
use prost::Message;

// -- protobuf wire-format construction -------------------------------------
//
// Fixtures are built as raw bytes rather than by encoding a Rust struct: the
// point is to present input the generated types cannot produce themselves --
// absent required fields, unknown field numbers, extension ranges.

fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn tag(field: u32, wire_type: u8, out: &mut Vec<u8>) {
    varint(((field as u64) << 3) | wire_type as u64, out);
}

/// Length-delimited field (wire type 2): strings, bytes, submessages.
fn len_delimited(field: u32, payload: &[u8], out: &mut Vec<u8>) {
    tag(field, 2, out);
    varint(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

/// Varint field (wire type 0): integers, bools, enums.
fn varint_field(field: u32, value: u64, out: &mut Vec<u8>) {
    tag(field, 0, out);
    varint(value, out);
}

/// 32-bit fixed field (wire type 5): float.
fn float_field(field: u32, value: f32, out: &mut Vec<u8>) {
    tag(field, 5, out);
    out.extend_from_slice(&value.to_le_bytes());
}

/// A FeedHeader carrying both its required version and a timestamp.
fn valid_header_bytes() -> Vec<u8> {
    let mut header = Vec::new();
    len_delimited(1, b"2.0", &mut header);
    varint_field(3, 1_700_000_000, &mut header);
    header
}

/// A FeedMessage wrapping the given entity payloads.
fn feed_message_bytes(entities: &[Vec<u8>]) -> Vec<u8> {
    let mut message = Vec::new();
    len_delimited(1, &valid_header_bytes(), &mut message);
    for entity in entities {
        len_delimited(2, entity, &mut message);
    }
    message
}

// -- proto2 required fields ------------------------------------------------

/// **The headline finding: `prost` does not enforce proto2 `required`.**
///
/// Empty input is not a decode failure. It yields a `FeedMessage` whose
/// required `header` is a defaulted `FeedHeader` and whose required
/// `gtfs_realtime_version` is `""`.
///
/// The pinned Java bindings reject the same bytes with
/// `InvalidProtocolBufferException`, so this is a real divergence from the
/// canonical validator, not a detail. Two consequences for the rule engine:
///
/// 1. Presence of every required field must be checked by an explicit rule.
///    The decoder will never raise it.
/// 2. Because `required` generates a non-`Option` field, "absent" and "present
///    but empty" are indistinguishable after decoding. A rule that must tell
///    them apart cannot be written against the decoded message alone.
#[test]
fn empty_input_decodes_instead_of_failing() {
    let decoded = FeedMessage::decode(&[][..]).expect("prost accepts empty input");

    assert_eq!(decoded.header.gtfs_realtime_version, "");
    assert!(decoded.header.timestamp.is_none());
    assert!(decoded.entity.is_empty());
}

/// A FeedMessage with no `header` at all decodes to a defaulted header.
#[test]
fn missing_required_header_decodes() {
    let mut message = Vec::new();
    len_delimited(2, &[], &mut message); // an entity, but no header

    let decoded = FeedMessage::decode(&message[..]).expect("missing required header is accepted");
    assert_eq!(decoded.header.gtfs_realtime_version, "");
}

/// A header present but missing its required version field decodes the same
/// way as one carrying `version = ""`. This is the indistinguishability above,
/// made concrete: both byte strings produce an identical Rust value.
#[test]
fn missing_required_version_is_indistinguishable_from_empty() {
    let mut header_without = Vec::new();
    varint_field(3, 1_700_000_000, &mut header_without);
    let mut absent = Vec::new();
    len_delimited(1, &header_without, &mut absent);

    let mut header_empty = Vec::new();
    len_delimited(1, b"", &mut header_empty);
    varint_field(3, 1_700_000_000, &mut header_empty);
    let mut present_but_empty = Vec::new();
    len_delimited(1, &header_empty, &mut present_but_empty);

    let from_absent = FeedMessage::decode(&absent[..]).expect("decodes");
    let from_empty = FeedMessage::decode(&present_but_empty[..]).expect("decodes");

    assert_ne!(absent, present_but_empty, "the inputs differ on the wire");
    assert_eq!(
        from_absent, from_empty,
        "but decode to the same value, so no rule can separate them"
    );
}

/// The same holds nested: `TripUpdate.trip` is required, and its absence is
/// silently accepted.
#[test]
fn missing_required_nested_field_decodes() {
    let mut entity = Vec::new();
    len_delimited(1, b"entity-1", &mut entity);
    len_delimited(3, &[], &mut entity); // TripUpdate with no `trip`

    let message = feed_message_bytes(&[entity]);
    let decoded = FeedMessage::decode(&message[..]).expect("missing nested required is accepted");

    let trip_update = decoded.entity[0]
        .trip_update
        .as_ref()
        .expect("trip_update present");
    assert_eq!(trip_update.trip.trip_id, None);
}

// -- fields optional in proto2 but required by GTFS-Realtime v2.0 ----------

/// `FeedHeader.timestamp` is proto2-optional, and v2.0 treats it as required.
/// It arrives as `Option`, so unlike the `required` fields above, a rule *can*
/// tell absent from present. Header-timestamp rules are therefore writable
/// against the decoded message; required-field rules are not.
#[test]
fn semantically_required_optional_field_keeps_its_absence() {
    let mut header = Vec::new();
    len_delimited(1, b"2.0", &mut header);
    let mut message = Vec::new();
    len_delimited(1, &header, &mut message);

    let decoded = FeedMessage::decode(&message[..]).expect("decodes");
    assert!(decoded.header.timestamp.is_none(), "absence is observable");

    let mut with_zero = Vec::new();
    len_delimited(1, b"2.0", &mut with_zero);
    varint_field(3, 0, &mut with_zero);
    let mut message_zero = Vec::new();
    len_delimited(1, &with_zero, &mut message_zero);

    let decoded_zero = FeedMessage::decode(&message_zero[..]).expect("decodes");
    assert_eq!(
        decoded_zero.header.timestamp,
        Some(0),
        "zero is not absence"
    );
}

// -- unknown fields and extensions ----------------------------------------

/// Unknown fields decode without error but are **dropped**: `prost` keeps no
/// unknown-field set, so re-encoding does not reproduce the input.
///
/// This settles the open question in GTF-11's Core API section: a content
/// fingerprint for E017 must be taken over the raw received bytes. Hashing a
/// re-encoded message would treat two genuinely different snapshots as
/// identical.
#[test]
fn unknown_fields_decode_but_are_dropped_on_reencode() {
    let mut header = valid_header_bytes();
    varint_field(999, 42, &mut header); // field number not in the schema
    let mut message = Vec::new();
    len_delimited(1, &header, &mut message);

    let decoded = FeedMessage::decode(&message[..]).expect("unknown fields do not fail decoding");
    assert_eq!(decoded.header.gtfs_realtime_version, "2.0");

    let reencoded = decoded.encode_to_vec();
    assert!(
        reencoded.len() < message.len(),
        "re-encoding is shorter: the unknown field is gone"
    );
    assert_ne!(reencoded, message);
}

/// Extension ranges behave the same way. The schema reserves 1000-1999 and
/// 9000-9999 on most messages, and producers such as MTA/NYCT put real data
/// there. `prost` has no proto2 extension support, so that data is invisible
/// to validation and lost on re-encode.
///
/// The feed still validates -- extension-bearing input is not rejected -- but
/// no selected rule may depend on extension content.
#[test]
fn extension_range_fields_decode_but_are_dropped() {
    let mut entity = Vec::new();
    len_delimited(1, b"entity-1", &mut entity);
    varint_field(1000, 7, &mut entity); // inside the reserved extension range

    let message = feed_message_bytes(&[entity]);
    let decoded = FeedMessage::decode(&message[..]).expect("extension data does not fail decoding");

    assert_eq!(decoded.entity.len(), 1);
    assert_eq!(decoded.entity[0].id, "entity-1");
    assert!(
        decoded.encode_to_vec().len() < message.len(),
        "extension dropped"
    );
}

/// Unknown enum values survive as raw integers rather than failing or being
/// coerced to a default, so a rule can detect and report them. `incrementality`
/// is generated as `Option<i32>`, not as an enum type.
///
/// The pinned Java bindings differ: proto2 moves an unrecognised enum value
/// into the unknown-field set, so `hasIncrementality()` returns `false` and the
/// field reads as *absent*. Rust sees "present but invalid", Java sees
/// "absent". Any rule over `incrementality` must pick one reading deliberately
/// and record the choice as an approved delta.
#[test]
fn unknown_enum_values_are_preserved_as_integers() {
    let mut header = valid_header_bytes();
    varint_field(2, 99, &mut header); // no such Incrementality variant
    let mut message = Vec::new();
    len_delimited(1, &header, &mut message);

    let decoded = FeedMessage::decode(&message[..]).expect("unknown enum does not fail decoding");
    assert_eq!(decoded.header.incrementality, Some(99));
    assert!(
        feed_header::Incrementality::try_from(99).is_err(),
        "and it is not a defined variant"
    );
}

// -- content identity ------------------------------------------------------

/// Decoded equality is **not** content identity. Two messages differing only in
/// unknown or extension bytes compare equal, because the distinguishing bytes
/// were dropped at decode time.
///
/// E017 (feed identical to the previous snapshot) must therefore fingerprint
/// the raw input, never the decoded value.
#[test]
fn decoded_equality_is_not_content_identity() {
    let plain = {
        let mut message = Vec::new();
        len_delimited(1, &valid_header_bytes(), &mut message);
        message
    };
    let with_extension = {
        let mut header = valid_header_bytes();
        varint_field(1000, 7, &mut header);
        let mut message = Vec::new();
        len_delimited(1, &header, &mut message);
        message
    };

    assert_ne!(plain, with_extension, "the bytes differ");
    assert_eq!(
        FeedMessage::decode(&plain[..]).unwrap(),
        FeedMessage::decode(&with_extension[..]).unwrap(),
        "the decoded messages do not"
    );
}

// -- malformed and truncated input ----------------------------------------

/// Truncation is caught, and the error names the field being read, which is
/// worth surfacing in the decode-failure notice.
#[test]
fn truncated_input_fails_with_field_context() {
    let mut message = Vec::new();
    len_delimited(1, &valid_header_bytes(), &mut message);
    let truncated = &message[..message.len() - 2];

    let error = FeedMessage::decode(truncated).expect_err("truncated input fails");
    let rendered = format!("{error:?}");
    assert!(rendered.contains("BufferUnderflow"), "got {rendered}");
    assert!(
        rendered.contains("header"),
        "error names the field: {rendered}"
    );
}

/// A length prefix claiming more bytes than remain is rejected.
#[test]
fn lying_length_prefix_fails() {
    let mut message = Vec::new();
    tag(1, 2, &mut message);
    varint(200, &mut message); // claims 200 bytes
    message.extend_from_slice(b"2.0"); // supplies 3

    assert!(FeedMessage::decode(&message[..]).is_err());
}

/// An unsupported wire type for a known field is rejected rather than skipped.
#[test]
fn wrong_wire_type_for_known_field_fails() {
    let mut message = Vec::new();
    varint_field(1, 5, &mut message); // header as a varint, not a submessage

    assert!(FeedMessage::decode(&message[..]).is_err());
}

// -- entity payloads -------------------------------------------------------

/// A single FeedEntity may carry several payloads at once. All of them stay
/// visible, which is why GTF-11 forbids modelling the payload as an exclusive
/// Rust enum: collapsing it would hide a malformed entity from the rules meant
/// to report it.
#[test]
fn entity_with_multiple_payloads_keeps_all_of_them() {
    let trip_update = {
        let mut descriptor = Vec::new();
        len_delimited(1, b"trip-1", &mut descriptor);
        let mut payload = Vec::new();
        len_delimited(1, &descriptor, &mut payload);
        payload
    };
    let vehicle_position = {
        let mut position = Vec::new();
        float_field(1, 47.0, &mut position);
        float_field(2, 8.0, &mut position);
        let mut payload = Vec::new();
        len_delimited(2, &position, &mut payload);
        payload
    };

    let mut entity = Vec::new();
    len_delimited(1, b"multi", &mut entity);
    len_delimited(3, &trip_update, &mut entity);
    len_delimited(4, &vehicle_position, &mut entity);
    len_delimited(5, &[], &mut entity); // an Alert too

    let message = feed_message_bytes(&[entity]);
    let decoded = FeedMessage::decode(&message[..]).expect("decodes");

    let entity = &decoded.entity[0];
    assert!(entity.trip_update.is_some());
    assert!(entity.vehicle.is_some());
    assert!(entity.alert.is_some());
}

/// Each supported entity type decodes on its own with its fields intact.
#[test]
fn each_entity_type_decodes() {
    let mut descriptor = Vec::new();
    len_delimited(1, b"trip-1", &mut descriptor);
    let mut trip_update = Vec::new();
    len_delimited(1, &descriptor, &mut trip_update);
    let mut trip_entity = Vec::new();
    len_delimited(1, b"tu", &mut trip_entity);
    len_delimited(3, &trip_update, &mut trip_entity);

    let mut position = Vec::new();
    float_field(1, 47.5, &mut position);
    float_field(2, 8.5, &mut position);
    let mut vehicle = Vec::new();
    len_delimited(2, &position, &mut vehicle);
    let mut vehicle_entity = Vec::new();
    len_delimited(1, b"vp", &mut vehicle_entity);
    len_delimited(4, &vehicle, &mut vehicle_entity);

    let mut alert = Vec::new();
    varint_field(6, 2, &mut alert); // cause
    let mut alert_entity = Vec::new();
    len_delimited(1, b"al", &mut alert_entity);
    len_delimited(5, &alert, &mut alert_entity);

    let message = feed_message_bytes(&[trip_entity, vehicle_entity, alert_entity]);
    let decoded = FeedMessage::decode(&message[..]).expect("decodes");

    assert_eq!(decoded.entity.len(), 3);
    assert_eq!(
        decoded.entity[0].trip_update.as_ref().unwrap().trip.trip_id,
        Some("trip-1".to_string())
    );
    let vehicle_position = decoded.entity[1].vehicle.as_ref().unwrap();
    assert_eq!(vehicle_position.position.as_ref().unwrap().latitude, 47.5);
    assert_eq!(decoded.entity[2].alert.as_ref().unwrap().cause, Some(2));
}

// -- size --------------------------------------------------------------------

/// `prost` applies no size ceiling of its own: a large message decodes, and
/// allocation tracks the input. Bounding input is therefore the adapter's job
/// (CLI/URL fetch), as GTF-11 specifies -- not something the decoder provides.
#[test]
fn large_messages_decode_without_a_built_in_limit() {
    let entities: Vec<Vec<u8>> = (0..50_000)
        .map(|index| {
            let mut entity = Vec::new();
            len_delimited(1, format!("entity-{index}").as_bytes(), &mut entity);
            entity
        })
        .collect();

    let message = feed_message_bytes(&entities);
    assert!(message.len() > 500_000, "a non-trivial payload");

    let decoded = FeedMessage::decode(&message[..]).expect("no built-in ceiling");
    assert_eq!(decoded.entity.len(), 50_000);
}

// -- fixtures for the Java cross-check ------------------------------------

/// Writes the byte fixtures that `scripts/rt_parity/decoder_java_check.java`
/// replays through the pinned `gtfs-realtime-bindings:0.0.4`, to record where
/// the two decoders disagree.
///
/// Ignored by default: CI has no JDK, and this writes files.
///
/// ```text
/// cargo test -p gtfs-guru-rt --test decoder -- --ignored dump_fixtures
/// ```
#[test]
#[ignore = "writes fixture files for the optional Java cross-check"]
fn dump_fixtures() {
    let mut header_without_version = Vec::new();
    varint_field(3, 1_700_000_000, &mut header_without_version);
    let mut missing_version = Vec::new();
    len_delimited(1, &header_without_version, &mut missing_version);

    let mut header_unknown = valid_header_bytes();
    varint_field(999, 42, &mut header_unknown);
    let mut unknown_field = Vec::new();
    len_delimited(1, &header_unknown, &mut unknown_field);

    let mut extension_entity = Vec::new();
    len_delimited(1, b"entity-1", &mut extension_entity);
    varint_field(1000, 7, &mut extension_entity);

    let mut header_unknown_enum = valid_header_bytes();
    varint_field(2, 99, &mut header_unknown_enum);
    let mut unknown_enum = Vec::new();
    len_delimited(1, &header_unknown_enum, &mut unknown_enum);

    let mut trip_update_no_trip = Vec::new();
    len_delimited(1, b"entity-1", &mut trip_update_no_trip);
    len_delimited(3, &[], &mut trip_update_no_trip);

    let fixtures: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("missing_required_header", {
            let mut message = Vec::new();
            len_delimited(2, &[], &mut message);
            message
        }),
        ("missing_required_version", missing_version),
        (
            "missing_required_nested_trip",
            feed_message_bytes(&[trip_update_no_trip]),
        ),
        ("unknown_field", unknown_field),
        ("extension_field", feed_message_bytes(&[extension_entity])),
        ("unknown_enum", unknown_enum),
        ("valid_minimal", {
            let mut message = Vec::new();
            len_delimited(1, &valid_header_bytes(), &mut message);
            message
        }),
    ];

    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/rt-decoder-fixtures");
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    for (name, bytes) in &fixtures {
        std::fs::write(dir.join(format!("{name}.pb")), bytes).expect("write fixture");
    }
    println!("wrote {} fixtures to {}", fixtures.len(), dir.display());
}
