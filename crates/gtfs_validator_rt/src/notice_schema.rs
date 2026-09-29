//! Realtime notice schemas, deliberately separate from the Schedule surface.
//!
//! GTF-11: adding RT codes to `gtfs_validator_core`'s notice schema would
//! wrongly make them part of the GTFS Schedule specification surface.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use gtfs_guru_core::notice_schema::{NoticeSchemaSeverity, ReferencesSchema};
use serde::{Deserialize, Serialize};

/// One RT notice code, in the form the compiler can check against the rules.
pub struct RtNoticeSchemaEntry {
    pub code: &'static str,
    pub severity: NoticeSchemaSeverity,
    /// `(field name, field type)` in report order.
    pub fields: &'static [(&'static str, &'static str)],
}

static RT_NOTICE_SCHEMA_ENTRIES: &[RtNoticeSchemaEntry] = &[
    RtNoticeSchemaEntry {
        code: "full_dataset_entity_is_deleted",
        severity: NoticeSchemaSeverity::Error,
        fields: &[
            ("entityIndex", "integer"),
            ("entityId", "string"),
            ("isDeleted", "boolean"),
        ],
    },
    RtNoticeSchemaEntry {
        code: "invalid_realtime_version",
        severity: NoticeSchemaSeverity::Error,
        fields: &[("fieldPath", "string"), ("gtfsRealtimeVersion", "string")],
    },
    RtNoticeSchemaEntry {
        code: "missing_header_incrementality",
        severity: NoticeSchemaSeverity::Error,
        fields: &[("fieldPath", "string"), ("gtfsRealtimeVersion", "string")],
    },
    RtNoticeSchemaEntry {
        code: "runtime_exception_in_rt_validator_error",
        severity: NoticeSchemaSeverity::Error,
        fields: &[("validator", "string"), ("message", "string")],
    },
];

/// The hand-written half: prose and canonical provenance, editable without a
/// recompile.
#[derive(Debug, Clone, Deserialize)]
pub struct RtNoticeMetadata {
    #[serde(rename = "canonicalId")]
    pub canonical_id: Option<String>,
    #[serde(rename = "dependencyClass")]
    pub dependency_class: String,
    /// Java's occurrence prefix, as a template. Empty where Java emits none.
    #[serde(rename = "occurrenceIdentity")]
    pub occurrence_identity: String,
    #[serde(rename = "javaBehavior")]
    pub java_behavior: String,
    #[serde(rename = "shortSummary")]
    pub short_summary: Option<String>,
    pub description: Option<String>,
    pub references: Option<ReferencesSchema>,
    #[serde(default)]
    pub properties: BTreeMap<String, RtFieldMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RtFieldMetadata {
    pub description: Option<String>,
}

fn rt_notice_metadata() -> &'static BTreeMap<String, RtNoticeMetadata> {
    static METADATA: OnceLock<BTreeMap<String, RtNoticeMetadata>> = OnceLock::new();
    METADATA.get_or_init(|| {
        serde_json::from_str(include_str!("../rt_notice_metadata.json"))
            .expect("bundled RT notice metadata must be valid JSON")
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RtNoticeSchema {
    pub code: String,
    #[serde(rename = "severityLevel")]
    pub severity_level: NoticeSchemaSeverity,
    #[serde(rename = "canonicalId", skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
    #[serde(rename = "dependencyClass")]
    pub dependency_class: String,
    #[serde(rename = "occurrenceIdentity")]
    pub occurrence_identity: String,
    #[serde(rename = "javaBehavior")]
    pub java_behavior: String,
    #[serde(rename = "shortSummary", skip_serializing_if = "Option::is_none")]
    pub short_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<ReferencesSchema>,
    pub properties: BTreeMap<String, RtFieldSchema>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RtFieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(rename = "fieldName")]
    pub field_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Join the compile-time table with the bundled metadata.
///
/// Panics if a code in the table has no metadata entry: the two halves are one
/// definition split across two files, and a half-defined notice is a bug, not a runtime condition.
pub fn build_rt_notice_schema_map() -> BTreeMap<String, RtNoticeSchema> {
    let metadata = rt_notice_metadata();
    let mut map = BTreeMap::new();

    for entry in RT_NOTICE_SCHEMA_ENTRIES {
        let meta = metadata
            .get(entry.code)
            .unwrap_or_else(|| panic!("no RT notice metadata for {}", entry.code));

        let properties = entry
            .fields
            .iter()
            .map(|(name, field_type)| {
                (
                    (*name).to_string(),
                    RtFieldSchema {
                        field_type: (*field_type).to_string(),
                        field_name: (*name).to_string(),
                        description: meta
                            .properties
                            .get(*name)
                            .and_then(|field| field.description.clone()),
                    },
                )
            })
            .collect();

        map.insert(
            entry.code.to_string(),
            RtNoticeSchema {
                code: entry.code.to_string(),
                severity_level: entry.severity,
                canonical_id: meta.canonical_id.clone(),
                dependency_class: meta.dependency_class.clone(),
                occurrence_identity: meta.occurrence_identity.clone(),
                java_behavior: meta.java_behavior.clone(),
                short_summary: meta.short_summary.clone(),
                description: meta.description.clone(),
                references: meta.references.clone(),
                properties,
            },
        );
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_has_metadata_and_every_metadata_entry_has_a_code() {
        let schemas = build_rt_notice_schema_map();
        assert_eq!(schemas.len(), RT_NOTICE_SCHEMA_ENTRIES.len());
        for code in rt_notice_metadata().keys() {
            assert!(
                schemas.contains_key(code),
                "metadata for unknown code {code}"
            );
        }
    }

    #[test]
    fn rule_notices_carry_their_canonical_id() {
        let schemas = build_rt_notice_schema_map();
        for (code, expected) in [
            ("full_dataset_entity_is_deleted", "E039"),
            ("invalid_realtime_version", "E038"),
            ("missing_header_incrementality", "E049"),
        ] {
            assert_eq!(
                schemas[code].canonical_id.as_deref(),
                Some(expected),
                "{code}"
            );
        }
    }

    #[test]
    fn every_notice_has_described_fields() {
        for (code, schema) in build_rt_notice_schema_map() {
            // Present or absent, but never present and empty.
            if let Some(canonical_id) = &schema.canonical_id {
                assert!(
                    !canonical_id.is_empty(),
                    "{code} declares an empty canonical id"
                );
            }
            for (name, field) in &schema.properties {
                assert!(
                    field.description.is_some(),
                    "{code}.{name} has no description"
                );
            }
        }
    }
}
