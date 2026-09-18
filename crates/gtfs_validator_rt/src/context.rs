//! One ordered pass over a snapshot's entities, and the shared facts drawn
//! from it.
//!
//! Every rule reads this instead of walking `feed.message().entity` itself. The
//! canonical Java validator re-scans the entity list once per validator -- seven
//! of its nine scan unconditionally -- and that repetition is what this context
//! exists to avoid.

use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;

use crate::feed::RtFeed;
use crate::transit_realtime::{Alert, TripUpdate, VehiclePosition};

/// Reinterpret a protobuf `uint64` as the signed `long` exposed by Java's
/// generated bindings. Canonical timestamp comparisons use this view.
pub const fn as_java_long(value: u64) -> i64 {
    value as i64
}

/// One payload, with the position and identity of the entity carrying it.
///
/// Notices have no CSV row to point at, so `entity_index` -- the payload's
/// position in the received order -- is what locates a finding in the message.
#[derive(Debug, Clone, Copy)]
pub struct RtEntityRef<'a, T> {
    pub entity_index: usize,
    /// May be empty: Java accepts an explicitly present empty required string.
    /// An absent `FeedEntity.id` is rejected by the loading boundary.
    pub entity_id: &'a str,
    pub payload: &'a T,
}

/// An `entity_id` used by more than one entity, with every position that used
/// it, ascending.
#[derive(Debug, Clone)]
pub struct DuplicateEntityId<'a> {
    pub entity_id: &'a str,
    pub indices: Vec<usize>,
}

/// The snapshot as the rules see it.
///
/// Built once, in the producer's entity order, and never rebuilt. Field order
/// within each payload list is the received order, so notices come out in a
/// stable sequence regardless of which rule produced them.
///
/// `#[non_exhaustive]` because Phase 3 adds `static_index` for the Schedule
/// cross-reference rules; construct with [`RtSnapshotContext::new`].
#[non_exhaustive]
#[derive(Debug)]
pub struct RtSnapshotContext<'a> {
    pub feed: &'a RtFeed,

    /// When the snapshot was observed.
    ///
    /// Supplied by the caller, never read from the system clock: freshness and
    /// future-timestamp rules must give the same answer for a local file, a
    /// recorded fixture, and an archived snapshot. Rules receive this value and
    /// must not call the clock themselves.
    pub observed_at: DateTime<Utc>,

    /// An entity carrying several payloads appears in each matching list, under
    /// the same `entity_index`. The protobuf uses independent optional fields,
    /// so a malformed entity really can populate more than one, and collapsing
    /// them into an exclusive enum would hide it from the rules meant to report
    /// it (`tests/decoder.rs` pins that such input decodes).
    pub trip_updates: Vec<RtEntityRef<'a, TripUpdate>>,
    pub vehicle_positions: Vec<RtEntityRef<'a, VehiclePosition>>,
    pub alerts: Vec<RtEntityRef<'a, Alert>>,

    /// Total entities received, including those with no payload this crate
    /// validates.
    pub entity_count: usize,

    /// Entities carrying none of the schema's payload fields.
    pub entities_without_payload: Vec<usize>,

    /// Entities whose explicitly present required `id` is empty.
    pub entities_without_id: Vec<usize>,

    /// Non-empty ids used more than once, in first-seen order.
    pub duplicate_entity_ids: Vec<DuplicateEntityId<'a>>,
}

impl<'a> RtSnapshotContext<'a> {
    /// One pass over the entities, preserving the received order.
    pub fn new(feed: &'a RtFeed, observed_at: DateTime<Utc>) -> Self {
        let entities = feed.entities();

        let mut trip_updates = Vec::new();
        let mut vehicle_positions = Vec::new();
        let mut alerts = Vec::new();
        let mut entities_without_payload = Vec::new();
        let mut entities_without_id = Vec::new();
        // First-seen order is kept in `occurrences`; the map only locates a
        // bucket. Nothing iterates the map, so no output depends on hash order.
        let mut bucket_of: FxHashMap<&str, usize> = FxHashMap::default();
        let mut occurrences: Vec<(&str, Vec<usize>)> = Vec::new();

        for (entity_index, entity) in entities.iter().enumerate() {
            let entity_id = entity.id.as_str();

            if entity_id.is_empty() {
                entities_without_id.push(entity_index);
            } else {
                match bucket_of.get(entity_id) {
                    Some(&bucket) => occurrences[bucket].1.push(entity_index),
                    None => {
                        bucket_of.insert(entity_id, occurrences.len());
                        occurrences.push((entity_id, vec![entity_index]));
                    }
                }
            }

            let mut carries_payload = false;

            if let Some(payload) = entity.trip_update.as_ref() {
                trip_updates.push(RtEntityRef {
                    entity_index,
                    entity_id,
                    payload,
                });
                carries_payload = true;
            }
            if let Some(payload) = entity.vehicle.as_ref() {
                vehicle_positions.push(RtEntityRef {
                    entity_index,
                    entity_id,
                    payload,
                });
                carries_payload = true;
            }
            if let Some(payload) = entity.alert.as_ref() {
                alerts.push(RtEntityRef {
                    entity_index,
                    entity_id,
                    payload,
                });
                carries_payload = true;
            }
            if !carries_payload {
                entities_without_payload.push(entity_index);
            }
        }

        let duplicate_entity_ids = occurrences
            .into_iter()
            .filter(|(_, indices)| indices.len() > 1)
            .map(|(entity_id, indices)| DuplicateEntityId { entity_id, indices })
            .collect();

        Self {
            feed,
            observed_at,
            trip_updates,
            vehicle_positions,
            alerts,
            entity_count: entities.len(),
            entities_without_payload,
            entities_without_id,
            duplicate_entity_ids,
        }
    }

    /// Whether the snapshot supplies both entity types needed by the deferred
    /// combined-feed rules W003 and E047.
    pub fn has_combined_trip_and_vehicle_entities(&self) -> bool {
        !self.trip_updates.is_empty() && !self.vehicle_positions.is_empty()
    }

    /// The header timestamp, when the producer sent one.
    pub fn header_timestamp(&self) -> Option<i64> {
        self.feed.message().header.timestamp.map(as_java_long)
    }
}
