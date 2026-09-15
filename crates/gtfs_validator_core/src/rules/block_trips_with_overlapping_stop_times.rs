//! Mirrors `BlockTripsWithOverlappingStopTimesValidator` from gtfs-validator
//! 8.0.1 step for step: the trip interval comes from the first and last
//! stop_time by stop_sequence (both need arrival and departure), intervals are
//! sorted by (first arrival, last departure), a pair whose last stop of trip A
//! equals the first stop of trip B is a block transfer and not an overlap, and
//! two trips only overlap when their services share an active date. The
//! `intersection` field is that first shared date, not a time range.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{Datelike, NaiveDate, Weekday};

use crate::{GtfsFeed, NoticeContainer, NoticeSeverity, ValidationNotice, Validator};
use gtfs_guru_model::{Calendar, ExceptionType, GtfsDate, GtfsTime, ServiceAvailability, StringId};

const CODE_BLOCK_TRIPS_WITH_OVERLAPPING_STOP_TIMES: &str =
    "block_trips_with_overlapping_stop_times";

#[derive(Debug, Default)]
pub struct BlockTripsWithOverlappingStopTimesValidator;

impl Validator for BlockTripsWithOverlappingStopTimesValidator {
    fn name(&self) -> &'static str {
        "block_trips_with_overlapping_stop_times"
    }

    fn validate(&self, feed: &GtfsFeed, notices: &mut NoticeContainer) {
        if feed.trips.rows.is_empty() || feed.stop_times.rows.is_empty() {
            return;
        }
        let service_dates = build_service_dates(feed);
        let mut intersections = ServiceIntersectionCache::new(&service_dates);
        let mut blocks: HashMap<StringId, Vec<TripInterval>> = HashMap::new();

        for (index, trip) in feed.trips.rows.iter().enumerate() {
            let row_number = feed.trips.row_number(index);
            let Some(block_id) = trip.block_id.filter(|id| id.0 != 0) else {
                continue;
            };
            let trip_id = trip.trip_id;
            let service_id = trip.service_id;
            if trip_id.0 == 0 || service_id.0 == 0 {
                continue;
            }
            // Trips without stop times are reported elsewhere. The index is
            // already ordered by stop_sequence, like Java's byTripId().
            let Some(stop_time_indices) = feed.stop_times_by_trip.get(&trip_id) else {
                continue;
            };
            let (Some(&first_index), Some(&last_index)) =
                (stop_time_indices.first(), stop_time_indices.last())
            else {
                continue;
            };
            let first = &feed.stop_times.rows[first_index];
            let last = &feed.stop_times.rows[last_index];
            let (
                Some(first_arrival),
                Some(first_departure),
                Some(last_arrival),
                Some(last_departure),
            ) = (
                first.arrival_time,
                first.departure_time,
                last.arrival_time,
                last.departure_time,
            )
            else {
                continue;
            };

            blocks.entry(block_id).or_default().push(TripInterval {
                block_id,
                trip_id,
                service_id,
                first_arrival,
                first_departure,
                last_arrival,
                last_departure,
                row_number,
            });
        }

        let mut groups: Vec<_> = blocks.into_iter().collect();
        groups.sort_by(|(left_id, _), (right_id, _)| {
            feed.pool
                .resolve(*left_id)
                .cmp(&feed.pool.resolve(*right_id))
        });
        for (_, intervals) in &mut groups {
            // Stable, like Collections.sort: ties keep trips.txt order.
            intervals.sort_by_key(|interval| {
                (
                    interval.first_arrival.total_seconds(),
                    interval.last_departure.total_seconds(),
                )
            });
        }

        for (_, intervals) in &groups {
            for i in 0..intervals.len() {
                let current = &intervals[i];
                for next in intervals.iter().skip(i + 1) {
                    // Sorted by first arrival, so nothing further down can
                    // overlap once this one starts after the current ends.
                    if current.last_departure.total_seconds() <= next.first_arrival.total_seconds()
                    {
                        break;
                    }
                    // Many agencies model a block transfer by repeating the
                    // stop_times row for both trips. Java allows that pair.
                    if current.last_arrival == next.first_arrival
                        && current.last_departure == next.first_departure
                    {
                        continue;
                    }
                    let Some(intersection) =
                        intersections.first_shared_date(current.service_id, next.service_id)
                    else {
                        continue;
                    };
                    notices.push(overlap_notice(feed, current, next, intersection));
                }
            }
        }
    }
}

