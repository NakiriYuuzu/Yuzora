//! The "what's new" notes (docs/beta2/release.md §3). Upstream's update_notes
//! asks here for the notes this build carries and for where a newer
//! release's notes are kept. The interface is English only, so these are the
//! English ones.
//!
//! CHANGELOG.md is English: Yuzora's entries above Herdr's. CHANGELOG.zh-TW.md
//! is a document for people who read Chinese (the release notes carry both);
//! the app does not show it, but a test keeps it in step with the English one.

use crate::update_notes::{Note, parse};

const ENGLISH: &str = include_str!("../../../CHANGELOG.md");

/// The notes this build carries.
pub fn bundled() -> Vec<Note> {
    parse(ENGLISH)
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
        assert_eq!(shown.len(), parse(ENGLISH).len());
        assert_eq!(shown[0].items[0].detail, parse(ENGLISH)[0].items[0].detail);
        assert_ne!(shown[0].items[0].detail, parse(CHINESE)[0].items[0].detail);
    }
}
