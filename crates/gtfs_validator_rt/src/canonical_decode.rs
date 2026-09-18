//! Compatibility decoding for the protobuf implementation embedded in the
//! pinned Java validator.
//!
//! The public Rust model is generated from the current schema. Before it sees
//! bytes, this module applies the older `gtfs-realtime-bindings:0.0.4` schema:
//! unknown fields and enum values are ignored, wrong-wire occurrences stay
//! unknown, proto2 strings use Java's lossy getter view, and required fields are
//! checked after protobuf merge semantics have been applied.

use std::sync::OnceLock;

use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};

use crate::feed::RtDecodeError;
use crate::transit_realtime::FeedMessage;

const JAVA_DESCRIPTOR_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/gtfs-realtime-java-0.0.4.bin"));
const FEED_MESSAGE_NAME: &str = "transit_realtime.FeedMessage";
const JAVA_RECURSION_LIMIT: usize = 64;
const REPLACEMENT_CHARACTER_UTF8: &[u8] = &[0xef, 0xbf, 0xbd];

pub(crate) fn decode(bytes: &[u8]) -> Result<FeedMessage, RtDecodeError> {
    let descriptor = java_feed_message_descriptor();
    // Java replacement characters can expand one invalid byte to three UTF-8
    // bytes. No other normalization expands more, so this bounds the copy.
    let normalized_limit = bytes.len().saturating_mul(3);
    let normalized = normalize_message(
        bytes,
        &descriptor,
        descriptor.full_name(),
        0,
        normalized_limit,
    )?;

    let dynamic =
        DynamicMessage::decode(descriptor, normalized.as_slice()).map_err(RtDecodeError::Prost)?;
    let mut missing = Vec::new();
    collect_missing_required(&dynamic, FEED_MESSAGE_NAME, &mut missing);
    if !missing.is_empty() {
        return Err(RtDecodeError::MissingRequired {
            fields: missing.join(", "),
        });
    }
    // Avoid retaining both decoded representations at once. The dynamic value
    // exists only to apply old-schema required-field semantics.
    drop(dynamic);

    FeedMessage::decode(normalized.as_slice()).map_err(RtDecodeError::Prost)
}

fn java_feed_message_descriptor() -> MessageDescriptor {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();

    POOL.get_or_init(|| {
        DescriptorPool::decode(JAVA_DESCRIPTOR_BYTES)
            .expect("build-generated Java 0.0.4 descriptor must be valid")
    })
    .get_message_by_name(FEED_MESSAGE_NAME)
    .expect("Java 0.0.4 descriptor must define FeedMessage")
}