fn overlap_notice(
    feed: &GtfsFeed,
    current: &TripInterval,
    next: &TripInterval,
    intersection: NaiveDate,
) -> ValidationNotice {
    let mut notice = ValidationNotice::new(
        CODE_BLOCK_TRIPS_WITH_OVERLAPPING_STOP_TIMES,
        NoticeSeverity::Error,
        "trips in the same block have overlapping stop times",
    );
    let block_id = feed.pool.resolve(current.block_id);
    let service_id_a = feed.pool.resolve(current.service_id);
    let service_id_b = feed.pool.resolve(next.service_id);
    let trip_id_a = feed.pool.resolve(current.trip_id);
    let trip_id_b = feed.pool.resolve(next.trip_id);
    notice.insert_context_field("blockId", block_id.as_str());
    notice.insert_context_field("csvRowNumberA", current.row_number);
    notice.insert_context_field("csvRowNumberB", next.row_number);
    // Java serialises GtfsDate as YYYYMMDD in notice context.
    notice.insert_context_field("intersection", intersection.format("%Y%m%d").to_string());
    notice.insert_context_field("serviceIdA", service_id_a.as_str());
    notice.insert_context_field("serviceIdB", service_id_b.as_str());
    notice.insert_context_field("tripIdA", trip_id_a.as_str());
    notice.insert_context_field("tripIdB", trip_id_b.as_str());
    notice.field_order = vec![
        "blockId".into(),
        "csvRowNumberA".into(),
        "csvRowNumberB".into(),
        "intersection".into(),
        "serviceIdA".into(),
        "serviceIdB".into(),
        "tripIdA".into(),
        "tripIdB".into(),
    ];
    notice
}

#[derive(Debug, Clone, Copy)]
struct TripInterval {
    block_id: StringId,
    trip_id: StringId,
    service_id: StringId,
    first_arrival: GtfsTime,
    first_departure: GtfsTime,
    last_arrival: GtfsTime,
    last_departure: GtfsTime,
    row_number: u64,
}

/// `ServiceIdIntersectionCache`: memoised first shared active date per
/// unordered service pair. A service with no active dates never intersects,
/// not even with itself.
struct ServiceIntersectionCache<'a> {
    service_dates: &'a HashMap<StringId, BTreeSet<NaiveDate>>,
    cache: HashMap<(StringId, StringId), Option<NaiveDate>>,
}

impl<'a> ServiceIntersectionCache<'a> {
    fn new(service_dates: &'a HashMap<StringId, BTreeSet<NaiveDate>>) -> Self {
        Self {
            service_dates,
            cache: HashMap::new(),
        }
    }

    fn first_shared_date(&mut self, left: StringId, right: StringId) -> Option<NaiveDate> {
        let key = if left.0 <= right.0 {
            (left, right)
        } else {
            (right, left)
        };
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }
        let found = match (
            self.service_dates.get(&key.0),
            self.service_dates.get(&key.1),
        ) {
            (Some(left), Some(right)) => first_intersecting_date(left, right),
            _ => None,
        };
        self.cache.insert(key, found);
        found
    }
}

/// `CalendarUtil.firstIntersectingDate`: merge-walk two sorted date sets.
fn first_intersecting_date(
    left: &BTreeSet<NaiveDate>,
    right: &BTreeSet<NaiveDate>,
) -> Option<NaiveDate> {
    let mut left_iter = left.iter().peekable();
    let mut right_iter = right.iter().peekable();
    loop {
        let (Some(a), Some(b)) = (left_iter.peek(), right_iter.peek()) else {
            return None;
        };
        match a.cmp(b) {
            std::cmp::Ordering::Equal => return Some(**a),
            std::cmp::Ordering::Less => {
                left_iter.next();
            }
            std::cmp::Ordering::Greater => {
                right_iter.next();
            }
        }
    }
}

