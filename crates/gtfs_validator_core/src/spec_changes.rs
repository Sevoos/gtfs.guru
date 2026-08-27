//! What upstream changed recently, and whether this build supports it.
//!
//! `spec_baseline.json` says *which* upstream state we answer for;
//! `spec_changes.json` says what moved to get there and what we do about it.
//! The compatibility page on the website is generated from both, so a claim on
//! that page is a claim in version control.
//!
//! Curation is deliberate: a support status is a public promise, and no
//! mechanical diff of upstream prose can decide whether a change is
//! implemented, consciously declined, or still open. What *is* mechanical is
//! checking that the promises match the build — [`validate_spec_changes`] does
//! that, and `committed_spec_changes_are_coherent` runs it against the
//! committed documents on every `cargo test`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::spec_baseline::{Acknowledged, SpecBaseline};
use crate::spec_surface::SpecSurface;

/// The committed document, embedded so normal builds stay hermetic.
pub const SPEC_CHANGES_JSON: &str = include_str!("../spec_changes.json");

/// Flags a notice needs before it is reported at all.
const OPTIONAL_MODE_FLAGS: &[&str] = &["--thorough", "--google-rules"];

/// The `acknowledged` categories, paired with the entry kind they describe and
/// whether the item is expected to be *present* in this build's surface.
///
/// The presence column is what turns the baseline's bookkeeping into a test: a
/// field we accept beyond the reference must really be accepted, and a canonical
/// notice we record as unimplemented must really not be emitted.
const DIFFERENCE_CATEGORIES: &[(&str, ChangeKind, bool)] = &[
    ("specFilesNotSupported", ChangeKind::File, false),
    ("specFieldsNotSupported", ChangeKind::Field, false),
    ("fieldsNotInSpec", ChangeKind::Field, true),
    ("requiredMismatches", ChangeKind::Field, true),
    ("enumValuesNotSupported", ChangeKind::EnumValue, false),
    ("enumValuesNotInSpec", ChangeKind::EnumValue, true),
    ("canonicalNoticesNotImplemented", ChangeKind::Rule, false),
    ("noticesNotInCanonical", ChangeKind::Rule, true),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    File,
    Field,
    EnumValue,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SupportStatus {
    /// This build implements it.
    Supported,
    /// A difference from upstream that the baseline records as intended.
    AcceptedDifference,
    /// Upstream has it, this build does not, and nobody has committed to it.
    NotSupported,
    /// Upstream has it, this build does not, and it is being worked on.
    Planned,
}

impl SupportStatus {
    pub fn label(self) -> &'static str {
        match self {
            SupportStatus::Supported => "Supported",
            SupportStatus::AcceptedDifference => "Accepted difference",
            SupportStatus::NotSupported => "Not supported",
            SupportStatus::Planned => "Planned",
        }
    }

    /// A CSS-class-safe slug, so the page can style each status.
    pub fn slug(self) -> &'static str {
        match self {
            SupportStatus::Supported => "supported",
            SupportStatus::AcceptedDifference => "accepted",
            SupportStatus::NotSupported => "unsupported",
            SupportStatus::Planned => "planned",
        }
    }
}

/// Where the change came from, so a reader can check it themselves.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Upstream {
    /// `spec` or `canonical`.
    pub kind: String,
    /// A human-readable reference such as `google/transit#640`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub url: String,
    /// `YYYY-MM-DD`, the date upstream landed it.
    ///
    /// Absent for a long-standing difference that upstream never "landed" on a
    /// date we can point at — a Google extension the reference has never
    /// listed, say. An undated entry is an ongoing difference rather than a new
    /// arrival, so it never appears as news.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpecChangeEntry {
    pub kind: ChangeKind,
    /// `file.txt/field`, `file.txt/field=value`, `file.txt`, or a notice code.
    pub id: String,
    pub title: String,
    /// One sentence of plain English, rendered as the page's row body.
    pub summary: String,
    pub status: SupportStatus,
    /// The `acknowledged` category, required when the status is
    /// `acceptedDifference` and forbidden otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difference: Option<String>,
    /// The GTFS Guru release that gained support, when there is one.
    #[serde(rename = "guruVersion", default)]
    pub guru_version: Option<String>,
    /// A CLI flag the behaviour is gated behind, when it is not on by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
    /// Notice codes the change introduced or is reported through.
    #[serde(default)]
    pub notices: Vec<String>,
    pub upstream: Upstream,
}

