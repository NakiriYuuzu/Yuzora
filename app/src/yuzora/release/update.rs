//! Looking for an update: the newest release on this install's channel
//! ([`super::channel`]), its signed manifest ([`super::manifest`]), then
//! upstream's download, check and stage beside the install
//! (`crate::update::stage`). Upstream's clock, "Restart" and swap stay as
//! they are (src/update.rs).
//!
//! Only an installed release build looks (`HERDR_APP_RELEASE` at build time,
//! scripts/yuzora/package), or any build pointed at a test release server
//! with `YUZORA_UPDATE_URL`. Whatever the server, the manifest must be signed
//! by a trusted key.

use super::channel::{self, Channel};
use super::manifest::{self, Manifest};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A test release server standing in for github.com and api.github.com: it
/// serves `/repos/NakiriYuuzu/Yuzora/releases` and the release downloads.
pub const URL_ENV: &str = "YUZORA_UPDATE_URL";
/// Debug builds only: one more trusted key, the test server's.
pub const TEST_KEY_ENV: &str = "YUZORA_UPDATE_TEST_KEY";
const REPO: &str = "NakiriYuuzu/Yuzora";
/// Downloads the app reads into memory (the release list, a manifest, notes).
const MAX_FETCH: &str = "8388608";
/// Releases per page of the list (GitHub's most), and the most pages read.
const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 5;

/// Where releases are listed and downloaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// GitHub's release list, newest first.
    pub list: String,
    /// The prefix of every release download: `<downloads>/v<version>/<file>`.
    pub downloads: String,
}

impl Source {
    pub fn github() -> Source {
        Source { list: format!("https://api.github.com/repos/{REPO}/releases?per_page={PER_PAGE}"), downloads: format!("https://github.com/{REPO}/releases/download") }
    }

    /// A server laid out like GitHub at `base` (tests).
    pub fn at(base: &str) -> Source {
        let base = base.trim_end_matches('/');
        Source { list: format!("{base}/repos/{REPO}/releases?per_page={PER_PAGE}"), downloads: format!("{base}/{REPO}/releases/download") }
    }

    fn from_env() -> Source {
        std::env::var(URL_ENV).ok().filter(|u| !u.is_empty()).map_or_else(Source::github, |base| Source::at(&base))
    }
}

/// Whether this install updates itself: upstream's `checks_app`, for Yuzora.
pub fn checks_app(root: Option<PathBuf>) -> Option<PathBuf> {
    root.filter(|_| option_env!("HERDR_APP_RELEASE").is_some() || std::env::var_os(URL_ENV).is_some())
}

/// Whether a staged `version` may still be applied: the channel this install
/// follows now takes it (upstream's `staged_version` asks).
pub fn keeps(version: &str) -> bool {
    keeps_on(Channel::chosen(), version)
}

/// [`keeps`] for the channel a build of `running` follows given `settings`.
#[cfg(test)]
fn keeps_for(settings: &serde_json::Value, running: &str, version: &str) -> bool {
    keeps_on(Channel::chosen_from(settings, running), version)
}

fn keeps_on(channel: Channel, version: &str) -> bool {
    channel.takes(version, false)
}

/// Upstream's `check`, for Yuzora: Ok(Some(version)) once a newer version is
/// downloaded, checked and staged beside `root`.
pub fn check(root: &Path) -> Result<Option<String>, String> {
    // Drops a stage that's off the channel now, and its "Restart" with it.
    let _ = crate::update::staged_version(root);
    let Some(found) = find(&Source::from_env(), Channel::chosen(), env!("CARGO_PKG_VERSION"), &keys())? else {
        return Ok(None);
    };
    // Notes are optional: a missing or damaged changelog never holds up the
    // update. Each language is kept, so switching shows the other at once.
    for (locale, notes) in notes_to_fetch(&found, crate::update_notes::has) {
        match fetch_asset(notes) {
            Ok(text) => crate::update_notes::cache(locale, &String::from_utf8_lossy(&text)),
            Err(e) => tracing::warn!("update notes ({locale}) for {}: {e}", found.version),
        }
    }
    let Some(entry) = found.targets.get(&super::target()) else {
        tracing::info!("Yuzora {} has no build for {}", found.version, super::target());
        return Ok(None);
    };
    crate::update::stage(root, &found.version, &entry.url, &entry.sha256)
}