fn collect_missing_required(message: &DynamicMessage, path: &str, missing: &mut Vec<String>) {
    for field in message.descriptor().fields() {
        let field_path = format!("{path}.{}", field.name());
        let is_present = message.has_field(&field);

        if field.is_required() && !is_present {
            missing.push(field_path.clone());
        }
        if !is_present {
            continue;
        }

        let value = message.get_field(&field);
        match value.as_ref() {
            Value::Message(nested) => collect_missing_required(nested, &field_path, missing),
            Value::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    if let Value::Message(nested) = value {
                        collect_missing_required(
                            nested,
                            &format!("{field_path}[{index}]"),
                            missing,
                        );
                    }
                }
            }
            Value::Map(values) => {
                for (key, value) in values {
                    if let Value::Message(nested) = value {
                        collect_missing_required(
                            nested,
                            &format!("{field_path}[{key:?}]"),
                            missing,
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn normalize_message(
    input: &[u8],
    descriptor: &MessageDescriptor,
    path: &str,
    depth: usize,
    output_limit: usize,
) -> Result<Vec<u8>, RtDecodeError> {
    let mut output = Vec::with_capacity(input.len().min(output_limit));
    normalize_message_into(input, descriptor, path, depth, output_limit, &mut output)?;
    Ok(output)
}

fn normalize_message_into(
    input: &[u8],
    descriptor: &MessageDescriptor,
    path: &str,
    depth: usize,
    output_limit: usize,
    output: &mut Vec<u8>,
) -> Result<(), RtDecodeError> {
    let mut cursor = 0;

    while cursor < input.len() {
        let field_start = cursor;
        let key = read_java_varint32(input, &mut cursor, path)?;
        let number = key >> 3;
        if number == 0 {
            return Err(malformed(
                path,
                format!("invalid field number 0 at byte {field_start}"),
            ));
        }

        let wire = WireType::try_from((key & 0x07) as u8).map_err(|wire| {
            malformed(
                path,
                format!("invalid wire type {wire} at byte {field_start}"),
            )
        })?;
        if wire == WireType::EndGroup {
            return Err(malformed(
                path,
                format!("unexpected end-group tag at byte {field_start}"),
            ));
        }

        let Some(field) = descriptor.get_field(number) else {
            skip_value(input, &mut cursor, number, wire, path, depth)?;
            continue;
        };
        let kind = field.kind();
        if !accepts_wire_type(&field, &kind, wire) {
            skip_value(input, &mut cursor, number, wire, path, depth)?;
            continue;
        }

        normalize_known_field(
            input,
            &mut cursor,
            number,
            wire,
            &field,
            kind,
            path,
            depth,
            output_limit,
            output,
        )?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn normalize_known_field(
    input: &[u8],
    cursor: &mut usize,
    number: u32,
    wire: WireType,
    field: &FieldDescriptor,
    kind: Kind,
    path: &str,
    depth: usize,
    output_limit: usize,
    output: &mut Vec<u8>,
) -> Result<(), RtDecodeError> {
    match wire {
        WireType::Varint => {
            if let Some(value) = normalize_varint(input, cursor, &kind, path)? {
                write_varint_field(number, value, output, output_limit, path)?;
            }
        }
        WireType::SixtyFourBit => {
            let value = read_fixed(input, cursor, 8, path)?;
            write_fixed_field(number, wire, value, output, output_limit, path)?;
        }
        WireType::LengthDelimited if field.is_list() && is_packable(&kind) => {
            let payload = read_length_delimited(input, cursor, path)?;
            let payload_start = output.len();
            normalize_packed_into(payload, &kind, path, output_limit, output)?;
            if output.len() != payload_start {
                insert_length_delimited_prefix(number, payload_start, output, output_limit, path)?;
            }
        }
        WireType::LengthDelimited => {
            let payload = read_length_delimited(input, cursor, path)?;
            match kind {
                Kind::Message(nested) => {
                    let nested_depth = enter_recursion(depth, path)?;
                    let nested_path = format!("{path}.{}", field.name());
                    let payload_start = output.len();
                    normalize_message_into(
                        payload,
                        &nested,
                        &nested_path,
                        nested_depth,
                        output_limit,
                        output,
                    )?;
                    insert_length_delimited_prefix(
                        number,
                        payload_start,
                        output,
                        output_limit,
                        path,
                    )?;
                }
                Kind::String => {
                    write_java_string(number, payload, output, output_limit, path)?;
                }
                Kind::Bytes => {
                    write_length_delimited(number, payload, output, output_limit, path)?;
                }
                _ => return Err(malformed(path, "unexpected length-delimited field kind")),
            }
        }
        WireType::StartGroup => {
            // The Java 0.0.4 schema defines no group fields. Keep this safe if a
            // future compatibility descriptor ever introduces one.
            skip_group(input, cursor, number, path, enter_recursion(depth, path)?)?;
        }
        WireType::ThirtyTwoBit => {
            let value = read_fixed(input, cursor, 4, path)?;
            write_fixed_field(number, wire, value, output, output_limit, path)?;
        }
        WireType::EndGroup => return Err(malformed(path, "unexpected end-group tag")),
    }

    Ok(())
}

fn normalize_varint(
    input: &[u8],
    cursor: &mut usize,
    kind: &Kind,
    path: &str,
) -> Result<Option<u64>, RtDecodeError> {
    let value = match kind {
        Kind::Int32 => (read_java_varint32(input, cursor, path)? as i32 as i64) as u64,
        Kind::Uint32 | Kind::Sint32 => u64::from(read_java_varint32(input, cursor, path)?),
        Kind::Enum(enumeration) => {
            let value = read_java_varint32(input, cursor, path)? as i32;
            return Ok(enumeration.get_value(value).map(|_| (value as i64) as u64));
        }
        Kind::Int64 | Kind::Uint64 | Kind::Sint64 => read_java_varint64(input, cursor, path)?,
        Kind::Bool => u64::from(read_java_varint64(input, cursor, path)? != 0),
        _ => return Err(malformed(path, "unexpected varint field kind")),
    };

    Ok(Some(value))
}

fn normalize_packed_into(
    input: &[u8],
    kind: &Kind,
    path: &str,
    output_limit: usize,
    output: &mut Vec<u8>,
) -> Result<(), RtDecodeError> {
    let mut cursor = 0;

    while cursor < input.len() {
        match kind {
            Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => {
                let value = read_fixed(input, &mut cursor, 8, path)?;
                extend_output(output, value, output_limit, path)?;
            }
            Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => {
                let value = read_fixed(input, &mut cursor, 4, path)?;
                extend_output(output, value, output_limit, path)?;
            }
            _ => {
                if let Some(value) = normalize_varint(input, &mut cursor, kind, path)? {
                    write_varint(value, output, output_limit, path)?;
                }
            }
        }
    }

    Ok(())
}

fn accepts_wire_type(field: &FieldDescriptor, kind: &Kind, wire: WireType) -> bool {
    if field.is_group() {
        return wire == WireType::StartGroup;
    }

    wire == scalar_wire_type(kind)
        || (field.is_list() && wire == WireType::LengthDelimited && is_packable(kind))
}

fn scalar_wire_type(kind: &Kind) -> WireType {
    match kind {
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => WireType::SixtyFourBit,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => WireType::ThirtyTwoBit,
        Kind::Int32
        | Kind::Int64
        | Kind::Uint32
        | Kind::Uint64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Bool
        | Kind::Enum(_) => WireType::Varint,
        Kind::String | Kind::Bytes | Kind::Message(_) => WireType::LengthDelimited,
    }
}

fn is_packable(kind: &Kind) -> bool {
    !matches!(kind, Kind::String | Kind::Bytes | Kind::Message(_))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireType {
    Varint,
    SixtyFourBit,
    LengthDelimited,
    StartGroup,
    EndGroup,
    ThirtyTwoBit,
}

impl TryFrom<u8> for WireType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Varint),
            1 => Ok(Self::SixtyFourBit),
            2 => Ok(Self::LengthDelimited),
            3 => Ok(Self::StartGroup),
            4 => Ok(Self::EndGroup),
            5 => Ok(Self::ThirtyTwoBit),
            other => Err(other),
        }
    }
}

fn skip_value(
    input: &[u8],
    cursor: &mut usize,
    field_number: u32,
    wire: WireType,
    path: &str,
    depth: usize,
) -> Result<(), RtDecodeError> {
    match wire {
        WireType::Varint => {
            read_java_varint64(input, cursor, path)?;
        }
        WireType::SixtyFourBit => {
            read_fixed(input, cursor, 8, path)?;
        }
        WireType::LengthDelimited => {
            read_length_delimited(input, cursor, path)?;
        }
        WireType::StartGroup => {
            skip_group(
                input,
                cursor,
                field_number,
                path,
                enter_recursion(depth, path)?,
            )?;
        }
        WireType::EndGroup => return Err(malformed(path, "unexpected end-group tag")),
        WireType::ThirtyTwoBit => {
            read_fixed(input, cursor, 4, path)?;
        }
    }

    Ok(())
}

fn skip_group(
    input: &[u8],
    cursor: &mut usize,
    group_number: u32,
    path: &str,
    depth: usize,
) -> Result<(), RtDecodeError> {
    loop {
        if *cursor == input.len() {
            return Err(malformed(path, "unterminated group"));
        }

        let key = read_java_varint32(input, cursor, path)?;
        let number = key >> 3;
        if number == 0 {
            return Err(malformed(path, "invalid field number inside group"));
        }
        let wire = WireType::try_from((key & 0x07) as u8)
            .map_err(|wire| malformed(path, format!("invalid wire type {wire} inside group")))?;

        if wire == WireType::EndGroup {
            if number == group_number {
                return Ok(());
            }
            return Err(malformed(path, "mismatched end-group tag"));
        }

        skip_value(input, cursor, number, wire, path, depth)?;
    }
}

fn enter_recursion(depth: usize, path: &str) -> Result<usize, RtDecodeError> {
    if depth >= JAVA_RECURSION_LIMIT {
        return Err(RtDecodeError::RecursionLimit {
            path: path.to_string(),
            limit: JAVA_RECURSION_LIMIT,
        });
    }
    Ok(depth + 1)
}

fn read_fixed<'a>(
    input: &'a [u8],
    cursor: &mut usize,
    width: usize,
    path: &str,
) -> Result<&'a [u8], RtDecodeError> {
    let end = cursor
        .checked_add(width)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| malformed(path, "truncated fixed-width field"))?;
    let value = &input[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn read_length_delimited<'a>(
    input: &'a [u8],
    cursor: &mut usize,
    path: &str,
) -> Result<&'a [u8], RtDecodeError> {
    let raw_length = read_java_varint32(input, cursor, path)?;
    if (raw_length as i32) < 0 {
        return Err(malformed(path, "negative length"));
    }
    let length = raw_length as usize;
    let end = cursor
        .checked_add(length)
        .filter(|&end| end <= input.len())
        .ok_or_else(|| malformed(path, "truncated length-delimited field"))?;
    let payload = &input[*cursor..end];
    *cursor = end;
    Ok(payload)
}

/// Java's `readRawVarint32`: consume up to ten bytes and discard upper bits.
fn read_java_varint32(input: &[u8], cursor: &mut usize, path: &str) -> Result<u32, RtDecodeError> {
    let start = *cursor;
    let mut value = 0_u32;

    for index in 0..10 {
        let Some(&byte) = input.get(*cursor) else {
            return Err(malformed(path, format!("truncated varint at byte {start}")));
        };
        *cursor += 1;

        if index < 5 {
            value |= u32::from(byte & 0x7f) << (index * 7);
        }
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }

    Err(malformed(
        path,
        format!("varint exceeds ten bytes at byte {start}"),
    ))
}

/// Java protobuf 2.6.1's byte-array fast path uses the tenth byte only to
/// terminate the varint. Reaching it contributes bit 63 regardless of that
/// byte's payload; the preceding nine bytes supply bits 0 through 62.
fn read_java_varint64(input: &[u8], cursor: &mut usize, path: &str) -> Result<u64, RtDecodeError> {
    let start = *cursor;
    let mut value = 0_u64;

    for index in 0..10 {
        let Some(&byte) = input.get(*cursor) else {
            return Err(malformed(path, format!("truncated varint at byte {start}")));
        };
        *cursor += 1;

        if index == 9 {
            value |= 1_u64 << 63;
        } else {
            value |= u64::from(byte & 0x7f) << (index * 7);
        }
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }

    Err(malformed(
        path,
        format!("varint exceeds ten bytes at byte {start}"),
    ))
}

fn write_varint_field(
    field_number: u32,
    value: u64,
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    let key = u64::from(field_number) << 3;
    ensure_output(
        output,
        varint_len(key) + varint_len(value),
        output_limit,
        path,
    )?;
    write_varint_unchecked(key, output);
    write_varint_unchecked(value, output);
    Ok(())
}

fn write_fixed_field(
    field_number: u32,
    wire: WireType,
    value: &[u8],
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    let wire = match wire {
        WireType::SixtyFourBit => 1,
        WireType::ThirtyTwoBit => 5,
        _ => return Err(malformed(path, "unexpected fixed-width wire type")),
    };
    let key = (u64::from(field_number) << 3) | wire;
    ensure_output(output, varint_len(key) + value.len(), output_limit, path)?;
    write_varint_unchecked(key, output);
    output.extend_from_slice(value);
    Ok(())
}

fn insert_length_delimited_prefix(
    field_number: u32,
    payload_start: usize,
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    let payload_len = output.len() - payload_start;
    let mut prefix = [0_u8; 20];
    let key = (u64::from(field_number) << 3) | 2;
    let key_len = encode_varint(key, &mut prefix);
    let length_len = encode_varint(payload_len as u64, &mut prefix[key_len..]);
    let prefix_len = key_len + length_len;

    ensure_output(output, prefix_len, output_limit, path)?;
    let old_len = output.len();
    output.resize(old_len + prefix_len, 0);
    output.copy_within(payload_start..old_len, payload_start + prefix_len);
    output[payload_start..payload_start + prefix_len].copy_from_slice(&prefix[..prefix_len]);
    Ok(())
}

fn write_length_delimited(
    field_number: u32,
    payload: &[u8],
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    let key = (u64::from(field_number) << 3) | 2;
    ensure_output(
        output,
        varint_len(key) + varint_len(payload.len() as u64) + payload.len(),
        output_limit,
        path,
    )?;
    write_varint_unchecked(key, output);
    write_varint_unchecked(payload.len() as u64, output);
    output.extend_from_slice(payload);
    Ok(())
}

fn write_java_string(
    field_number: u32,
    payload: &[u8],
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    let normalized_len = java_utf8_normalized_len(payload)?;
    let key = (u64::from(field_number) << 3) | 2;
    ensure_output(
        output,
        varint_len(key) + varint_len(normalized_len as u64) + normalized_len,
        output_limit,
        path,
    )?;
    write_varint_unchecked(key, output);
    write_varint_unchecked(normalized_len as u64, output);
    write_java_utf8(payload, output);
    Ok(())
}

fn extend_output(
    output: &mut Vec<u8>,
    bytes: &[u8],
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    ensure_output(output, bytes.len(), output_limit, path)?;
    output.extend_from_slice(bytes);
    Ok(())
}

fn write_varint(
    value: u64,
    output: &mut Vec<u8>,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    ensure_output(output, varint_len(value), output_limit, path)?;
    write_varint_unchecked(value, output);
    Ok(())
}

fn ensure_output(
    output: &[u8],
    additional: usize,
    output_limit: usize,
    path: &str,
) -> Result<(), RtDecodeError> {
    if output
        .len()
        .checked_add(additional)
        .is_none_or(|length| length > output_limit)
    {
        return Err(RtDecodeError::NormalizationLimit {
            path: path.to_string(),
            limit: output_limit,
        });
    }
    Ok(())
}

fn varint_len(value: u64) -> usize {
    ((64 - value.leading_zeros()).max(1) as usize).div_ceil(7)
}

fn write_varint_unchecked(mut value: u64, output: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            return;
        }
        output.push(byte | 0x80);
    }
}