impl SpecChangeEntry {
    /// `(file, field)` for a field entry, `(file, field)` plus the value for an
    /// enum entry; `None` when the identifier does not have that shape.
    fn field_parts(&self) -> Option<(&str, &str)> {
        let (file, rest) = self.id.split_once('/')?;
        let field = rest.split_once('=').map_or(rest, |(name, _)| name);
        (!file.is_empty() && !field.is_empty()).then_some((file, field))
    }

    fn enum_value(&self) -> Option<i64> {
        self.id.split_once('=')?.1.parse().ok()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChangeWindow {
    /// Changes dated after this release are presented as new.
    #[serde(rename = "sinceVersion")]
    pub since_version: String,
    #[serde(rename = "sinceDate")]
    pub since_date: String,
    pub note: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpecChanges {
    pub window: ChangeWindow,
    pub entries: Vec<SpecChangeEntry>,
}

impl SpecChanges {
    /// Entries upstream landed after the window's release date.
    pub fn in_window(&self) -> Vec<&SpecChangeEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                entry
                    .upstream
                    .date
                    .as_deref()
                    .is_some_and(|date| date > self.window.since_date.as_str())
            })
            .collect()
    }

    /// Differences the baseline records as intended, grouped by category so the
    /// page can present them under the same headings the watcher uses.
    pub fn accepted_differences(&self) -> BTreeMap<&str, Vec<&SpecChangeEntry>> {
        let mut grouped: BTreeMap<&str, Vec<&SpecChangeEntry>> = BTreeMap::new();
        for entry in &self.entries {
            if entry.status != SupportStatus::AcceptedDifference {
                continue;
            }
            if let Some(category) = entry.difference.as_deref() {
                grouped.entry(category).or_default().push(entry);
            }
        }
        grouped
    }

    /// Entries upstream has that this build does not implement yet.
    pub fn open_gaps(&self) -> Vec<&SpecChangeEntry> {
        self.entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.status,
                    SupportStatus::NotSupported | SupportStatus::Planned
                )
            })
            .collect()
    }
}

/// The committed changes, or `None` if the bundled document does not parse.
///
/// Non-panicking for the same reason as [`crate::spec_baseline::spec_baseline`]:
/// a library must not take down its callers over data it ships. The committed
/// file is kept honest by the tests below instead.
pub fn spec_changes() -> Option<&'static SpecChanges> {
    static CHANGES: OnceLock<Option<SpecChanges>> = OnceLock::new();
    CHANGES
        .get_or_init(|| match serde_json::from_str(SPEC_CHANGES_JSON) {
            Ok(changes) => Some(changes),
            Err(err) => {
                debug_assert!(false, "bundled spec changes must be valid JSON: {err}");
                None
            }
        })
        .as_ref()
}

/// Every way the committed metadata can contradict the build or the baseline.
///
/// An empty vector is the only acceptable result for a committed document. The
/// messages are written to be read in CI output, so each one names the entry and
/// says what would have to change.
pub fn validate_spec_changes(
    changes: &SpecChanges,
    surface: &SpecSurface,
    baseline: &SpecBaseline,
) -> Vec<String> {
    let mut problems = Vec::new();

    let mut seen = BTreeSet::new();
    for entry in &changes.entries {
        if !seen.insert((entry.kind, entry.id.as_str())) {
            problems.push(format!(
                "duplicate entry for {:?} `{}`",
                entry.kind, entry.id
            ));
        }
        check_shape(entry, &mut problems);
        check_notices(entry, surface, &mut problems);
        check_status(entry, surface, &mut problems);
    }

    check_acknowledged_correspondence(changes, &baseline.acknowledged, &mut problems);
    problems
}

