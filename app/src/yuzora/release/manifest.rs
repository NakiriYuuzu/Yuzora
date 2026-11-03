//! The update manifest: what a release offers, and the proof it came from
//! Yuzora's release key.
//!
//! Each release carries `yuzora-update.json` and `yuzora-update.json.sig`, a
//! minisign signature of the manifest's exact bytes (prehashed, as the
//! `tauri signer sign` and `minisign -S` tools write it; base64-wrapped or
//! not). The manifest names the archive for each target with its SHA-256, so
//! a verified manifest vouches for the download (scripts/yuzora/manifest.ts):
//!
//! ```json
//! { "schema": 1, "name": "yuzora", "version": "0.1.0-beta.3", "channel": "beta",
//!   "notes": { "url": ".../v0.1.0-beta.3/changelog.md", "sha256": "…" },
//!   "localized_notes": { "zh-TW": { "url": ".../v0.1.0-beta.3/changelog.zh-TW.md", "sha256": "…" } },
//!   "targets": { "macos-aarch64": { "url": ".../v0.1.0-beta.3/yuzora-0.1.0-beta.3-macos-aarch64.tar.gz",
//!                                   "sha256": "…", "size": 123 } } }
//! ```
//!
//! Nothing is believed before the signature checks out, and a signed
//! manifest must still be for this release: the right name and version, a
//! channel matching the version, files only under that release's downloads.

use base64::Engine;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MANIFEST: &str = "yuzora-update.json";
pub const SIGNATURE: &str = "yuzora-update.json.sig";
/// The manifest format this app reads; a newer one is refused, not guessed at.
pub const SCHEMA: u64 = 1;

/// Minisign public keys (the key line) whose manifests the app accepts.
/// The release key, `742456BCCDC22A24`, is the one the Tauri app (0.1.0-beta.1
/// and older) already trusts for its updates (src-tauri/tauri.conf.json). A
/// new key ships here, in an update signed with the old one, before
/// releases are signed with it (docs/beta2/release.md, "Keys").
pub const TRUSTED_KEYS: &[&str] = &["RWQkKsLNvFYkdJ+BHNkJQTGVWAT+niNUk3v7Oy+hNXgeDn7tpkby/0QZ"];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u64,
    pub name: String,
    pub version: String,
    pub channel: String,
    /// The release's CHANGELOG.md, for the "what's new" notes.
    #[serde(default)]
    pub notes: Option<Asset>,
    /// The same notes in another language, by i18n locale (`zh-TW`). Older
    /// manifests have none; an older app skips them and keeps `notes`.
    #[serde(default)]
    pub localized_notes: BTreeMap<String, Asset>,
    pub targets: BTreeMap<String, Asset>,
}

impl Manifest {
    /// The notes in `locale`, or the English ones when the release has none in it.
    pub fn notes_in(&self, locale: &str) -> Option<&Asset> {
        self.localized_notes.get(locale).or(self.notes.as_ref())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

/// Checks `signature` against `keys`, then that the manifest is for release
/// `version` with every file under `downloads/v<version>/`.
pub fn read(manifest: &[u8], signature: &[u8], keys: &[String], version: &str, downloads: &str) -> Result<Manifest, String> {
    verify(manifest, signature, keys)?;
    let m: Manifest = serde_json::from_slice(manifest).map_err(|e| format!("{MANIFEST}: {e}"))?;
    checks(&m, version, downloads)?;
    Ok(m)
}

/// What a verified manifest must still be: see [`read`].
fn checks(m: &Manifest, version: &str, downloads: &str) -> Result<(), String> {
    if m.schema != SCHEMA {
        return Err(format!("{MANIFEST}: schema {} (this app reads {SCHEMA}); update Yuzora by hand", m.schema));
    }
    if m.name != "yuzora" {
        return Err(format!("{MANIFEST}: for {:?}, not yuzora", m.name));
    }
    if m.version != version {
        return Err(format!("{MANIFEST}: for {}, but the release is {version}", m.version));
    }
    let channel = if version.contains('-') { "beta" } else { "stable" };
    if m.channel != channel {
        return Err(format!("{MANIFEST}: channel {:?} for {version}", m.channel));
    }
    let prefix = format!("{downloads}/v{version}/");
    // Every file, notes in any language too. A locale this app doesn't show
    // is still checked, and never refused for being unknown.
    let notes = m.notes.iter().map(|a| ("notes".to_owned(), a));
    let localized = m.localized_notes.iter().map(|(locale, a)| (format!("notes {locale}"), a));
    for (what, asset) in m.targets.iter().map(|(t, a)| (t.clone(), a)).chain(notes).chain(localized) {
        let name = asset.url.strip_prefix(&prefix).ok_or_else(|| format!("{MANIFEST}: {what} is not in this release: {}", asset.url))?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
            return Err(format!("{MANIFEST}: {what}: odd file name {name:?}"));
        }
        if asset.sha256.len() != 64 || !asset.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("{MANIFEST}: {what}: sha256 must be 64 lowercase hex digits"));
        }
    }
    Ok(())
}

