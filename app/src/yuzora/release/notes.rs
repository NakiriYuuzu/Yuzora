//! The "what's new" notes (docs/beta2/release.md §3). Upstream's update_notes
//! asks here for the notes this build carries and for where a newer
//! release's notes are kept. The interface is English only, so these are the
//! English ones.
//!
//! CHANGELOG.md is English: Yuzora's entries above Herdr's. Herdr numbers its
//! own releases, so its entry for a version is not Yuzora's release of that
//! number: the app shows Yuzora's entries only. CHANGELOG.zh-TW.md
//! is a document for people who read Chinese (the release notes carry both);
//! the app does not show it, but a test keeps it in step with the English one.

use crate::update_notes::{Note, parse};

const ENGLISH: &str = include_str!("../../../CHANGELOG.md");

/// The notes this build carries.
pub fn bundled() -> Vec<Note> {
    yuzoras(ENGLISH)
}

/// Yuzora's entries in a changelog. They run newest to oldest; Herdr's start
/// at the first version newer than the one above it.
pub fn yuzoras(text: &str) -> Vec<Note> {
    let mut notes = parse(text);
    let end = notes.windows(2).position(|pair| !crate::update::newer(&pair[0].version, &pair[1].version)).map_or(notes.len(), |i| i + 1);
    notes.truncate(end);
    notes
}

/// The app-home file keeping a newer release's notes.
pub const CACHE_FILE: &str = "update-notes-cache.md";

#[cfg(test)]
mod tests {
    use super::*;

    const CHINESE: &str = include_str!("../../../CHANGELOG.zh-TW.md");

    #[test]
    fn both_changelogs_open_with_this_version() {
        assert_eq!(parse(ENGLISH)[0].version, env!("CARGO_PKG_VERSION"));
        assert_eq!(parse(CHINESE)[0].version, env!("CARGO_PKG_VERSION"));
        assert_eq!(bundled()[0].version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn every_chinese_entry_matches_its_english_one() {
        let english = parse(ENGLISH);
        for zh in parse(CHINESE) {
            // Yuzora's entries come first, so the first of a version is Yuzora's.
            let en = english.iter().find(|n| n.version == zh.version).unwrap_or_else(|| panic!("{} is only in CHANGELOG.zh-TW.md", zh.version));
            assert_eq!(zh.date, en.date, "{}", zh.version);
            assert_eq!(zh.spotlight, en.spotlight, "{}", zh.version);
            let fixed = |n: &Note| n.items.iter().map(|i| i.fixed).collect::<Vec<_>>();
            assert_eq!(fixed(&zh), fixed(en), "{}: the same items under the same headings", zh.version);
        }
    }

    #[test]
    fn the_app_shows_the_english_notes_only() {
        let shown = bundled();
        assert_eq!(shown[0].items[0].detail, parse(ENGLISH)[0].items[0].detail);
        assert_ne!(shown[0].items[0].detail, parse(CHINESE)[0].items[0].detail);
    }

    #[test]
    fn the_app_shows_yuzoras_entries_and_none_of_herdrs() {
        // Herdr's 0.1.4 is not Yuzora's 0.1.4: it has another date, and a spotlight that would open What's new.
        let text = "## [0.1.4] - 2026-11-26\n### New\n- **Ours.** Yuzora's.\n\n## [0.1.0-beta.2] - 2026-11-04\n- First.\n\n## [0.1.5] - 2026-10-27\n<!-- spotlight -->\n- Herdr's.\n\n## [0.1.4] - 2026-10-23\n<!-- spotlight -->\n- Herdr's.\n";
        let shown = yuzoras(text);
        let seen: Vec<_> = shown.iter().map(|n| (n.version.as_str(), n.date.as_str(), n.spotlight)).collect();
        assert_eq!(seen, [("0.1.4", "2026-11-26", false), ("0.1.0-beta.2", "2026-11-04", false)]);
        assert_eq!(yuzoras("## [99.0.0] - 2026-10-30\n- Test release\n").len(), 1);

        let versions = |notes: &[Note]| notes.iter().map(|n| n.version.clone()).collect::<Vec<_>>();
        assert!(bundled().len() < parse(ENGLISH).len(), "with no Herdr entry in CHANGELOG.md, the next line checks nothing");
        assert_eq!(versions(&bundled()), versions(&parse(CHINESE)), "CHANGELOG.zh-TW.md holds Yuzora's entries only");
    }
}