fn check_shape(entry: &SpecChangeEntry, problems: &mut Vec<String>) {
    let identifies_a_column = matches!(entry.kind, ChangeKind::Field | ChangeKind::EnumValue);
    if identifies_a_column && entry.field_parts().is_none() {
        problems.push(format!(
            "`{}` must read `file.txt/field` for a {:?} entry",
            entry.id, entry.kind
        ));
    }
    if entry.kind == ChangeKind::EnumValue && entry.enum_value().is_none() {
        problems.push(format!(
            "`{}` must read `file.txt/field=<integer>` for an enum value",
            entry.id
        ));
    }

    match entry.status {
        SupportStatus::Supported => {
            if entry.guru_version.is_none() {
                problems.push(format!(
                    "`{}` is supported, so it must name the GTFS Guru version that gained it",
                    entry.id
                ));
            }
        }
        _ => {
            if entry.guru_version.is_some() {
                problems.push(format!(
                    "`{}` is `{}`, so it must not name a GTFS Guru version",
                    entry.id,
                    entry.status.label()
                ));
            }
        }
    }

    if let Some(flag) = entry.requires.as_deref() {
        if !OPTIONAL_MODE_FLAGS.contains(&flag) {
            problems.push(format!(
                "`{}` requires unknown flag `{flag}`; expected one of {}",
                entry.id,
                OPTIONAL_MODE_FLAGS.join(", ")
            ));
        }
    }

    if !matches!(entry.upstream.kind.as_str(), "spec" | "canonical") {
        problems.push(format!(
            "`{}` has upstream kind `{}`; expected `spec` or `canonical`",
            entry.id, entry.upstream.kind
        ));
    }
    if let Some(date) = entry.upstream.date.as_deref() {
        // Bytes throughout: mixing a byte length with a character index would
        // read a non-ASCII date inconsistently.
        let bytes = date.as_bytes();
        let dated_yyyy_mm_dd = bytes.len() == 10
            && bytes.iter().enumerate().all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            });
        if !dated_yyyy_mm_dd {
            problems.push(format!(
                "`{}` has upstream date `{date}`; expected `YYYY-MM-DD`",
                entry.id
            ));
        }
    } else if entry.status == SupportStatus::Supported {
        // Support that landed in response to an upstream change has a date; an
        // undated supported entry would be unfalsifiable news.
        problems.push(format!(
            "`{}` is supported, so it must date the upstream change it answers",
            entry.id
        ));
    }
    // The page renders this as an href, so a scheme it would be wrong to
    // follow is caught here rather than shipped to readers.
    if entry.upstream.url.is_empty() {
        problems.push(format!("`{}` must cite an upstream URL", entry.id));
    } else if !entry.upstream.url.starts_with("https://")
        && !entry.upstream.url.starts_with("http://")
    {
        problems.push(format!(
            "`{}` cites upstream URL `{}`; expected an http(s) address",
            entry.id, entry.upstream.url
        ));
    }
    if entry.summary.is_empty() {
        problems.push(format!("`{}` must carry a one-sentence summary", entry.id));
    }
}

fn check_notices(entry: &SpecChangeEntry, surface: &SpecSurface, problems: &mut Vec<String>) {
    for code in &entry.notices {
        // A rule this build does not emit is the whole point of an entry
        // recorded as not implemented, so only the other statuses have to
        // resolve.
        let expected_absent = entry.difference.as_deref() == Some("canonicalNoticesNotImplemented")
            || matches!(
                entry.status,
                SupportStatus::NotSupported | SupportStatus::Planned
            );
        if !expected_absent && !surface.notices.contains_key(code) {
            problems.push(format!(
                "`{}` cites notice `{code}`, which this build does not emit",
                entry.id
            ));
        }
    }
}