/// `CalendarUtil.buildServicePeriodMap` + `ServicePeriod.toDates()`: the
/// weekly pattern between start and end (end clamped to start when the
/// calendar row is inverted), plus every added date, minus every removed
/// date. Removals win over additions regardless of row order.
fn build_service_dates(feed: &GtfsFeed) -> HashMap<StringId, BTreeSet<NaiveDate>> {
    let mut added: HashMap<StringId, HashSet<NaiveDate>> = HashMap::new();
    let mut removed: HashMap<StringId, HashSet<NaiveDate>> = HashMap::new();
    if let Some(calendar_dates) = &feed.calendar_dates {
        for row in &calendar_dates.rows {
            let Some(date) = gtfs_date_to_naive(row.date) else {
                continue;
            };
            match row.exception_type {
                ExceptionType::Added => {
                    added.entry(row.service_id).or_default().insert(date);
                }
                _ => {
                    removed.entry(row.service_id).or_default().insert(date);
                }
            }
        }
    }

    let mut dates_by_service: HashMap<StringId, BTreeSet<NaiveDate>> = HashMap::new();
    if let Some(calendar) = &feed.calendar {
        for row in &calendar.rows {
            let (Some(start), Some(mut end)) = (
                gtfs_date_to_naive(row.start_date),
                gtfs_date_to_naive(row.end_date),
            ) else {
                continue;
            };
            if start > end {
                end = start;
            }
            let dates = dates_by_service.entry(row.service_id).or_default();
            let mut current = start;
            while current <= end {
                if service_available_on_date(row, current) {
                    dates.insert(current);
                }
                match current.succ_opt() {
                    Some(next) => current = next,
                    None => break,
                }
            }
        }
    }
    for (service_id, dates) in added {
        dates_by_service
            .entry(service_id)
            .or_default()
            .extend(dates);
    }
    for (service_id, dates) in removed {
        if let Some(active) = dates_by_service.get_mut(&service_id) {
            for date in dates {
                active.remove(&date);
            }
        }
    }
    dates_by_service
}

fn gtfs_date_to_naive(date: GtfsDate) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(date.year(), date.month() as u32, date.day() as u32)
}

fn service_available_on_date(calendar: &Calendar, date: NaiveDate) -> bool {
    match date.weekday() {
        Weekday::Mon => is_available(calendar.monday),
        Weekday::Tue => is_available(calendar.tuesday),
        Weekday::Wed => is_available(calendar.wednesday),
        Weekday::Thu => is_available(calendar.thursday),
        Weekday::Fri => is_available(calendar.friday),
        Weekday::Sat => is_available(calendar.saturday),
        Weekday::Sun => is_available(calendar.sunday),
    }
}