/// Whether one of `keys` signed exactly these bytes.
pub fn verify(bytes: &[u8], signature: &[u8], keys: &[String]) -> Result<(), String> {
    let signature = decode_signature(signature)?;
    let mut last = String::from("no trusted keys");
    for key in keys {
        let key = minisign_verify::PublicKey::from_base64(key).map_err(|e| format!("trusted key {key}: {e}"))?;
        // Prehashed only: the legacy algorithm is for files minisign signed before 0.8.
        match key.verify(bytes, &signature, false) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("{SIGNATURE}: {last}"))
}

/// A minisign signature file's text, or that text base64-encoded (Tauri's `.sig`).
fn decode_signature(raw: &[u8]) -> Result<minisign_verify::Signature, String> {
    let text = std::str::from_utf8(raw).map_err(|_| format!("{SIGNATURE}: not text"))?.trim();
    let text = if text.starts_with("untrusted comment:") {
        text.to_owned()
    } else {
        let bytes = base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| format!("{SIGNATURE}: {e}"))?;
        String::from_utf8(bytes).map_err(|_| format!("{SIGNATURE}: not a minisign signature"))?
    };
    minisign_verify::Signature::decode(&text).map_err(|e| format!("{SIGNATURE}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway key made for these tests only (its secret half was deleted
    // after signing the fixtures); never trusted by the app.
    const TEST_KEY: &str = include_str!("fixtures/test-key.pub");
    const MANIFEST_FIXTURE: &[u8] = include_bytes!("fixtures/yuzora-update.json");
    const SIGNATURE_FIXTURE: &[u8] = include_bytes!("fixtures/yuzora-update.json.sig");
    const DOWNLOADS: &str = "https://github.com/NakiriYuuzu/Yuzora/releases/download";

    fn test_keys() -> Vec<String> {
        vec![TEST_KEY.trim().to_owned()]
    }

    #[test]
    fn a_signed_manifest_reads() {
        let m = read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &test_keys(), "0.1.0-beta.3", DOWNLOADS).unwrap();
        assert_eq!((m.name.as_str(), m.version.as_str(), m.channel.as_str()), ("yuzora", "0.1.0-beta.3", "beta"));
        assert!(m.targets.contains_key("macos-aarch64") && m.targets.contains_key("windows-x86_64"));
        assert!(m.notes.is_some());
        // Written before notes came in more languages: Chinese falls back to English.
        assert!(m.localized_notes.is_empty());
        assert_eq!(m.notes_in(yuzora_core::i18n::ZH_TW), m.notes.as_ref());
    }

    #[test]
    fn notes_in_the_shown_language_or_english() {
        let good: serde_json::Value = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        let url = |name: &str| format!("{DOWNLOADS}/v0.1.0-beta.3/{name}");
        let mut m = good.clone();
        m["localized_notes"] = serde_json::json!({ "zh-TW": { "url": url("changelog.zh-TW.md"), "sha256": "5".repeat(64) } });
        let m: Manifest = serde_json::from_value(m).unwrap();
        assert!(checks(&m, "0.1.0-beta.3", DOWNLOADS).is_ok());
        assert_eq!(m.notes_in(yuzora_core::i18n::ZH_TW).unwrap().url, url("changelog.zh-TW.md"));
        assert_eq!(m.notes_in(yuzora_core::i18n::EN).unwrap().url, url("changelog.md"));
        // Notes only in a language this app doesn't show: still checked, and Chinese gets English.
        let mut m = good;
        m["localized_notes"] = serde_json::json!({ "ja": { "url": url("changelog.ja.md"), "sha256": "6".repeat(64) } });
        let m: Manifest = serde_json::from_value(m).unwrap();
        assert!(checks(&m, "0.1.0-beta.3", DOWNLOADS).is_ok());
        assert_eq!(m.notes_in(yuzora_core::i18n::ZH_TW).unwrap().url, url("changelog.md"));
    }

    #[test]
    fn the_signature_is_checked_first_and_fails_closed() {
        // The release key did not sign the fixture.
        let release: Vec<String> = TRUSTED_KEYS.iter().map(|k| (*k).to_owned()).collect();
        let err = read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &release, "0.1.0-beta.3", DOWNLOADS).unwrap_err();
        assert!(err.contains("different key"), "{err}");
        // One changed byte.
        let mut tampered = MANIFEST_FIXTURE.to_vec();
        let at = tampered.iter().position(|&b| b == b'3').unwrap();
        tampered[at] = b'4';
        assert!(read(&tampered, SIGNATURE_FIXTURE, &test_keys(), "0.1.0-beta.3", DOWNLOADS).unwrap_err().contains("verification failed"));
        // No signature, junk, an empty key list.
        assert!(read(MANIFEST_FIXTURE, b"", &test_keys(), "0.1.0-beta.3", DOWNLOADS).is_err());
        assert!(read(MANIFEST_FIXTURE, b"untrusted comment: x\nnot base64\n", &test_keys(), "0.1.0-beta.3", DOWNLOADS).is_err());
        assert!(read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &[], "0.1.0-beta.3", DOWNLOADS).unwrap_err().contains("no trusted keys"));
        // A second key in the list still lets the right one through.
        let both = vec![release[0].clone(), test_keys()[0].clone()];
        assert!(read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &both, "0.1.0-beta.3", DOWNLOADS).is_ok());
    }

    #[test]
    fn plain_and_base64_signatures_both_read() {
        let raw = base64::engine::general_purpose::STANDARD.decode(std::str::from_utf8(SIGNATURE_FIXTURE).unwrap().trim()).unwrap();
        assert!(String::from_utf8_lossy(&raw).starts_with("untrusted comment:"));
        assert!(verify(MANIFEST_FIXTURE, &raw, &test_keys()).is_ok());
        assert!(verify(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &test_keys()).is_ok());
    }

    #[test]
    fn a_signed_manifest_must_be_for_this_release() {
        let keys = test_keys();
        assert!(read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &keys, "0.1.0-beta.4", DOWNLOADS).unwrap_err().contains("but the release is 0.1.0-beta.4"));
        assert!(read(MANIFEST_FIXTURE, SIGNATURE_FIXTURE, &keys, "0.1.0-beta.3", "https://example.com/releases/download").unwrap_err().contains("not in this release"));
    }

    #[test]
    fn checks_after_the_signature() {
        // Unsigned variants of the fixture reach the checks behind the signature.
        let check = |m: serde_json::Value, version: &str| -> Result<Manifest, String> {
            let m: Manifest = serde_json::from_value(m).map_err(|e| e.to_string())?;
            checks(&m, version, DOWNLOADS).map(|()| m)
        };
        let good: serde_json::Value = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        assert!(check(good.clone(), "0.1.0-beta.3").is_ok());
        let mut m = good.clone();
        m["schema"] = 2.into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("schema 2"));
        let mut m = good.clone();
        m["name"] = "herdr-app".into();
        assert!(check(m, "0.1.0-beta.3").is_err());
        let mut m = good.clone();
        m["channel"] = "stable".into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("channel"));
        let mut m = good.clone();
        m["targets"]["macos-aarch64"]["url"] = format!("{DOWNLOADS}/v0.1.0-beta.3/../v0.0.1/x.tar.gz").into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("odd file name"));
        let mut m = good.clone();
        m["targets"]["macos-aarch64"]["url"] = format!("{DOWNLOADS}/v0.1.0-beta.2/x.tar.gz").into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("not in this release"));
        let mut m = good.clone();
        m["targets"]["macos-aarch64"]["sha256"] = "ABC".into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("sha256"));
        let mut m = good.clone();
        m["notes"]["url"] = "https://media.herdr.dev/x.md".into();
        assert!(check(m, "0.1.0-beta.3").unwrap_err().contains("notes"));
        // The Chinese notes are held to the same rules.
        let zh = |url: String, sha256: String| {
            let mut m = good.clone();
            m["localized_notes"] = serde_json::json!({ "zh-TW": { "url": url, "sha256": sha256 } });
            check(m, "0.1.0-beta.3")
        };
        let err = zh(format!("{DOWNLOADS}/v0.1.0-beta.3/changelog.zh-TW.md"), "A".repeat(64)).unwrap_err();
        assert!(err.contains("notes zh-TW: sha256"), "{err}");
        let err = zh("https://media.herdr.dev/changelog.zh-TW.md".into(), "5".repeat(64)).unwrap_err();
        assert!(err.contains("notes zh-TW is not in this release"), "{err}");
        let err = zh(format!("{DOWNLOADS}/v0.1.0-beta.3/zh/../changelog.md"), "5".repeat(64)).unwrap_err();
        assert!(err.contains("notes zh-TW: odd file name"), "{err}");
    }
}