fn check_status(entry: &SpecChangeEntry, surface: &SpecSurface, problems: &mut Vec<String>) {
    let expected_present = match entry.status {
        SupportStatus::Supported => Some(true),
        SupportStatus::NotSupported | SupportStatus::Planned => Some(false),
        // Governed by the category instead: some accepted differences are
        // things we accept beyond the reference, others are things we decline.
        SupportStatus::AcceptedDifference => entry.difference.as_deref().and_then(|category| {
            DIFFERENCE_CATEGORIES
                .iter()
                .find(|(name, ..)| *name == category)
                .map(|(.., present)| *present)
        }),
    };
    let Some(expected_present) = expected_present else {
        return;
    };

    let present = is_present(entry, surface);
    if present != expected_present {
        let described = if expected_present {
            "is missing from"
        } else {
            "is still in"
        };
        problems.push(format!(
            "`{}` is recorded as `{}`, but it {described} this build's spec surface",
            entry.id,
            entry.status.label()
        ));
    }

    // A required mismatch means the reference requires the field and we do not.
    // If it ever becomes required here, the record is what is wrong.
    if entry.difference.as_deref() == Some("requiredMismatches") {
        if let Some((file, field)) = entry.field_parts() {
            if surface
                .files
                .get(file)
                .is_some_and(|schema| schema.required_fields.iter().any(|name| name == field))
            {
                problems.push(format!(
                    "`{}` is recorded as a required mismatch, but this build now requires it",
                    entry.id
                ));
            }
        }
    }
}

fn is_present(entry: &SpecChangeEntry, surface: &SpecSurface) -> bool {
    match entry.kind {
        ChangeKind::File => surface.files.contains_key(entry.id.as_str()),
        ChangeKind::Field => entry
            .field_parts()
            .and_then(|(file, field)| {
                surface
                    .files
                    .get(file)
                    .map(|schema| schema.fields.iter().any(|name| name == field))
            })
            .unwrap_or(false),
        ChangeKind::EnumValue => entry
            .field_parts()
            .zip(entry.enum_value())
            .and_then(|((file, field), value)| {
                surface
                    .files
                    .get(file)
                    .and_then(|schema| schema.enums.get(field))
                    .map(|values| values.contains(&value))
            })
            .unwrap_or(false),
        ChangeKind::Rule => surface.notices.contains_key(entry.id.as_str()),
    }
}

/// The accepted differences and the baseline's `acknowledged` block must be the
/// same set, in both directions.
///
/// One direction stops the page from claiming a difference was reviewed when the
/// baseline never accepted it. The other is the gate that matters day to day:
/// when Spec Watch adds an accepted difference, the page has to explain it, so a
/// silent baseline move cannot leave the public documentation behind.
fn check_acknowledged_correspondence(
    changes: &SpecChanges,
    acknowledged: &Acknowledged,
    problems: &mut Vec<String>,
) {
    let mut recorded: BTreeSet<(&str, String)> = BTreeSet::new();
    for entry in &changes.entries {
        if entry.status != SupportStatus::AcceptedDifference {
            if entry.difference.is_some() {
                problems.push(format!(
                    "`{}` is `{}`, so it must not name an `acknowledged` category",
                    entry.id,
                    entry.status.label()
                ));
            }
            continue;
        }
        let Some(category) = entry.difference.as_deref() else {
            problems.push(format!(
                "`{}` is an accepted difference, so it must name its `acknowledged` category",
                entry.id
            ));
            continue;
        };
        match DIFFERENCE_CATEGORIES
            .iter()
            .find(|(name, ..)| *name == category)
        {
            None => problems.push(format!(
                "`{}` names unknown `acknowledged` category `{category}`",
                entry.id
            )),
            Some((_, kind, _)) if *kind != entry.kind => problems.push(format!(
                "`{}` is a {:?} entry under `{category}`, which describes {:?} entries",
                entry.id, entry.kind, kind
            )),
            Some(_) => {
                recorded.insert((category, entry.id.clone()));
            }
        }
    }

    for (category, item) in acknowledged_items(acknowledged) {
        if !recorded.contains(&(category, item.clone())) {
            problems.push(format!(
                "the baseline acknowledges `{item}` under `{category}`, but \
                 spec_changes.json does not explain it"
            ));
        }
    }

    let known: BTreeSet<(&str, String)> = acknowledged_items(acknowledged).into_iter().collect();
    for (category, item) in &recorded {
        if !known.contains(&(category, item.clone())) {
            problems.push(format!(
                "spec_changes.json explains `{item}` as `{category}`, which the baseline \
                 does not acknowledge"
            ));
        }
    }
}

