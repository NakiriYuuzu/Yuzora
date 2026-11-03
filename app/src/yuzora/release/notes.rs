//! The "what's new" notes in the shown language (docs/beta2/release.md
//! §3, 更新說明). Upstream's update_notes asks here for the notes this build
//! carries and for where a newer release's notes are kept, per i18n locale.
//!
//! CHANGELOG.md is English: Yuzora's entries above Herdr's. CHANGELOG.zh-TW.md
//! has Yuzora's entries in Traditional Chinese, the same versions, headings
//! and items; a version it lacks (Herdr's) shows in English.

use crate::update_notes::{Note, parse};
use yuzora_core::i18n;

const ENGLISH: &str = include_str!("../../../CHANGELOG.md");
const CHINESE: &str = include_str!("../../../CHANGELOG.zh-TW.md");

/// The notes this build carries, in `locale`.
pub fn bundled(locale: &str) -> Vec<Note> {
    let english = parse(ENGLISH);
    if locale != i18n::ZH_TW {
        return english;
    }
    let mut notes = parse(CHINESE);
    let missing: Vec<Note> = english.into_iter().filter(|e| !notes.iter().any(|n| n.version == e.version)).collect();
    notes.extend(missing);
    notes
}

/// The app-home file keeping a newer release's notes in `locale`.
pub fn cache_file(locale: &str) -> &'static str {
    if locale == i18n::ZH_TW { "update-notes-cache.zh-TW.md" } else { "update-notes-cache.md" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_language_keeps_its_own_cache() {
        assert_eq!(cache_file(i18n::EN), "update-notes-cache.md");
        assert_eq!(cache_file(i18n::ZH_TW), "update-notes-cache.zh-TW.md");
    }

    #[test]
    fn both_changelogs_open_with_this_version() {
        assert_eq!(parse(ENGLISH)[0].version, env!("CARGO_PKG_VERSION"));
        assert_eq!(parse(CHINESE)[0].version, env!("CARGO_PKG_VERSION"));
        assert_eq!(bundled(i18n::EN)[0].version, env!("CARGO_PKG_VERSION"));
        assert_eq!(bundled(i18n::ZH_TW)[0].version, env!("CARGO_PKG_VERSION"));
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
    fn chinese_first_then_english_for_what_it_lacks() {
        let zh = bundled(i18n::ZH_TW);
        let current = zh.iter().find(|n| n.version == env!("CARGO_PKG_VERSION")).unwrap();
        assert_eq!(current.items[0].detail, parse(CHINESE)[0].items[0].detail);
        assert_ne!(current.items[0].detail, parse(ENGLISH)[0].items[0].detail);
        // Herdr's versions are only in English.
        let herdr = parse(ENGLISH).into_iter().find(|n| !parse(CHINESE).iter().any(|c| c.version == n.version)).unwrap();
        assert!(zh.iter().any(|n| n.version == herdr.version && n.items[0].detail == herdr.items[0].detail));
        assert_eq!(zh.iter().filter(|n| n.version == env!("CARGO_PKG_VERSION")).count(), 1);
        // Any other language is English.
        assert_eq!(bundled("fr").len(), parse(ENGLISH).len());
        assert_eq!(bundled(i18n::EN)[0].items[0].detail, parse(ENGLISH)[0].items[0].detail);
    }
}
