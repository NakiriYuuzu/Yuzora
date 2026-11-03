//! The "what's new" notes in the shown language (docs/beta2/release.md
//! §3, 更新說明). Upstream's update_notes asks here for the CHANGELOG this
//! build carries and for where a newer release's notes are kept, per i18n
//! locale.

use yuzora_core::i18n;

/// The CHANGELOG this build carries, in `locale`.
pub fn bundled(locale: &str) -> &'static str {
    // One CHANGELOG for every language until the Chinese one is written.
    let _ = locale;
    include_str!("../../../CHANGELOG.md")
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
        assert_eq!(bundled(i18n::EN), include_str!("../../../CHANGELOG.md"));
    }
}
