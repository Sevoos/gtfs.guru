use gtfs_guru_core::{
    input::GtfsInput, rules::PathwayReachableLocationValidator, GtfsFeed, NoticeContainer,
    NoticeSeverity, StringPool, Validator,
};
use gtfs_guru_model::{Pathway, PathwayMode, Stop};
use std::fs;
use std::path::{Path, PathBuf};

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // root
        .unwrap()
        .to_path_buf()
}

fn test_feeds_root() -> PathBuf {
    project_root().join("test-gtfs-feeds")
}

/// The frozen slice of the MBTA feed, carrying only the two tables this test
/// reads. Committed, so the numbers below are facts about a known input rather
/// than whatever the publisher served today. See
/// `test-gtfs-feeds/real-world/manifest.json` for its provenance.
fn mbta_pathway_fixture() -> PathBuf {
    let path = test_feeds_root()
        .join("real-world")
        .join("boston_mbta_pathways.zip");
    assert!(
        path.is_file(),
        "committed fixture missing at {path:?}; it is tracked, so a clean \
         checkout always has it"
    );
    path
}

/// Counts the data records in a CSV payload the way the loader reads it:
/// header consumed, ragged rows tolerated, BOM skipped.
fn csv_record_count(data: &[u8]) -> usize {
    let body = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(body)
        .records()
        .count()
}

#[test]
fn test_base_valid() {
    let feed_path = test_feeds_root().join("base-valid");
    assert!(
        feed_path.exists(),
        "Base valid feed not found at {:?}",
        feed_path
    );

    let input = GtfsInput::from_path(&feed_path).expect("Failed to create input");
    let runner = gtfs_guru_core::rules::default_runner();

    // Set validation date to a date within the valid range of the feed if necessary,
    // or rely on today if the feed is dynamic.
    // The base-valid README or content might specify dates.
    // For now, let's assume it's designed to pass or we might need to mock date.

    let outcome = gtfs_guru_core::engine::validate_input(&input, &runner);

    // Filter out INFO/WARNING notices. Base valid might have warnings.
    let unexpected_notices: Vec<_> = outcome
        .notices
        .iter()
        .filter(|n| n.severity == NoticeSeverity::Error)
        .collect();

    assert!(
        unexpected_notices.is_empty(),
        "Expected no errors in base-valid, found: {:#?}",
        unexpected_notices
    );
}

#[test]
fn test_mbta_pathways_are_fully_loaded_and_reachable() {
    let feed_path = mbta_pathway_fixture();

    let input = GtfsInput::from_path(&feed_path).expect("Failed to create MBTA input");
    let reader = input.reader();
    let pool = StringPool::new();
    let mut load_notices = NoticeContainer::new();
    let raw_pathways = reader
        .read_file("pathways.txt")
        .expect("Failed to read raw MBTA pathways.txt");
    let stops = reader
        .read_csv_with_notices::<Stop>("stops.txt", &mut load_notices, &pool)
        .expect("Failed to read MBTA stops.txt");
    let pathways = reader
        .read_csv_with_notices::<Pathway>("pathways.txt", &mut load_notices, &pool)
        .expect("Failed to read MBTA pathways.txt");

    // The fixture is frozen, so these are exact expectations rather than a
    // self-comparison. Both halves matter: the raw record count proves the
    // fixture itself has not been swapped, and the row count proves the loader
    // kept every record.
    assert_eq!(
        csv_record_count(&raw_pathways),
        9_293,
        "fixture changed: pathways.txt should hold 9293 records"
    );
    assert_eq!(
        pathways.rows.len(),
        9_293,
        "every MBTA pathway row must deserialize; none may be dropped"
    );
    assert_eq!(
        stops.rows.len(),
        10_293,
        "every MBTA stop row must deserialize; none may be dropped"
    );

    // The defect this fixture exists for. MBTA spells descending stairs as a
    // negative `stair_count`, which a narrower integer type rejects row by row.
    // Counting the surviving negatives proves the sign was preserved, not just
    // that the row arrived -- a `u32` field would drop all 441 and an unsigned
    // reinterpretation would keep the rows while corrupting every value.
    let descending = pathways
        .rows
        .iter()
        .filter(|pathway| pathway.stair_count.is_some_and(|count| count < 0))
        .count();
    assert_eq!(
        descending, 441,
        "negative stair_count values must survive with their sign intact"
    );

    // Fare gates and exit gates are the modes with their own validation rules,
    // and a frozen input is what lets us assert they are actually present to be
    // validated rather than hoping today's feed still has some.
    let mode_count = |mode: PathwayMode| {
        pathways
            .rows
            .iter()
            .filter(|pathway| pathway.pathway_mode == mode)
            .count()
    };
    assert_eq!(mode_count(PathwayMode::FareGate), 120, "fare gates");
    assert_eq!(mode_count(PathwayMode::ExitGate), 150, "exit gates");
    assert_eq!(mode_count(PathwayMode::Elevator), 486, "elevators");
    assert_eq!(mode_count(PathwayMode::Escalator), 187, "escalators");

    let feed = GtfsFeed {
        stops,
        pathways: Some(pathways),
        pool,
        ..Default::default()
    };
    let mut notices = NoticeContainer::new();
    PathwayReachableLocationValidator.validate(&feed, &mut notices);
    let unreachable: Vec<_> = notices
        .iter()
        .filter(|notice| notice.code == "pathway_unreachable_location")
        .collect();

    assert!(
        unreachable.is_empty(),
        "MBTA has no canonical pathway reachability errors: {unreachable:#?}"
    );
}

