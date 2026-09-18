//! One decoded GTFS-Realtime message, with the facts about its arrival that
//! rules are allowed to see.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::canonical_decode;
use crate::transit_realtime::FeedMessage;

/// Default ceiling on a single RT message.
///
/// Raw `prost` decoding imposes no input ceiling -- a 50k-entity message decodes
/// with allocation tracking the input (see `tests/decoder.rs`) -- so the bound
/// lives here, at the edge where bytes enter. Real snapshots are a few
/// megabytes; this is deliberately generous while still refusing a hostile
/// payload.
///
/// Override with `GTFS_VALIDATOR_MAX_RT_BYTES`, matching the convention of the
/// Schedule reader's `GTFS_VALIDATOR_MAX_MEMBER_BYTES`.
const DEFAULT_MAX_RT_BYTES: u64 = 64 * 1024 * 1024;

pub fn max_rt_bytes() -> u64 {
    std::env::var("GTFS_VALIDATOR_MAX_RT_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MAX_RT_BYTES)
}

/// Where a message came from. Recorded in the report so a stored result names
/// its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtSource {
    File(PathBuf),
    Url(String),
    /// Bytes supplied directly, by a test or an embedded fixture.
    Bytes,
}

impl fmt::Display for RtSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtSource::File(path) => write!(formatter, "{}", path.display()),
            RtSource::Url(url) => write!(formatter, "{url}"),
            RtSource::Bytes => write!(formatter, "<bytes>"),
        }
    }
}

/// SHA-256 over the bytes **as received**, before decoding.
///
/// It must be taken over the raw input rather than over the decoded message or
/// a re-encoding of it. The Java-compatibility boundary deliberately removes
/// fields the old bindings cannot observe, and generated `prost` values keep no
/// unknown-field set. Two snapshots differing only in extension data therefore
/// compare equal as decoded values; `tests/decoder.rs` pins that behavior.
///
/// The bytes themselves are not retained. GTF-11 asks for no extra full input
/// buffer without a demonstrated need, and a digest is what the duplicate-
/// detection rules (E017, Phase 7) actually require.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentFingerprint([u8; 32]);

impl ContentFingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ContentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ContentFingerprint({self})")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RtDecodeError {
    #[error("malformed protobuf at {path}: {detail}")]
    Malformed { path: String, detail: String },

    #[error("message missing required fields: {fields}")]
    MissingRequired { fields: String },

    #[error("protobuf recursion limit {limit} exceeded at {path}")]
    RecursionLimit { path: String, limit: usize },

    #[error("Java-compatible protobuf normalization exceeded {limit} bytes at {path}")]
    NormalizationLimit { path: String, limit: usize },

    #[error("could not decode Java-compatible realtime message: {0}")]
    Prost(#[source] prost::DecodeError),
}

#[derive(Debug, thiserror::Error)]
pub enum RtFeedError {
    #[error("realtime message is at least {actual} bytes, over the {limit} byte limit")]
    TooLarge { actual: u64, limit: u64 },

    #[error("could not read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A load failure, not a validation notice. No ordinary rule runs when the
    /// Java 0.0.4 bindings would reject the message.
    #[error(
        "could not decode realtime message from {input} ({encoded_len} bytes, sha256 {content_fingerprint}): {error}"
    )]
    Decode {
        input: RtSource,
        encoded_len: usize,
        content_fingerprint: ContentFingerprint,
        #[source]
        error: RtDecodeError,
    },
}

/// A decoded message together with its provenance.
#[derive(Debug, Clone)]
pub struct RtFeed {
    message: FeedMessage,
    source: RtSource,
    /// Size of the received payload in bytes, before decoding.
    encoded_len: usize,
    content_fingerprint: ContentFingerprint,
}

impl RtFeed {
    /// Fingerprint, then decode. The hash is taken first so it always describes
    /// what arrived, even when decoding later fails.
    pub fn from_bytes(bytes: &[u8], source: RtSource) -> Result<Self, RtFeedError> {
        Self::from_bytes_with_limit(bytes, source, max_rt_bytes())
    }

    /// As [`RtFeed::from_bytes`], with the ceiling supplied rather than read
    /// from the environment. The CLI and URL adapters pass their own limit.
    pub fn from_bytes_with_limit(
        bytes: &[u8],
        source: RtSource,
        limit: u64,
    ) -> Result<Self, RtFeedError> {
        if bytes.len() as u64 > limit {
            return Err(RtFeedError::TooLarge {
                actual: bytes.len() as u64,
                limit,
            });
        }

        let content_fingerprint = ContentFingerprint::of(bytes);
        let message = canonical_decode::decode(bytes).map_err(|error| RtFeedError::Decode {
            input: source.clone(),
            encoded_len: bytes.len(),
            content_fingerprint,
            error,
        })?;

        Ok(Self {
            message,
            source,
            encoded_len: bytes.len(),
            content_fingerprint,
        })
    }

    /// Read a local `.pb` file without ever reading more than the configured
    /// ceiling plus one byte.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, RtFeedError> {
        Self::from_path_with_limit(path, max_rt_bytes())
    }

    /// As [`RtFeed::from_path`], with an explicit ceiling.
    pub fn from_path_with_limit(path: impl AsRef<Path>, limit: u64) -> Result<Self, RtFeedError> {
        let path = path.as_ref();

        // Check the size from metadata first: reading then checking would mean
        // allocating the very payload the limit exists to refuse.
        let metadata = std::fs::metadata(path).map_err(|source| RtFeedError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if metadata.len() > limit {
            return Err(RtFeedError::TooLarge {
                actual: metadata.len(),
                limit,
            });
        }

        let mut file = File::open(path).map_err(|source| RtFeedError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        // Metadata avoids allocating for an already-large file. `take` is the
        // actual safety boundary: the file may grow after metadata was read.
        let bytes =
            read_bounded(&mut file, metadata.len(), limit).map_err(|source| RtFeedError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes.len() as u64 > limit {
            return Err(RtFeedError::TooLarge {
                actual: bytes.len() as u64,
                limit,
            });
        }

        Self::from_bytes_with_limit(&bytes, RtSource::File(path.to_path_buf()), limit)
    }

    /// Entities in the order the producer sent them.
    pub fn entities(&self) -> &[crate::transit_realtime::FeedEntity] {
        &self.message.entity
    }

    /// The Java-compatible message seen by validation rules.
    pub fn message(&self) -> &FeedMessage {
        &self.message
    }

    pub fn source(&self) -> &RtSource {
        &self.source
    }

    pub fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    pub fn content_fingerprint(&self) -> ContentFingerprint {
        self.content_fingerprint
    }
}

fn read_bounded(reader: &mut impl Read, expected_len: u64, limit: u64) -> std::io::Result<Vec<u8>> {
    let capacity = expected_len.min(limit).min(usize::MAX as u64) as usize;
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Seek};

    use super::read_bounded;

    #[test]
    fn bounded_reader_stops_after_the_limit_plus_one_byte() {
        let mut input = Cursor::new(vec![0; 100]);

        let bytes = read_bounded(&mut input, 0, 4).expect("read succeeds");

        assert_eq!(bytes.len(), 5);
        assert_eq!(input.stream_position().unwrap(), 5);
    }
}