/// The notes to download for `found`: per language, the asset that language
/// shows (its own, else the English one), unless `has(locale, version)` says
/// that language's notes are already kept.
fn notes_to_fetch<'a>(found: &'a Manifest, has: impl Fn(&'static str, &str) -> bool) -> Vec<(&'static str, &'a manifest::Asset)> {
    yuzora_core::i18n::LOCALES.iter().filter_map(|&locale| Some((locale, found.notes_in(locale)?))).filter(|(locale, _)| !has(locale, &found.version)).collect()
}

/// The newest release on `channel` newer than `running`, with its manifest
/// checked against `keys`. None: nothing newer (or nothing signed).
pub fn find(source: &Source, channel: Channel, running: &str, keys: &[String]) -> Result<Option<Manifest>, String> {
    let mut list = Vec::new();
    for page in 1..=MAX_PAGES {
        let url = if page == 1 { source.list.clone() } else { format!("{}&page={page}", source.list) };
        let releases = channel::releases(&fetch(&url, true)?.ok_or_else(|| format!("{url}: not found"))?)?;
        let more = channel::reaches_older(&releases, PER_PAGE, running);
        list.extend(releases);
        if !more {
            break;
        }
    }
    let Some(candidate) = channel::pick_in(&list, channel, running) else {
        return Ok(None);
    };
    let bytes = fetch(&candidate.manifest_url, false)?.ok_or_else(|| format!("{}: not found", candidate.manifest_url))?;
    let signature = fetch(&candidate.signature_url, false)?.ok_or_else(|| format!("{}: not found", candidate.signature_url))?;
    manifest::read(&bytes, &signature, keys, &candidate.version, &source.downloads).map(Some)
}

/// The trusted keys: the release key, and in a debug build the test server's.
fn keys() -> Vec<String> {
    let mut keys: Vec<String> = manifest::TRUSTED_KEYS.iter().map(|k| (*k).to_owned()).collect();
    if cfg!(debug_assertions)
        && option_env!("HERDR_APP_RELEASE").is_none()
        && let Some(key) = std::env::var(TEST_KEY_ENV).ok().filter(|k| !k.is_empty())
    {
        keys.push(key);
    }
    keys
}

/// A file the signed manifest vouches for, checked against its sha256.
fn fetch_asset(asset: &manifest::Asset) -> Result<Vec<u8>, String> {
    let bytes = fetch(&asset.url, false)?.ok_or_else(|| format!("{}: not found", asset.url))?;
    if crate::update::sha256(&bytes) != asset.sha256 {
        return Err(format!("{}: checksum mismatch", asset.url));
    }
    Ok(bytes)
}

