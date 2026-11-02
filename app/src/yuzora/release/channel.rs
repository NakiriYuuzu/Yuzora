//! Which releases an install updates to, and finding the newest one in
//! GitHub's release list (`GET /repos/NakiriYuuzu/Yuzora/releases`).
//!
//! - **stable**: releases that aren't pre-releases, versions without a
//!   pre-release part (`0.1.0`).
//! - **beta**: those and pre-releases (`0.1.0-beta.3`).
//!
//! A beta build starts on beta, a stable build on stable; the `update`
//! section of Yuzora's settings.json (`{"channel": "stable"}`) picks.
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

    /// Where a build of `version` starts: a pre-release on beta.
    pub fn of_build(version: &str) -> Channel {
        if version.contains('-') { Channel::Beta } else { Channel::Stable }
    }

    /// The channel `settings` (the `update` section) picks, else the build's.
    pub fn chosen_from(settings: &Value, version: &str) -> Channel {
        settings.get("channel").and_then(Value::as_str).and_then(Channel::parse).unwrap_or_else(|| Channel::of_build(version))
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

/// The newest release in `releases` (GitHub's JSON list) on `channel` that is
/// newer than `running` and has both the manifest and its signature. None:
/// nothing newer. Drafts and tags that aren't `v<version>` never count.
pub fn pick(releases: &[u8], channel: Channel, running: &str) -> Result<Option<Candidate>, String> {
    let list: Value = serde_json::from_slice(releases).map_err(|e| format!("release list: {e}"))?;
    let list = list.as_array().ok_or("release list: not a list")?;
    let mut best: Option<Candidate> = None;
    for release in list {
        if release["draft"].as_bool() != Some(false) {
            continue;
        }
        let Some(version) = release["tag_name"].as_str().and_then(|t| t.strip_prefix('v')).filter(|v| plausible(v)) else {
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
    Ok(best)
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
    fn builds_start_on_their_own_channel_and_settings_pick() {
        assert_eq!(Channel::of_build("0.1.0-beta.2"), Channel::Beta);
        assert_eq!(Channel::of_build("0.1.0"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&Value::Null, "0.1.0-beta.2"), Channel::Beta);
        assert_eq!(Channel::chosen_from(&json!({"channel": "stable"}), "0.1.0-beta.2"), Channel::Stable);
        assert_eq!(Channel::chosen_from(&json!({"channel": "beta"}), "0.1.0"), Channel::Beta);
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
    fn plausible_versions() {
        for v in ["0.1.0", "0.1.0-beta.2", "10.20.30-rc.1"] {
            assert!(plausible(v), "{v}");
        }
        for v in ["0.1", "0.1.0.0", "0.1.0-", "0.1.0-beta..1", "0.1.0-beta/1", "a.b.c", "0.1.0-beta 1", ""] {
            assert!(!plausible(v), "{v}");
        }
    }
}