fn is_available(availability: ServiceAvailability) -> bool {
    matches!(availability, ServiceAvailability::Available)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CsvTable;
    use gtfs_guru_model::{GtfsDate, RouteType, StopTime};

    fn run(feed: &mut GtfsFeed) -> Vec<ValidationNotice> {
        let mut notices = NoticeContainer::new();
        feed.rebuild_stop_times_index();
        BlockTripsWithOverlappingStopTimesValidator.validate(feed, &mut notices);
        notices.iter().cloned().collect()
    }

    fn calendar(rows: Vec<Calendar>) -> Option<CsvTable<Calendar>> {
        Some(CsvTable {
            headers: Vec::new(),
            rows,
            row_numbers: Vec::new(),
        })
    }

    #[test]
    fn emits_notice_for_overlapping_trips_in_same_block() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        feed.calendar = calendar(vec![calendar_row("SVC1", "20240101", Weekday::Mon, &feed)]);

        let notices = run(&mut feed);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].code,
            CODE_BLOCK_TRIPS_WITH_OVERLAPPING_STOP_TIMES
        );
    }

    #[test]
    fn no_notice_for_non_overlapping_trips() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "09:00:00", "10:00:00", &feed));
        feed.calendar = calendar(vec![calendar_row("SVC1", "20240101", Weekday::Mon, &feed)]);

        assert!(run(&mut feed).is_empty());
    }

    #[test]
    fn no_notice_when_service_dates_do_not_overlap() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC2", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        feed.calendar = calendar(vec![
            calendar_row("SVC1", "20240101", Weekday::Mon, &feed),
            calendar_row("SVC2", "20240102", Weekday::Tue, &feed),
        ]);

        assert!(run(&mut feed).is_empty());
    }

    #[test]
    fn emits_notice_when_service_dates_overlap() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC2", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        feed.calendar = calendar(vec![
            calendar_row("SVC1", "20240101", Weekday::Mon, &feed),
            calendar_row("SVC2", "20240101", Weekday::Mon, &feed),
        ]);

        let notices = run(&mut feed);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].code,
            CODE_BLOCK_TRIPS_WITH_OVERLAPPING_STOP_TIMES
        );
    }

    /// Hyderabad (mdb-2457): trip A's last stop and trip B's first stop carry
    /// the same arrival/departure pair. That is a block transfer, not an
    /// overlap, and Java skips the pair explicitly.
    #[test]
    fn block_transfer_with_identical_boundary_stop_time_is_not_an_overlap() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = vec![
            stop_time("T1", "STOP1", 1, "06:00:00", "06:00:00", &feed),
            stop_time("T1", "STOP2", 2, "06:27:05", "06:28:44", &feed),
            stop_time("T2", "STOP2", 1, "06:27:05", "06:28:44", &feed),
            stop_time("T2", "STOP1", 2, "07:00:00", "07:00:00", &feed),
        ];
        feed.calendar = calendar(vec![calendar_row("SVC1", "20240101", Weekday::Mon, &feed)]);

        assert!(run(&mut feed).is_empty());
    }

    /// Same boundary times but with an actual overlap on trip B's first stop:
    /// only the departure matches, so the transfer exception does not apply.
    #[test]
    fn boundary_stop_time_with_different_arrival_still_overlaps() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = vec![
            stop_time("T1", "STOP1", 1, "06:00:00", "06:00:00", &feed),
            stop_time("T1", "STOP2", 2, "06:27:05", "06:28:44", &feed),
            stop_time("T2", "STOP2", 1, "06:27:00", "06:28:44", &feed),
            stop_time("T2", "STOP1", 2, "07:00:00", "07:00:00", &feed),
        ];
        feed.calendar = calendar(vec![calendar_row("SVC1", "20240101", Weekday::Mon, &feed)]);

        assert_eq!(run(&mut feed).len(), 1);
    }

    /// SNCB (mdb-686): every weekday is 0 and there are no calendar_dates, so
    /// the service has no active date. Java requires a shared active date even
    /// for identical service_ids; a service that never runs cannot overlap.
    #[test]
    fn same_service_id_without_active_dates_does_not_overlap() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        let mut never = calendar_row("SVC1", "20240101", Weekday::Mon, &feed);
        never.monday = ServiceAvailability::Unavailable;
        feed.calendar = calendar(vec![never]);

        assert!(run(&mut feed).is_empty());
    }

    /// Same service_id but the service is not defined in calendar.txt or
    /// calendar_dates.txt at all: no dates, no overlap.
    #[test]
    fn same_service_id_unknown_to_calendars_does_not_overlap() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        feed.calendar = calendar(vec![]);

        assert!(run(&mut feed).is_empty());
    }

    /// `intersection` is the first shared active date as YYYYMMDD, the way
    /// Java's Gson serialiser writes GtfsDate.
    #[test]
    fn intersection_is_first_shared_service_date() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC2", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        // SVC1 runs Mondays through January 2024; SVC2 runs Mondays from the
        // 15th. First shared Monday is 2024-01-15.
        let mut svc1 = calendar_row("SVC1", "20240101", Weekday::Mon, &feed);
        svc1.end_date = GtfsDate::parse("20240131").unwrap();
        let mut svc2 = calendar_row("SVC2", "20240115", Weekday::Mon, &feed);
        svc2.end_date = GtfsDate::parse("20240131").unwrap();
        feed.calendar = calendar(vec![svc1, svc2]);

        let notices = run(&mut feed);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0]
                .context
                .get("intersection")
                .and_then(|v| v.as_str()),
            Some("20240115")
        );
    }

    /// Trips whose first or last stop_time lacks arrival or departure are not
    /// given an interval at all, like Java's constructOrderedTripIntervals.
    #[test]
    fn trip_without_boundary_times_is_skipped() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        let mut second = stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed);
        second[0].departure_time = None;
        feed.stop_times.rows.extend(second);
        feed.calendar = calendar(vec![calendar_row("SVC1", "20240101", Weekday::Mon, &feed)]);

        assert!(run(&mut feed).is_empty());
    }

    /// A calendar row with start_date after end_date collapses to the start
    /// date, as CalendarUtil does, instead of producing no dates.
    #[test]
    fn inverted_calendar_range_collapses_to_start_date() {
        let mut feed = base_feed();
        feed.trips.rows = vec![
            trip("T1", "SVC1", "BLOCK1", &feed),
            trip("T2", "SVC1", "BLOCK1", &feed),
        ];
        feed.stop_times.rows = stop_times_for_trip("T1", "08:00:00", "09:00:00", &feed);
        feed.stop_times
            .rows
            .extend(stop_times_for_trip("T2", "08:30:00", "09:30:00", &feed));
        let mut inverted = calendar_row("SVC1", "20240101", Weekday::Mon, &feed);
        inverted.end_date = GtfsDate::parse("20231201").unwrap();
        feed.calendar = calendar(vec![inverted]);

        let notices = run(&mut feed);
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0]
                .context
                .get("intersection")
                .and_then(|v| v.as_str()),
            Some("20240101")
        );
    }

    fn base_feed() -> GtfsFeed {
        let mut feed = GtfsFeed::default();
        feed.agency = CsvTable {
            headers: Vec::new(),
            rows: vec![gtfs_guru_model::Agency {
                agency_id: None,
                agency_name: "Agency".into(),
                agency_url: feed.pool.intern("https://example.com"),
                agency_timezone: feed.pool.intern("UTC"),
                agency_lang: None,
                agency_phone: None,
                agency_fare_url: None,
                agency_email: None,
                cemv_support: None,
            }],
            row_numbers: Vec::new(),
        };
        feed.stops = CsvTable {
            headers: Vec::new(),
            rows: vec![
                gtfs_guru_model::Stop {
                    stop_id: feed.pool.intern("STOP1"),
                    stop_name: Some("Stop 1".into()),
                    stop_lat: Some(10.0),
                    stop_lon: Some(20.0),
                    ..Default::default()
                },
                gtfs_guru_model::Stop {
                    stop_id: feed.pool.intern("STOP2"),
                    stop_name: Some("Stop 2".into()),
                    stop_lat: Some(10.1),
                    stop_lon: Some(20.1),
                    ..Default::default()
                },
            ],
            row_numbers: Vec::new(),
        };
        feed.routes = CsvTable {
            headers: Vec::new(),
            rows: vec![gtfs_guru_model::Route {
                route_id: feed.pool.intern("R1"),
                route_short_name: Some("R1".into()),
                route_type: RouteType::Bus,
                ..Default::default()
            }],
            row_numbers: Vec::new(),
        };
        feed.trips = CsvTable::default();
        feed.stop_times = CsvTable {
            headers: Vec::new(),
            rows: Vec::new(),
            row_numbers: Vec::new(),
        };
        feed
    }

    fn trip(
        trip_id: &str,
        service_id: &str,
        block_id: &str,
        feed: &GtfsFeed,
    ) -> gtfs_guru_model::Trip {
        gtfs_guru_model::Trip {
            route_id: feed.pool.intern("R1"),
            service_id: feed.pool.intern(service_id),
            trip_id: feed.pool.intern(trip_id),
            block_id: Some(feed.pool.intern(block_id)),
            ..Default::default()
        }
    }

    fn stop_time(
        trip_id: &str,
        stop_id: &str,
        stop_sequence: u32,
        arrival: &str,
        departure: &str,
        feed: &GtfsFeed,
    ) -> StopTime {
        StopTime {
            trip_id: feed.pool.intern(trip_id),
            stop_id: feed.pool.intern(stop_id),
            stop_sequence,
            arrival_time: Some(GtfsTime::parse(arrival).unwrap()),
            departure_time: Some(GtfsTime::parse(departure).unwrap()),
            ..Default::default()
        }
    }

    fn stop_times_for_trip(
        trip_id: &str,
        start: &str,
        end: &str,
        feed: &GtfsFeed,
    ) -> Vec<StopTime> {
        vec![
            stop_time(trip_id, "STOP1", 1, start, start, feed),
            stop_time(trip_id, "STOP2", 2, end, end, feed),
        ]
    }

    fn calendar_row(
        service_id: &str,
        date_str: &str,
        weekday: Weekday,
        feed: &GtfsFeed,
    ) -> Calendar {
        let date = GtfsDate::parse(date_str).unwrap();
        let on = |day: Weekday| {
            if weekday == day {
                ServiceAvailability::Available
            } else {
                ServiceAvailability::Unavailable
            }
        };
        Calendar {
            service_id: feed.pool.intern(service_id),
            monday: on(Weekday::Mon),
            tuesday: on(Weekday::Tue),
            wednesday: on(Weekday::Wed),
            thursday: on(Weekday::Thu),
            friday: on(Weekday::Fri),
            saturday: on(Weekday::Sat),
            sunday: on(Weekday::Sun),
            start_date: date,
            end_date: date,
        }
    }
}