/// The `acknowledged` block flattened to `(category, identifier)` pairs, using
/// the same identifier shapes the entries use.
fn acknowledged_items(acknowledged: &Acknowledged) -> Vec<(&'static str, String)> {
    let mut items = Vec::new();
    let mut push_map = |category: &'static str, map: &BTreeMap<String, Vec<String>>| {
        for (holder, names) in map {
            for name in names {
                items.push((category, format!("{holder}/{name}")));
            }
        }
    };
    push_map(
        "specFieldsNotSupported",
        &acknowledged.spec_fields_not_supported,
    );
    push_map("fieldsNotInSpec", &acknowledged.fields_not_in_spec);
    push_map("requiredMismatches", &acknowledged.required_mismatches);
    push_map(
        "enumValuesNotSupported",
        &acknowledged.enum_values_not_supported,
    );
    push_map("enumValuesNotInSpec", &acknowledged.enum_values_not_in_spec);
    for file in &acknowledged.spec_files_not_supported {
        items.push(("specFilesNotSupported", file.clone()));
    }
    for code in &acknowledged.canonical_notices_not_implemented {
        items.push(("canonicalNoticesNotImplemented", code.clone()));
    }
    for code in &acknowledged.notices_not_in_canonical {
        items.push(("noticesNotInCanonical", code.clone()));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::{spec_changes, validate_spec_changes, SupportStatus};
    use crate::spec_baseline::spec_baseline;
    use crate::spec_surface::spec_surface;

    #[test]
    fn parses_the_bundled_changes() {
        let changes = spec_changes().expect("the committed changes must parse");
        assert!(!changes.entries.is_empty());
        assert_eq!(changes.window.since_date.len(), 10);
    }

    /// The gate: every published support claim has to match the build it
    /// describes, and every accepted difference has to match the baseline.
    #[test]
    fn committed_spec_changes_are_coherent() {
        let changes = spec_changes().expect("the committed changes must parse");
        let baseline = spec_baseline().expect("the committed baseline must parse");
        let problems = validate_spec_changes(changes, &spec_surface(), baseline);
        assert!(
            problems.is_empty(),
            "spec_changes.json disagrees with this build:\n  {}",
            problems.join("\n  ")
        );
    }

    #[test]
    fn detects_a_claim_the_build_does_not_support() {
        let mut changes = spec_changes().expect("parses").clone();
        let baseline = spec_baseline().expect("parses");
        let entry = changes
            .entries
            .iter_mut()
            .find(|entry| entry.status == SupportStatus::Supported)
            .expect("a supported entry to corrupt");
        entry.id = "trips.txt/not_a_real_column".to_string();

        let problems = validate_spec_changes(&changes, &spec_surface(), baseline);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("not_a_real_column")),
            "expected the missing column to be reported, got {problems:?}"
        );
    }

    #[test]
    fn detects_an_unexplained_accepted_difference() {
        let changes = spec_changes().expect("parses");
        let mut baseline = spec_baseline().expect("parses").clone();
        baseline
            .acknowledged
            .notices_not_in_canonical
            .push("invented_notice".to_string());

        let problems = validate_spec_changes(changes, &spec_surface(), &baseline);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("invented_notice")),
            "expected the unexplained difference to be reported, got {problems:?}"
        );
    }

    #[test]
    fn detects_an_upstream_url_that_is_not_http() {
        let mut changes = spec_changes().expect("parses").clone();
        let baseline = spec_baseline().expect("parses");
        changes.entries[0].upstream.url = "javascript:alert(1)".to_string();

        let problems = validate_spec_changes(&changes, &spec_surface(), baseline);
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("expected an http(s) address")),
            "expected the non-http URL to be reported, got {problems:?}"
        );
    }

    #[test]
    fn every_in_window_entry_is_newer_than_the_window() {
        let changes = spec_changes().expect("parses");
        for entry in changes.in_window() {
            let date = entry
                .upstream
                .date
                .as_deref()
                .expect("in-window entries are dated");
            assert!(date > changes.window.since_date.as_str());
        }
    }
}
