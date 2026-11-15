//! Deciding which Bluetooth advertisers are worth showing.
//!
//! A scan returns every device in range: phones, watches, headphones, and a
//! handful of unnamed beacons. None of that is useful when the operator wants
//! one label printer, so the list is filtered before it is drawn.
//!
//! Pure string logic on purpose: it is the part worth testing, and it needs no
//! adapter, no hardware and no async runtime.

/// Lower-case substrings that identify a label printer.
///
/// `by-482` and `by482` cover the PEDOOLO/BEEPRT advertising variants; `482`
/// catches firmware that drops the prefix; the rest cover the generic label
/// printer families this tool supports.
const PRINTER_HINTS: [&str; 12] = [
    "by-482", "by482", "482", "beeprt", "pedoolo", "printer", "label", "thermal", "tspl",
    "phomemo", "m220", "m110",
];

/// True when the advertised name looks like a label printer.
#[must_use]
pub fn looks_like_printer(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    PRINTER_HINTS.iter().any(|hint| lower.contains(hint))
}

/// True when `name` contains the filter text, case-insensitively.
///
/// An empty filter matches nothing: callers use [`is_visible`] to decide what
/// an empty filter means.
#[must_use]
pub fn matches_filter(name: &str, filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return false;
    }
    name.to_ascii_lowercase().contains(&filter)
}

/// Whether a device should appear in the Bluetooth list.
///
/// * `show_all` — everything, including unnamed advertisers.
/// * a non-empty `filter` — only names containing it, which is how the operator
///   narrows to the exact printer.
/// * otherwise — only names that look like label printers.
#[must_use]
pub fn is_visible(name: &str, filter: &str, show_all: bool) -> bool {
    if show_all {
        return true;
    }
    if !filter.trim().is_empty() {
        return matches_filter(name, filter);
    }
    looks_like_printer(name)
}

/// Sort key that puts likely printers first, then alphabetical.
#[must_use]
pub fn sort_key(name: &str) -> (u8, String) {
    let rank = if looks_like_printer(name) { 0 } else { 1 };
    (rank, name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{is_visible, looks_like_printer, matches_filter, sort_key};

    #[test]
    fn printer_names_are_recognised() {
        for name in [
            "BY-482BT",
            "by-482",
            "BEEPRT BY-482BT",
            "PEDOOLO BY-482BT",
            "BY482BT-1234",
            "Phomemo M220",
            "Label Printer",
            "Thermal_Label_Printe",
        ] {
            assert!(
                looks_like_printer(name),
                "{name} should look like a printer"
            );
        }
    }

    #[test]
    fn unrelated_devices_are_not_printers() {
        for name in [
            "Galaxy Buds",
            "iPhone",
            "Mi Band 7",
            "TP-Link Router",
            "(unnamed)",
            "",
        ] {
            assert!(
                !looks_like_printer(name),
                "{name} should not look like a printer"
            );
        }
    }

    #[test]
    fn default_view_hides_unrelated_devices() {
        assert!(is_visible("BY-482BT", "", false));
        assert!(!is_visible("Galaxy Buds", "", false));
        assert!(!is_visible("(unnamed)", "", false));
    }

    #[test]
    fn show_all_reveals_everything() {
        assert!(is_visible("Galaxy Buds", "", true));
        assert!(is_visible("(unnamed)", "", true));
    }

    #[test]
    fn filter_narrows_to_the_model() {
        assert!(is_visible("BY-482BT", "by-482", false));
        assert!(is_visible("BY-482BT-1234", "BY-482", false));
        assert!(!is_visible("Phomemo M220", "by-482", false));
        // The filter is case-insensitive and trimmed.
        assert!(is_visible("BY-482BT", "  bY-482  ", false));
    }

    #[test]
    fn filter_overrides_the_printer_heuristic() {
        // A filtered scan is explicit: if the operator typed a name, show what
        // matches instead of guessing.
        assert!(is_visible("Some Odd Name", "odd", false));
        assert!(!is_visible("BY-482BT", "odd", false));
    }

    #[test]
    fn matches_filter_ignores_empty_filters() {
        assert!(!matches_filter("BY-482BT", "   "));
    }

    #[test]
    fn sorting_puts_printers_first() {
        let mut names = ["iPhone", "Phomemo M220", "Galaxy Buds", "BY-482BT"];
        names.sort_by_key(|name| sort_key(name));

        assert_eq!(names[0], "BY-482BT");
        assert_eq!(names[1], "Phomemo M220");
        let tail: Vec<_> = names[2..].to_vec();
        assert_eq!(tail, vec!["Galaxy Buds", "iPhone"]);
    }
}
