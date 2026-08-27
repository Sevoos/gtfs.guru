//! The GTFS specification revision and canonical-validator release this build
//! is aligned with.
//!
//! `spec_baseline.json` is the single source of truth. Reports quote it so a
//! stored report says which upstream state it was produced against, and
//! `scripts/spec_watch.py` diffs upstream against it. Moving the baseline is the
//! deliberate act of accepting a new upstream state; `docs/spec-watch.md`
//! describes the protocol.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// The committed baseline document, embedded so normal builds stay hermetic.
pub const SPEC_BASELINE_JSON: &str = include_str!("../spec_baseline.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpecRevision {
    pub repository: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    pub commit: String,
    #[serde(rename = "committedAt")]
    pub committed_at: String,
    #[serde(rename = "specPaths")]
    pub spec_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CanonicalBaseline {
    pub repository: String,
    pub version: String,
    #[serde(rename = "publishedAt")]
    pub published_at: String,
    #[serde(rename = "rulesAsset")]
    pub rules_asset: String,
}

/// The upstream differences this baseline consciously accepts.
///
/// `scripts/spec_watch.py` owns the contents: a run reports only differences
/// this block does not already list. The compatibility page reads it so every
/// accepted difference is stated publicly rather than only in the repository.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Acknowledged {
    #[serde(rename = "specFilesNotSupported", default)]
    pub spec_files_not_supported: Vec<String>,
    #[serde(rename = "specFieldsNotSupported", default)]
    pub spec_fields_not_supported: BTreeMap<String, Vec<String>>,
    #[serde(rename = "fieldsNotInSpec", default)]
    pub fields_not_in_spec: BTreeMap<String, Vec<String>>,
    #[serde(rename = "requiredMismatches", default)]
    pub required_mismatches: BTreeMap<String, Vec<String>>,
    #[serde(rename = "enumValuesNotSupported", default)]
    pub enum_values_not_supported: BTreeMap<String, Vec<String>>,
    #[serde(rename = "enumValuesNotInSpec", default)]
    pub enum_values_not_in_spec: BTreeMap<String, Vec<String>>,
    #[serde(rename = "canonicalNoticesNotImplemented", default)]
    pub canonical_notices_not_implemented: Vec<String>,
    #[serde(rename = "noticesNotInCanonical", default)]
    pub notices_not_in_canonical: Vec<String>,
}

/// The baseline document. Reports need only the two revision objects; the
/// compatibility page also reads the accepted differences and the date.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpecBaseline {
    #[serde(rename = "specRevision")]
    pub spec_revision: SpecRevision,
    #[serde(rename = "canonicalBaseline")]
    pub canonical_baseline: CanonicalBaseline,
    /// Defaulted rather than required: a document without the watcher's
    /// bookkeeping still has to yield the two identifiers reports quote.
    #[serde(default)]
    pub acknowledged: Acknowledged,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: String,
}

/// The baseline, or `None` if the bundled document does not parse.
///
/// A library must not panic on data, even data it ships: a malformed baseline
/// should degrade the two identifier strings reports quote, not take down every
/// caller of the crate. `parses_the_bundled_baseline` is what actually keeps the
/// committed file honest, at build time rather than at a user's runtime.
pub fn spec_baseline() -> Option<&'static SpecBaseline> {
    static BASELINE: OnceLock<Option<SpecBaseline>> = OnceLock::new();
    BASELINE
        .get_or_init(|| match serde_json::from_str(SPEC_BASELINE_JSON) {
            Ok(baseline) => Some(baseline),
            Err(err) => {
                debug_assert!(false, "bundled spec baseline must be valid JSON: {err}");
                None
            }
        })
        .as_ref()
}

/// What the identifier accessors report when the baseline is unreadable.
const UNKNOWN_BASELINE: &str = "unknown";

/// `google/transit@<commit>`: the spec revision reports are aligned with.
pub fn spec_revision_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| match spec_baseline() {
        Some(baseline) => format!(
            "{}@{}",
            baseline.spec_revision.repository, baseline.spec_revision.commit
        ),
        None => UNKNOWN_BASELINE.to_string(),
    })
}

/// `MobilityData/gtfs-validator@<tag>`: the canonical release reports are
/// aligned with.
pub fn canonical_baseline_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| match spec_baseline() {
        Some(baseline) => format!(
            "{}@{}",
            baseline.canonical_baseline.repository, baseline.canonical_baseline.version
        ),
        None => UNKNOWN_BASELINE.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{canonical_baseline_id, spec_baseline, spec_revision_id};

    #[test]
    fn parses_the_bundled_baseline() {
        let baseline = spec_baseline().expect("the committed baseline must parse");

        assert_eq!(baseline.spec_revision.repository, "google/transit");
        assert_eq!(baseline.spec_revision.commit.len(), 40);
        assert!(baseline
            .spec_revision
            .spec_paths
            .iter()
            .any(|path| path.ends_with("reference.md")));
        assert_eq!(
            baseline.canonical_baseline.repository,
            "MobilityData/gtfs-validator"
        );
        assert!(baseline.canonical_baseline.version.starts_with('v'));
    }

    #[test]
    fn builds_report_identifiers() {
        assert!(spec_revision_id().starts_with("google/transit@"));
        assert!(canonical_baseline_id().starts_with("MobilityData/gtfs-validator@v"));
    }
}