#[test]
fn test_errors() {
    let errors_root = test_feeds_root().join("errors");
    assert!(errors_root.exists(), "Errors directory not found");

    visit_dirs(&errors_root, &mut |path| {
        // Only process directories that are "leaf" nodes (contain .txt files)
        // OR simply directories that match an error code name.
        // The structure is errors/category/error_code/*.txt

        if path.is_file() || contains_txt_files(path) {
            let error_code = if path.is_file() {
                path.file_stem().unwrap().to_str().unwrap()
            } else {
                path.file_name().unwrap().to_str().unwrap()
            };
            let expected_notice_code = match error_code {
                // Renamed by gtfs-validator v8.0.0; keep the existing tracked fixture path.
                "fare_transfer_rule_missing_transfer_count" => {
                    "fare_transfer_rule_without_transfer_count"
                }
                _ => error_code,
            };
            println!("Testing error expectation: {} in {:?}", error_code, path);

            let _date_guard = gtfs_guru_core::set_validation_date(Some(
                chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            ));
            let _thorough_guard = gtfs_guru_core::set_thorough_mode_enabled(true);
            let is_google = path.to_string_lossy().contains("google");
            let _google_guard = gtfs_guru_core::set_google_rules_enabled(is_google);

            let input = GtfsInput::from_path(path).expect("Failed to create input");
            let runner = gtfs_guru_core::rules::default_runner();
            let outcome = gtfs_guru_core::engine::validate_input(&input, &runner);

            let found = outcome
                .notices
                .iter()
                .any(|n| n.code == expected_notice_code);

            if !found {
                println!("Notices found: {:#?}", outcome.notices);
                panic!(
                    "Expected notice code '{}' not found in {:?}",
                    expected_notice_code, path
                );
            }
        }
    })
    .unwrap();
}

#[test]
fn test_warnings() {
    let warnings_root = test_feeds_root().join("warnings");
    assert!(warnings_root.exists(), "Warnings directory not found");

    visit_dirs(&warnings_root, &mut |path| {
        if path.is_file() || contains_txt_files(path) {
            let warning_code = if path.is_file() {
                path.file_stem().unwrap().to_str().unwrap()
            } else {
                path.file_name().unwrap().to_str().unwrap()
            };
            if warning_code == "leading_or_trailing_whitespaces" {
                return;
            }
            println!(
                "Testing warning expectation: {} in {:?}",
                warning_code, path
            );

            let _date_guard = gtfs_guru_core::set_validation_date(Some(
                chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            ));
            let _thorough_guard = gtfs_guru_core::set_thorough_mode_enabled(true);

            let is_google = path.to_string_lossy().contains("google");
            let _google_guard = gtfs_guru_core::set_google_rules_enabled(is_google);

            let input = GtfsInput::from_path(path).expect("Failed to create input");
            let runner = gtfs_guru_core::rules::default_runner();
            let outcome = gtfs_guru_core::engine::validate_input(&input, &runner);

            let found = outcome.notices.iter().any(|n| n.code == warning_code);

            if !found {
                println!("Notices found: {:#?}", outcome.notices);
                panic!(
                    "Expected warning code '{}' not found in {:?}",
                    warning_code, path
                );
            }
        }
    })
    .unwrap();
}

fn visit_dirs(dir: &Path, cb: &mut dyn FnMut(&Path)) -> std::io::Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if is_zip_file(&path) {
                    let stem = path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .unwrap_or("");
                    let sibling_dir = path.with_file_name(stem);
                    if !(sibling_dir.is_dir() && contains_txt_files(&sibling_dir)) {
                        cb(&path);
                    }
                }
                continue;
            }
            if path.is_dir() {
                // If this directory is a test case (contains GTFS txt files), run callback
                if contains_txt_files(&path) {
                    cb(&path);
                } else {
                    // Recurse
                    visit_dirs(&path, cb)?;
                }
            }
        }
    }
    Ok(())
}

fn contains_txt_files(path: &Path) -> bool {
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if let Some(ext) = entry_path.extension() {
                if ext == "txt" {
                    let name = entry_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("");
                    if !name.eq_ignore_ascii_case("README.txt") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn is_zip_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}
