//! Feed text reaching the map popup must be escaped.
//!
//! `stop_name` comes straight from the feed's CSV, travels through the
//! `stop_too_far_from_shape` notice into `GeoError.stop_name`, and ends up in
//! the Leaflet popup. Concatenated raw, a name like
//! `<img src=x onerror="...">` runs as JavaScript in the app's main window the
//! moment the marker is opened. The frontend has no JS test harness, so this
//! guards the sink at the source level.

const FRONTEND: &str = include_str!("../frontend/index.html");

#[test]
fn stop_names_are_escaped_before_reaching_a_popup() {
    let popups: Vec<&str> = FRONTEND
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(".bindPopup("))
        .collect();
    assert!(!popups.is_empty(), "the map popups moved or were renamed");

    for line in popups {
        // Either the popup is a fixed string, or every value it interpolates
        // goes through escapeHtml.
        let interpolates_feed_data = line.contains("err.");
        assert!(
            !interpolates_feed_data || line.contains("escapeHtml("),
            "popup interpolates feed data unescaped: {line}"
        );
    }
}

#[test]
fn the_only_html_sink_escapes_its_values() {
    for (number, line) in FRONTEND.lines().enumerate() {
        let line = line.trim();
        if !line.contains(".innerHTML") || line.starts_with("//") {
            continue;
        }
        // `escapeHtml` itself reads innerHTML back out of a detached element;
        // every other assignment must feed on already-escaped markup.
        let allowed = line == "return element.innerHTML;" || line.contains("renderDiff(");
        assert!(allowed, "new innerHTML sink at line {}: {line}", number + 1);
    }
}
