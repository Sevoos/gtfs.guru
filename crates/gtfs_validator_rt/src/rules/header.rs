use gtfs_guru_core::{NoticeContainer, NoticeSeverity, ValidationNotice};

use crate::transit_realtime::FeedHeader;
use crate::utils::java_utils::{java_parse_float, JavaParseFloatError};
use crate::RtSnapshotContext;

use crate::transit_realtime::feed_header::Incrementality;
use crate::validator::RtValidator;

const CODE_INVALID_REALTIME_VERSION: &str = "invalid_realtime_version";
const CODE_FULL_DATASET_ENTITY_IS_DELETED: &str = "full_dataset_entity_is_deleted";
const CODE_MISSING_HEADER_INCREMENTALITY: &str = "missing_header_incrementality";

const FULL_DATASET: i32 = Incrementality::FullDataset as i32;

const GTFS_RT_V1: &str = "1.0";
const GTFS_RT_V2: &str = "2.0";
static VALID_VERSIONS: &[&str] = &[GTFS_RT_V1, GTFS_RT_V2];

fn entity_has_is_deleted_notice(
    entity_index: usize,
    entity_id: &str,
    is_deleted: bool,
) -> ValidationNotice {
    let mut notice = ValidationNotice::new(
        CODE_FULL_DATASET_ENTITY_IS_DELETED,
        NoticeSeverity::Error,
        "FULL_DATASET feeds should not include entity.is_deleted",
    );
    notice.insert_context_field("entityIndex", entity_index);
    notice.insert_context_field("entityId", entity_id);
    notice.insert_context_field("isDeleted", is_deleted);
    notice.field_order = vec!["entityIndex".into(), "entityId".into(), "isDeleted".into()];
    notice
}

trait JavaFeedHeader {
    fn get_incrementality(&self) -> i32;
    fn has_incrementality(&self) -> bool;
}

impl JavaFeedHeader for FeedHeader {
    fn get_incrementality(&self) -> i32 {
        self.incrementality.unwrap_or(FULL_DATASET)
    }

    fn has_incrementality(&self) -> bool {
        self.incrementality.is_some()
    }
}

fn invalid_version_notice(version: &str) -> ValidationNotice {
    let mut notice = ValidationNotice::new(
        CODE_INVALID_REALTIME_VERSION,
        NoticeSeverity::Error,
        format!("header.gtfs_realtime_version {version:?} does not belong to {VALID_VERSIONS:?}"),
    );
    notice.insert_context_field("fieldPath", "header.gtfs_realtime_version");
    notice.insert_context_field("gtfsRealtimeVersion", version);
    notice.field_order = vec!["fieldPath".into(), "gtfsRealtimeVersion".into()];
    notice
}

fn is_v2_or_higher(version: &str) -> Result<bool, JavaParseFloatError> {
    Ok(java_parse_float(version)? >= 2.0)
}

/*
   Java's isValidVersion also treats an *absent* version as valid. That branch is
   unreachable here: the field is proto2 `required`, so the loading boundary
   rejects a message without it before any rule runs.
*/
fn is_valid_version(version: &str) -> bool {
    VALID_VERSIONS.contains(&version)
}

fn missing_incrementality_notice(version: &str) -> ValidationNotice {
    let mut notice = ValidationNotice::new(
        CODE_MISSING_HEADER_INCREMENTALITY,
        NoticeSeverity::Error,
        "header.incrementality is required for gtfs_realtime_version 2.0 and higher",
    );
    notice.insert_context_field("fieldPath", "header.incrementality");
    notice.insert_context_field("gtfsRealtimeVersion", version);
    notice.field_order = vec!["fieldPath".into(), "gtfsRealtimeVersion".into()];
    notice
}

#[derive(Debug, Default)]
pub struct HeaderValidator;

impl RtValidator for HeaderValidator {
    fn name(&self) -> &'static str {
        "header"
    }

    fn validate(&self, context: &RtSnapshotContext<'_>, notices: &mut NoticeContainer) {
        let mut error_list_e038: Vec<ValidationNotice> = Vec::new();
        let mut error_list_e039: Vec<ValidationNotice> = Vec::new();
        let mut error_list_e049: Vec<ValidationNotice> = Vec::new();

        let message = context.feed.message();
        let header = &message.header;
        let gtfs_version = &header.gtfs_realtime_version;

        if !is_valid_version(gtfs_version) {
            // E038 - Invalid header.gtfs_realtime_version
            error_list_e038.push(invalid_version_notice(gtfs_version));
        }

        match is_v2_or_higher(gtfs_version) {
            Ok(true) if !header.has_incrementality() => {
                // E049 - header incrementality not populated
                error_list_e049.push(missing_incrementality_notice(gtfs_version));
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!("error checking header version for E049: {error}")
            }
        }

        if header.get_incrementality() == FULL_DATASET {
            for (i, entity) in message.entity.iter().enumerate() {
                if let Some(is_deleted) = entity.is_deleted {
                    // E039 - FULL_DATASET feeds should not include entity.is_deleted
                    error_list_e039.push(entity_has_is_deleted_notice(i, &entity.id, is_deleted));
                }
            }
        }

        for notice in error_list_e038
            .into_iter()
            .chain(error_list_e039)
            .chain(error_list_e049)
        {
            notices.push(notice);
        }
    }
}
