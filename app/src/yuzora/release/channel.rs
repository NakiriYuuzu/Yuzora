//! Which releases an install updates to, and finding the newest one in
//! GitHub's release list (`GET /repos/NakiriYuuzu/Yuzora/releases`).
//!
//! - **stable**: releases that aren't pre-releases, versions without a
//!   pre-release part (`0.1.0`).
//! - **beta**: those and pre-releases (`0.1.0-beta.3`).
//!
//! A stable build starts on stable; the `update` section of Yuzora's
//! settings.json (`{"channel": "beta"}`) can pick beta. A beta build always
//! follows stable: betas are downloaded by hand, and `0.1.0` stable is
//! what takes a beta install over.
//! Only releases carrying a signed Yuzora manifest count: the Tauri app's
//! releases (0.1.0-beta.1 and older) have none.

use super::manifest::{MANIFEST, SIGNATURE};
use serde_json::Value;

/// The section of Yuzora's settings.json this lane owns.
pub const SECTION: &str = "update";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(name: &str) -> Option<Channel> {
        match name {
            "stable" => Some(Channel::Stable),
            "beta" => Some(Channel::Beta),
            _ => None,
        }
    }

    /// The channel a build of `version` follows. A beta build (a version with
    /// a pre-release part) only updates to stable releases, whatever the
    /// `update` section of the settings says; a stable build follows the
    /// section's `channel`, stable by default.
    pub fn chosen_from(settings: &Value, version: &str) -> Channel {
        if version.contains('-') {
            return Channel::Stable;
        }
        settings.get("channel").and_then(Value::as_str).and_then(Channel::parse).unwrap_or(Channel::Stable)
    }

    /// The channel this install follows now.
    pub fn chosen() -> Channel {
        Channel::chosen_from(&yuzora_core::settings::section(SECTION), env!("CARGO_PKG_VERSION"))
    }

    /// Whether a release of `version` belongs on this channel.
    pub fn takes(self, version: &str, prerelease: bool) -> bool {
        match self {
            Channel::Stable => !prerelease && !version.contains('-'),
            Channel::Beta => true,
        }
    }
}

/// A newer release that carries a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub version: String,
    pub manifest_url: String,
    pub signature_url: String,
}

/// One page of GitHub's release list (JSON), or why it isn't one.
pub fn releases(page: &[u8]) -> Result<Vec<Value>, String> {
    match serde_json::from_slice(page).map_err(|e| format!("release list: {e}"))? {
        Value::Array(list) => Ok(list),
        _ => Err("release list: not a list".into()),
    }
}

/// Whether the next (older) page could still hold something newer than
/// `running`: this page is full and every release on it is newer. On
/// stable, a run of betas can fill a page before the stable release.
pub fn reaches_older(page: &[Value], per_page: usize, running: &str) -> bool {
    page.len() >= per_page && !page.iter().any(|release| version_of(release).is_some_and(|v| !crate::update::newer(v, running)))
}

fn version_of(release: &Value) -> Option<&str> {
    release["tag_name"].as_str().and_then(|t| t.strip_prefix('v')).filter(|v| plausible(v))
}

/// [`pick_in`] for one page of GitHub's JSON list.
pub fn pick(releases: &[u8], channel: Channel, running: &str) -> Result<Option<Candidate>, String> {
    Ok(pick_in(&self::releases(releases)?, channel, running))
}