fn encode_varint(mut value: u64, output: &mut [u8]) -> usize {
    let mut cursor = 0;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        output[cursor] = if value == 0 { byte } else { byte | 0x80 };
        cursor += 1;
        if value == 0 {
            return cursor;
        }
    }
}

fn java_utf8_normalized_len(input: &[u8]) -> Result<usize, RtDecodeError> {
    let mut cursor = 0;
    let mut length = 0_usize;
    while cursor < input.len() {
        let (consumed, valid) = java_utf8_step(&input[cursor..]);
        cursor += consumed;
        length = length
            .checked_add(if valid {
                consumed
            } else {
                REPLACEMENT_CHARACTER_UTF8.len()
            })
            .ok_or_else(|| malformed("protobuf string", "normalized UTF-8 length overflow"))?;
    }
    Ok(length)
}

fn write_java_utf8(input: &[u8], output: &mut Vec<u8>) {
    let mut cursor = 0;
    while cursor < input.len() {
        let (consumed, valid) = java_utf8_step(&input[cursor..]);
        if valid {
            output.extend_from_slice(&input[cursor..cursor + consumed]);
        } else {
            output.extend_from_slice(REPLACEMENT_CHARACTER_UTF8);
        }
        cursor += consumed;
    }
}

/// Return the bytes consumed by one Java UTF-8 decoder result and whether they
/// form a valid scalar. Malformed lengths follow OpenJDK's UTF-8 decoder, which
/// can replace a multi-byte malformed sequence with one U+FFFD.
fn java_utf8_step(input: &[u8]) -> (usize, bool) {
    let first = input[0];
    if first < 0x80 {
        return (1, true);
    }

    if (0xc2..=0xdf).contains(&first) {
        if input.len() < 2 {
            return (1, false);
        }
        return if is_continuation(input[1]) {
            (2, true)
        } else {
            (1, false)
        };
    }

    if (0xe0..=0xef).contains(&first) {
        if input.len() < 3 {
            if input.len() > 1
                && ((first == 0xe0 && (input[1] & 0xe0) == 0x80) || !is_continuation(input[1]))
            {
                return (1, false);
            }
            return (input.len(), false);
        }

        let second = input[1];
        let third = input[2];
        if (first == 0xe0 && (second & 0xe0) == 0x80)
            || !is_continuation(second)
            || !is_continuation(third)
        {
            let malformed_len =
                if (first == 0xe0 && (second & 0xe0) == 0x80) || !is_continuation(second) {
                    1
                } else {
                    2
                };
            return (malformed_len, false);
        }
        if first == 0xed && second >= 0xa0 {
            return (3, false);
        }
        return (3, true);
    }

    if (0xf0..=0xf7).contains(&first) {
        if input.len() < 4 {
            if first > 0xf4
                || (input.len() > 1
                    && ((first == 0xf0 && !(0x90..=0xbf).contains(&input[1]))
                        || (first == 0xf4 && !(0x80..=0x8f).contains(&input[1]))
                        || !is_continuation(input[1])))
            {
                return (1, false);
            }
            if input.len() > 2 && !is_continuation(input[2]) {
                return (2, false);
            }
            return (input.len(), false);
        }

        let second = input[1];
        let third = input[2];
        let fourth = input[3];
        let valid = first <= 0xf4
            && is_continuation(second)
            && is_continuation(third)
            && is_continuation(fourth)
            && (first != 0xf0 || second >= 0x90)
            && (first != 0xf4 || second <= 0x8f);
        if valid {
            return (4, true);
        }

        let malformed_len = if first > 0xf4
            || (first == 0xf0 && !(0x90..=0xbf).contains(&second))
            || (first == 0xf4 && !(0x80..=0x8f).contains(&second))
            || !is_continuation(second)
        {
            1
        } else if !is_continuation(third) {
            2
        } else {
            3
        };
        return (malformed_len, false);
    }

    (1, false)
}

fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

fn malformed(path: &str, detail: impl Into<String>) -> RtDecodeError {
    RtDecodeError::Malformed {
        path: path.to_string(),
        detail: detail.into(),
    }
}