/// GETs `url` (redirects followed). Ok(None): 404. `api`: GitHub's JSON API.
fn fetch(url: &str, api: bool) -> Result<Option<Vec<u8>>, String> {
    let mut curl = crate::spawn::command(crate::update::system("curl"));
    curl.args(["-sSL", "--max-time", "30", "--max-filesize", MAX_FETCH, "-A", concat!("Yuzora/", env!("CARGO_PKG_VERSION")), "-w", "\n%{http_code}"]);
    if api {
        curl.args(["-H", "Accept: application/vnd.github+json", "-H", "X-GitHub-Api-Version: 2022-11-28"]);
    }
    let out = curl.arg(url).stdin(Stdio::null()).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let split = out.stdout.iter().rposition(|&b| b == b'\n').ok_or_else(|| format!("{url}: no status"))?;
    let (body, code) = (&out.stdout[..split], String::from_utf8_lossy(&out.stdout[split + 1..]).into_owned());
    match code.as_str() {
        "200" => Ok(Some(body.to_vec())),
        "404" => Ok(None),
        // GitHub answers 403 with a message when the hourly API limit is used up.
        _ => Err(format!("{url}: HTTP {code} {}", String::from_utf8_lossy(&body[..body.len().min(200)]).trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const MANIFEST_FIXTURE: &[u8] = include_bytes!("fixtures/yuzora-update.json");
    const SIGNATURE_FIXTURE: &[u8] = include_bytes!("fixtures/yuzora-update.json.sig");
    const TEST_KEY: &str = include_str!("fixtures/test-key.pub");

    fn test_keys() -> Vec<String> {
        vec![TEST_KEY.trim().to_owned()]
    }

    /// A one-thread release server answering `requests` requests from
    /// `routes` (path, status, body); anything else is a 404. Returns what it saw.
    fn serve_on(listener: TcpListener, routes: Vec<(String, u16, Vec<u8>)>, requests: usize) -> std::thread::JoinHandle<Vec<String>> {
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let path = request.split(' ').nth(1).unwrap_or_default().to_owned();
                let (status, body) = routes.iter().find(|(p, _, _)| *p == path).map_or((404, b"Not Found".to_vec()), |(_, s, b)| (*s, b.clone()));
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(&body).unwrap();
                seen.push(request);
            }
            seen
        })
    }

    /// GitHub's list with `tag` (carrying the manifest and its signature on
    /// this server) and the Tauri app's last stable release.
    fn list(base: &str, tag: &str) -> Vec<u8> {
        let asset = |name: &str| serde_json::json!({"name": name, "browser_download_url": format!("{base}/dl/{tag}/{name}")});
        serde_json::to_vec(&serde_json::json!([
            {"tag_name": tag, "draft": false, "prerelease": true, "assets": [asset(manifest::MANIFEST), asset(manifest::SIGNATURE)]},
            {"tag_name": "v0.0.19", "draft": false, "prerelease": false, "assets": [asset("latest.json")]},
        ]))
        .unwrap()
    }

    /// Serves the list for `tag`, then `manifest` and `signature` (None: 404).
    fn serve_release(tag: &str, manifest: &[u8], signature: Option<&[u8]>, requests: usize) -> (Source, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut routes = vec![
            (format!("/repos/{REPO}/releases?per_page={PER_PAGE}"), 200, list(&base, tag)),
            (format!("/dl/{tag}/{}", manifest::MANIFEST), 200, manifest.to_vec()),
        ];
        if let Some(signature) = signature {
            routes.push((format!("/dl/{tag}/{}", manifest::SIGNATURE), 200, signature.to_vec()));
        }
        // The list and files come from here; the fixture was signed with
        // GitHub's download URLs inside, so those stay GitHub's.
        let source = Source { list: format!("{base}/repos/{REPO}/releases?per_page={PER_PAGE}"), downloads: Source::github().downloads };
        (source, serve_on(listener, routes, requests))
    }

    #[test]
    fn finds_a_signed_newer_release() {
        let (source, server) = serve_release("v0.1.0-beta.3", MANIFEST_FIXTURE, Some(SIGNATURE_FIXTURE), 3);
        let found = find(&source, Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap().unwrap();
        assert_eq!(found.version, "0.1.0-beta.3");
        assert!(found.targets.contains_key("macos-aarch64"));
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("GET /repos/NakiriYuuzu/Yuzora/releases?per_page=100 HTTP/1.1\r\n"));
        assert!(requests[0].contains("Accept: application/vnd.github+json"));
        assert!(requests[0].contains(concat!("User-Agent: Yuzora/", env!("CARGO_PKG_VERSION"))));
        assert!(requests[2].starts_with("GET /dl/v0.1.0-beta.3/yuzora-update.json.sig "));
    }

    #[test]
    fn a_page_full_of_newer_releases_reads_the_next() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // 100 newer releases without a manifest fill the first page.
        let full: Vec<serde_json::Value> = (0..PER_PAGE).map(|n| serde_json::json!({"tag_name": format!("v0.1.0-rc.{n}"), "draft": false, "prerelease": true, "assets": []})).collect();
        let tag = "v0.1.0-beta.3";
        let routes = vec![
            (format!("/repos/{REPO}/releases?per_page={PER_PAGE}"), 200, serde_json::to_vec(&full).unwrap()),
            (format!("/repos/{REPO}/releases?per_page={PER_PAGE}&page=2"), 200, list(&base, tag)),
            (format!("/dl/{tag}/{}", manifest::MANIFEST), 200, MANIFEST_FIXTURE.to_vec()),
            (format!("/dl/{tag}/{}", manifest::SIGNATURE), 200, SIGNATURE_FIXTURE.to_vec()),
        ];
        let server = serve_on(listener, routes, 4);
        let source = Source { list: format!("{base}/repos/{REPO}/releases?per_page={PER_PAGE}"), downloads: Source::github().downloads };
        assert_eq!(find(&source, Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap().unwrap().version, "0.1.0-beta.3");
        let requests = server.join().unwrap();
        assert!(requests[1].starts_with("GET /repos/NakiriYuuzu/Yuzora/releases?per_page=100&page=2 "), "{}", requests[1]);
    }

    #[test]
    fn nothing_newer_or_off_channel_reads_only_the_list() {
        let (source, server) = serve_release("v0.1.0-beta.3", MANIFEST_FIXTURE, Some(SIGNATURE_FIXTURE), 2);
        assert_eq!(find(&source, Channel::Beta, "0.1.0-beta.3", &test_keys()), Ok(None));
        assert_eq!(find(&source, Channel::Stable, "0.1.0-beta.2", &test_keys()), Ok(None));
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn unsigned_tampered_or_foreign_manifests_never_pass() {
        let (source, server) = serve_release("v0.1.0-beta.3", MANIFEST_FIXTURE, None, 3);
        assert!(find(&source, Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap_err().contains("yuzora-update.json.sig: not found"));
        server.join().unwrap();

        let tampered = String::from_utf8_lossy(MANIFEST_FIXTURE).replace("0000000000000000000000000000000000000000000000000000000000000003", "1111111111111111111111111111111111111111111111111111111111111111");
        let (source, server) = serve_release("v0.1.0-beta.3", tampered.as_bytes(), Some(SIGNATURE_FIXTURE), 3);
        assert!(find(&source, Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap_err().contains("verification failed"));
        server.join().unwrap();

        // The release key never signed the fixture: the app's own keys refuse it.
        let (source, server) = serve_release("v0.1.0-beta.3", MANIFEST_FIXTURE, Some(SIGNATURE_FIXTURE), 3);
        assert!(find(&source, Channel::Beta, "0.1.0-beta.2", &keys()).is_err());
        server.join().unwrap();

        // A signed manifest of another release, served under this tag.
        let (source, server) = serve_release("v0.1.0-beta.4", MANIFEST_FIXTURE, Some(SIGNATURE_FIXTURE), 3);
        assert!(find(&source, Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap_err().contains("but the release is 0.1.0-beta.4"));
        server.join().unwrap();
    }

    #[test]
    fn a_refused_or_broken_list_is_an_error_to_retry() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let limited = br#"{"message":"API rate limit exceeded for 192.0.2.1."}"#.to_vec();
        let server = serve_on(listener, vec![(format!("/repos/{REPO}/releases?per_page={PER_PAGE}"), 403, limited)], 2);
        let err = find(&Source::at(&base), Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap_err();
        assert!(err.contains("HTTP 403") && err.contains("rate limit"), "{err}");
        assert!(find(&Source::at(&format!("{base}/elsewhere")), Channel::Beta, "0.1.0-beta.2", &test_keys()).unwrap_err().contains("not found"));
        server.join().unwrap();
    }

    #[test]
    fn only_release_builds_or_a_test_server_look() {
        assert_eq!(Source::at("http://127.0.0.1:9/").downloads, "http://127.0.0.1:9/NakiriYuuzu/Yuzora/releases/download");
        assert_eq!(Source::github().downloads, "https://github.com/NakiriYuuzu/Yuzora/releases/download");
        if option_env!("HERDR_APP_RELEASE").is_none() && std::env::var_os(URL_ENV).is_none() {
            assert_eq!(checks_app(Some(PathBuf::from("/Applications/Yuzora.app"))), None);
        }
        assert_eq!(checks_app(None), None);
    }

    #[test]
    fn notes_in_each_language_must_match_their_sha256() {
        use yuzora_core::i18n::{EN, ZH_TW};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (en, zh) = (b"## [0.1.0-beta.3] - 2026-11-03\n- English\n".to_vec(), "## [0.1.0-beta.3] - 2026-11-03\n- 中文\n".as_bytes().to_vec());
        let routes = vec![("/changelog.md".to_owned(), 200, en.clone()), ("/changelog.zh-TW.md".to_owned(), 200, zh.clone())];
        let server = serve_on(listener, routes, 3);
        let mut found: Manifest = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        found.notes = Some(manifest::Asset { url: format!("{base}/changelog.md"), sha256: crate::update::sha256(&en), size: None });
        let zh_tw = |sha256: String| [(ZH_TW.to_owned(), manifest::Asset { url: format!("{base}/changelog.zh-TW.md"), sha256, size: None })].into();
        found.localized_notes = zh_tw(crate::update::sha256(&zh));
        assert_eq!(fetch_asset(found.notes_in(EN).unwrap()).unwrap(), en);
        assert_eq!(fetch_asset(found.notes_in(ZH_TW).unwrap()).unwrap(), zh);
        // Served bytes that aren't what the signed manifest says are refused, not shown.
        found.localized_notes = zh_tw(crate::update::sha256(&en));
        let err = fetch_asset(found.notes_in(ZH_TW).unwrap()).unwrap_err();
        assert!(err.contains("changelog.zh-TW.md: checksum mismatch"), "{err}");
        server.join().unwrap();
    }

    #[test]
    fn each_language_fetches_its_own_notes_unless_kept() {
        use yuzora_core::i18n::{EN, ZH_TW};
        let asset = |name: &str| manifest::Asset { url: format!("https://example.test/{name}"), sha256: "0".repeat(64), size: None };
        let mut found: Manifest = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        found.notes = Some(asset("changelog.md"));
        found.localized_notes = [(ZH_TW.to_owned(), asset("changelog.zh-TW.md"))].into();
        let urls = |has: &dyn Fn(&str, &str) -> bool| notes_to_fetch(&found, has).into_iter().map(|(l, a)| (l, a.url.rsplit('/').next().unwrap().to_owned())).collect::<Vec<_>>();
        assert_eq!(urls(&|_, _| false), [(EN, "changelog.md".to_owned()), (ZH_TW, "changelog.zh-TW.md".to_owned())]);
        let version = found.version.clone();
        assert_eq!(urls(&|l, v| l == EN && v == version), [(ZH_TW, "changelog.zh-TW.md".to_owned())]);
        assert!(urls(&|_, _| true).is_empty());
    }

    #[test]
    fn a_beta_staged_before_picking_stable_is_not_kept() {
        assert!(keeps_on(Channel::Beta, "0.2.0-beta.1"));
        assert!(keeps_on(Channel::Beta, "0.2.0"));
        assert!(keeps_on(Channel::Stable, "0.2.0"));
        assert!(!keeps_on(Channel::Stable, "0.2.0-beta.1"));
    }

    #[test]
    fn a_beta_build_drops_a_staged_beta_but_keeps_stable() {
        let beta = serde_json::json!({"channel": "beta"});
        for settings in [serde_json::Value::Null, beta.clone()] {
            assert!(!keeps_for(&settings, "0.1.0-beta.2", "0.1.0-beta.3"), "{settings}");
            assert!(keeps_for(&settings, "0.1.0-beta.2", "0.1.0"), "{settings}");
        }
        // A stable build set to beta keeps a staged beta, as before.
        assert!(keeps_for(&beta, "0.1.0", "0.2.0-beta.1"));
    }
}