/// The newest release in `list` on `channel` that is newer than `running`
/// and has both the manifest and its signature. None: nothing newer. Drafts
/// and tags that aren't `v<version>` never count.
pub fn pick_in(list: &[Value], channel: Channel, running: &str) -> Option<Candidate> {
    let mut best: Option<Candidate> = None;
    for release in list {
        if release["draft"].as_bool() != Some(false) {
            continue;
        }
        let Some(version) = version_of(release) else {
            continue;
        };
        let prerelease = release["prerelease"].as_bool().unwrap_or(true);
        if !channel.takes(version, prerelease) || !crate::update::newer(version, running) {
            continue;
        }
        if best.as_ref().is_some_and(|b| !crate::update::newer(version, &b.version)) {
            continue;
        }
        let asset = |name: &str| {
            release["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(name))?["browser_download_url"].as_str().map(str::to_owned)
        };
        let (Some(manifest_url), Some(signature_url)) = (asset(MANIFEST), asset(SIGNATURE)) else {
            continue;
        };
        best = Some(Candidate { version: version.to_owned(), manifest_url, signature_url });
    }
    best
}

/// `0.1.0` or `0.1.0-beta.2`: three numbers, an optional pre-release of
/// letters, digits and dots. Keeps odd tags out of URLs and comparisons.
pub fn plausible(version: &str) -> bool {
    let (core, pre) = version.split_once('-').map_or((version, None), |(c, p)| (c, Some(p)));
    let numbers: Vec<&str> = core.split('.').collect();
    numbers.len() == 3
        && numbers.iter().all(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| !p.is_empty() && p.len() <= 32 && p.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release(tag: &str, prerelease: bool, draft: bool, signed: bool) -> Value {
        let mut assets = vec![json!({"name": "yuzora-0-macos-aarch64.tar.gz", "browser_download_url": format!("https://github.com/NakiriYuuzu/Yuzora/releases/download/{tag}/x.tar.gz")})];
        if signed {
            for name in [MANIFEST, SIGNATURE] {
                assets.push(json!({"name": name, "browser_download_url": format!("https://github.com/NakiriYuuzu/Yuzora/releases/download/{tag}/{name}")}));
            }
        }
        json!({"tag_name": tag, "prerelease": prerelease, "draft": draft, "assets": assets})
    }

    fn pick_from(list: Vec<Value>, channel: Channel, running: &str) -> Option<String> {
        pick(serde_json::to_vec(&list).unwrap().as_slice(), channel, running).unwrap().map(|c| c.version)
    }

    #[test]
    fn stable_builds_follow_settings_and_beta_builds_only_stable() {
        assert_eq!(Channel::chosen_from(&Value::Null, "0.1.0"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&json!({"channel": "beta"}), "0.1.0"), Channel::Beta);
        // A beta build takes nothing but stable releases, whatever settings say.
        assert_eq!(Channel::chosen_from(&Value::Null, "0.1.0-beta.2"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&json!({"channel": "stable"}), "0.1.0-beta.2"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&json!({"channel": "beta"}), "0.1.0-beta.2"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&json!({"channel": "nightly"}), "0.1.0"), Channel::Stable);
    }

    #[test]
    fn beta_takes_the_newest_signed_release_of_either_kind() {
        let list = vec![
            release("v0.1.0-beta.10", true, false, true),
            release("v0.1.0-beta.9", true, false, true),
            release("v0.1.0-beta.11", true, false, false), // no manifest
            release("v0.1.0-beta.12", true, true, true),   // draft
            release("v0.0.19", false, false, false),       // the Tauri app
        ];
        assert_eq!(pick_from(list.clone(), Channel::Beta, "0.1.0-beta.2").as_deref(), Some("0.1.0-beta.10"));
        assert_eq!(pick_from(list.clone(), Channel::Beta, "0.1.0-beta.10"), None);
        assert_eq!(pick_from(list, Channel::Stable, "0.1.0-beta.2"), None);
        let with_stable = vec![release("v0.1.0-beta.3", true, false, true), release("v0.1.0", false, false, true)];
        assert_eq!(pick_from(with_stable.clone(), Channel::Beta, "0.1.0-beta.2").as_deref(), Some("0.1.0"));
        assert_eq!(pick_from(with_stable, Channel::Stable, "0.1.0-beta.2").as_deref(), Some("0.1.0"));
    }

    #[test]
    fn a_beta_build_set_to_beta_sees_0_1_0_but_no_newer_beta() {
        let settings = json!({"channel": "beta"});
        let channel = Channel::chosen_from(&settings, "0.1.0-beta.2");
        let betas = vec![release("v0.1.0-beta.3", true, false, true), release("v0.1.0-beta.4", true, false, true)];
        assert_eq!(pick_from(betas.clone(), channel, "0.1.0-beta.2"), None);
        let mut with_stable = betas;
        with_stable.push(release("v0.1.0", false, false, true));
        assert_eq!(pick_from(with_stable, channel, "0.1.0-beta.2").as_deref(), Some("0.1.0"));
    }

    #[test]
    fn stable_skips_pre_releases_even_when_mislabeled() {
        let list = vec![release("v0.2.0-beta.1", false, false, true), release("v0.2.0", true, false, true), release("v0.1.1", false, false, true)];
        assert_eq!(pick_from(list, Channel::Stable, "0.1.0").as_deref(), Some("0.1.1"));
    }

    #[test]
    fn odd_tags_and_lists_never_count() {
        let list = vec![release("0.2.0", false, false, true), release("v0.2", false, false, true), release("v0.2.0/../x", false, false, true), release("v0.2.0-beta.1?x", true, false, true)];
        assert_eq!(pick_from(list, Channel::Beta, "0.1.0"), None);
        assert!(pick(b"{\"message\":\"API rate limit exceeded\"}", Channel::Beta, "0.1.0").is_err());
        assert!(pick(b"<html>", Channel::Beta, "0.1.0").is_err());
        let found = pick(serde_json::to_vec(&vec![release("v0.1.0-beta.3", true, false, true)]).unwrap().as_slice(), Channel::Beta, "0.1.0-beta.2").unwrap().unwrap();
        assert_eq!(found.manifest_url, format!("https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.3/{MANIFEST}"));
        assert_eq!(found.signature_url, format!("https://github.com/NakiriYuuzu/Yuzora/releases/download/v0.1.0-beta.3/{SIGNATURE}"));
    }

    #[test]
    fn only_a_full_page_of_newer_releases_reads_on() {
        let page = |tags: &[&str]| tags.iter().map(|t| release(t, true, false, false)).collect::<Vec<_>>();
        assert!(reaches_older(&page(&["v0.2.0-beta.3", "v0.2.0-beta.2"]), 2, "0.1.0"));
        assert!(!reaches_older(&page(&["v0.2.0-beta.3", "v0.2.0-beta.2"]), 3, "0.1.0"), "the last page");
        assert!(!reaches_older(&page(&["v0.2.0-beta.3", "v0.1.0"]), 2, "0.1.0"), "reached this version");
        assert!(reaches_older(&page(&["v0.2.0-beta.3", "odd"]), 2, "0.1.0"), "odd tags don't stop it");
    }

    #[test]
    fn plausible_versions() {
        for v in ["0.1.0", "0.1.0-beta.2", "10.20.30-rc.1"] {
            assert!(plausible(v), "{v}");
        }
        for v in ["0.1", "0.1.0.0", "0.1.0-", "0.1.0-beta..1", "0.1.0-beta/1", "a.b.c", "0.1.0-beta 1", ""] {
            assert!(!plausible(v), "{v}");
        }
    }
}
