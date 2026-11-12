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
    keeps_given(&yuzora_core::settings::section(channel::SECTION), env!("CARGO_PKG_VERSION"), version)
}

/// [`keeps`] with the `update` section and the running version passed in.
fn keeps_given(settings: &serde_json::Value, running: &str, version: &str) -> bool {
    Channel::chosen_from(settings, running).takes(version, false)
}

/// Why the last swap failed.
struct SwapFailure {
    busy: bool,
    error: String,
    /// The update that was waiting when it failed; none yet for a swap that failed as the app started.
    of: Option<String>,
}

impl SwapFailure {
    fn text(&self) -> String {
        swap_failure_text(self.busy, &self.error)
    }
}

static SWAP_FAILURE: std::sync::Mutex<Option<SwapFailure>> = std::sync::Mutex::new(None);

fn failure_slot() -> std::sync::MutexGuard<'static, Option<SwapFailure>> {
    SWAP_FAILURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `swap` (src/update.rs) could not move the folders: keeps why, for "Restart" and Settings › Updates.
pub fn note_swap_failure(e: &std::io::Error) {
    note_swap_failure_of(crate::update::ready(), e);
}

fn note_swap_failure_of(of: Option<String>, e: &std::io::Error) {
    *failure_slot() = Some(SwapFailure {
        busy: cfg!(windows) && busy_code(e.raw_os_error()),
        error: e.to_string(),
        of,
    });
}

/// The failure of the last swap, in words, for as long as the update it was for waits (`waiting`: the version waiting
/// now). A later swap that works ends the process; a staged update that is dropped leaves nothing to explain; and a
/// newer update staged since has not been tried, so what stopped the one before it is not said of it.
fn current(waiting: Option<&str>) -> Option<String> {
    let waiting = waiting?;
    let mut slot = failure_slot();
    let failure = slot.as_mut()?;
    // A swap that failed as the app started had no version to name: it was for the first one found waiting.
    let of = failure.of.get_or_insert_with(|| waiting.to_owned());
    (of == waiting).then(|| failure.text())
}

/// What the person is told while an update waits and its swap failed: Settings › Updates shows it (`update::status`).
pub fn swap_failure() -> Option<String> {
    current(crate::update::ready().as_deref())
}

/// What `app.update` says when `apply` returned false: the swap's failure if it had one, else that nothing is waiting.
pub fn apply_failure() -> String {
    swap_failure().unwrap_or_else(|| "no update is ready yet".into())
}

/// The overlay id of the toast that says an update did not install: one at a time.
pub const FAILED_TOAST: &str = "yuzora-update-failed";

/// How long that toast stays: long enough to read and click (an error's usual 8 s is not).
const FAILED_TOAST_FOR_MS: u64 = 30_000;

/// The notification for a swap that failed. The toast keeps one line each of title and body (mods/toast, 340 px), which is
/// far less than the reason and what to do, so it says that and where the whole text is: a click opens Updates.
pub(crate) fn toast_of(busy: bool) -> serde_json::Value {
    let body = if busy { "Install folder in use. Click for steps." } else { "Click for the reason and what to do." };
    serde_json::json!({"kind": "toast", "notification": {
        "title": "Yuzora could not install the update",
        "body": body,
        "level": "error",
        "timeout": FAILED_TOAST_FOR_MS,
        "action": {"name": "app.settings", "args": {"section": "updates"}},
    }})
}

/// What `command_error` (src/window.rs) opens instead of its own toast when `name` failed with `error` and that is the swap's failure.
pub fn swap_failure_toast(name: &str, error: &str) -> Option<serde_json::Value> {
    let slot = failure_slot();
    let failure = slot.as_ref().filter(|f| name == "app.update" && f.text() == error)?;
    Some(toast_of(failure.busy))
}

/// Windows says "in use" with sharing violation (32), access denied (5) or lock violation (33).
fn busy_code(code: Option<i32>) -> bool {
    matches!(code, Some(5 | 32 | 33))
}

pub(crate) fn swap_failure_text(busy: bool, error: &str) -> String {
    let installer = format!("download the installer from https://github.com/{REPO}/releases");
    if busy {
        format!(
            "Yuzora could not install the update because another program is using its install folder. Close any terminal opened in that folder. If you came from a beta, restart your PC once. Then choose Restart again, or {installer}."
        )
    } else {
        format!(
            "Yuzora could not install the update: {error}. Choose Restart to try again, or {installer}."
        )
    }
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
    // update.
    if let Some(notes) = notes_to_fetch(&found, crate::update_notes::has) {
        match fetch_asset(notes) {
            Ok(text) => crate::update_notes::cache(&String::from_utf8_lossy(&text)),
            Err(e) => tracing::warn!("update notes for {}: {e}", found.version),
        }
    }
    let Some(entry) = found.targets.get(&super::target()) else {
        tracing::info!("Yuzora {} has no build for {}", found.version, super::target());
        return Ok(None);
    };
    crate::update::stage(root, &found.version, &entry.url, &entry.sha256)
}

/// The notes to download for `found`: the English ones, unless `has(version)`
/// says they are already kept. (A manifest may also list notes in other
/// languages; the app is English only and does not fetch them.)
fn notes_to_fetch(found: &Manifest, has: impl Fn(&str) -> bool) -> Option<&manifest::Asset> {
    found.notes.as_ref().filter(|_| !has(&found.version))
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
    fn a_folder_in_use_is_what_windows_calls_32_5_and_33() {
        assert!([32, 5, 33].into_iter().all(|code| busy_code(Some(code))));
        assert!(!busy_code(Some(183)) && !busy_code(Some(2)) && !busy_code(None));
    }

    #[test]
    fn a_folder_in_use_says_what_to_close_and_where_the_installer_is() {
        let text = swap_failure_text(
            true,
            "The process cannot access the file because it is being used by another process. (os error 32)",
        );
        assert!(
            text.contains("another program is using its install folder"),
            "{text}"
        );
        assert!(text.contains("Restart"), "{text}");
        assert!(text.contains("restart your PC"), "{text}");
        assert!(
            text.contains("https://github.com/NakiriYuuzu/Yuzora/releases"),
            "{text}"
        );
    }

    #[test]
    fn any_other_swap_failure_carries_the_system_error() {
        let text = swap_failure_text(
            false,
            "Cannot create a file when that file already exists. (os error 183)",
        );
        assert!(text.contains("os error 183"), "{text}");
        assert!(
            text.contains("https://github.com/NakiriYuuzu/Yuzora/releases"),
            "{text}"
        );
        assert!(!text.contains("install folder"), "{text}");
    }

    #[test]
    fn the_upstream_files_call_what_this_module_provides() {
        let main = include_str!("../../main.rs");
        let leave = main
            .find("yuzora::release::cwd::leave_install_dir()")
            .expect("main() leaves the install folder");
        let swap = main
            .find("update::apply_on_startup()")
            .expect("main() applies a staged update");
        assert!(
            leave < swap,
            "the working directory moves before the startup swap"
        );
        let update = include_str!("../../update.rs");
        assert_eq!(
            update.matches("update::note_swap_failure(&e)").count(),
            2,
            "both ways a swap fails"
        );
        assert!(
            update.contains("\"failure\": crate::yuzora::release::update::swap_failure()"),
            "Settings › Updates is told why (status changes redraw it)"
        );
        let window = include_str!("../../window.rs");
        assert!(
            window.contains("self.fail(crate::yuzora::release::update::apply_failure())"),
            "Restart says why"
        );
        assert!(
            window.contains("crate::yuzora::release::update::swap_failure_toast(name, error)"),
            "the toast for it points at Settings › Updates"
        );
    }

    #[test]
    fn a_swap_failure_is_kept_for_as_long_as_the_update_waits_and_has_a_toast() {
        assert_eq!(current(Some("0.1.1")), None, "nothing failed yet");
        note_swap_failure_of(Some("0.1.1".into()), &std::io::Error::other("no"));
        let said = current(Some("0.1.1")).expect("said while the update waits");
        assert!(said.contains("could not install the update"), "{said}");
        assert_eq!(
            current(Some("0.1.1")).as_deref(),
            Some(said.as_str()),
            "Restart again, or Settings, say it again"
        );
        assert_eq!(
            current(None),
            None,
            "an update that is no longer waiting has nothing to explain"
        );
        assert_eq!(
            current(Some("0.1.2")),
            None,
            "a newer update staged since has not been tried: the older one's failure is not its"
        );
        assert_eq!(
            current(Some("0.1.1")).as_deref(),
            Some(said.as_str()),
            "asking about the newer one did not forget it"
        );
        note_swap_failure_of(None, &std::io::Error::other("no"));
        assert_eq!(
            current(Some("0.1.1")).as_deref(),
            Some(said.as_str()),
            "a swap that failed as the app started is for the update found waiting first"
        );
        assert_eq!(current(Some("0.1.2")), None, "and not for one staged after it");
        assert_eq!(
            apply_failure(),
            "no update is ready yet",
            "nothing is waiting in a test: the default"
        );
        let toast = swap_failure_toast("app.update", &said)
            .expect("Restart's failure gets the pointing toast");
        assert_eq!(
            toast["notification"]["action"]["args"]["section"],
            "updates"
        );
        assert_eq!(
            swap_failure_toast("app.update", "no update is ready yet"),
            None
        );
        assert_eq!(
            swap_failure_toast("space.close", &said),
            None,
            "another command's error is shown as it is"
        );
    }

    /// About how wide a line is in the toast (mods/toast truncates each of title and body to one line, 340 px).
    fn width(text: &str) -> usize {
        text.chars()
            .map(|c| if c as u32 >= 0x2E80 { 2 } else { 1 })
            .sum()
    }

    #[test]
    fn the_toast_for_a_swap_failure_fits_one_line_and_opens_updates() {
        for busy in [true, false] {
            let toast = toast_of(busy);
            assert_eq!(toast["kind"], "toast");
            let note = &toast["notification"];
            assert_eq!(note["level"], "error");
            assert_eq!(
                note["action"],
                serde_json::json!({"name": "app.settings", "args": {"section": "updates"}}),
                "a click opens Updates, which holds the whole reason"
            );
            assert!(
                note["timeout"].as_u64().unwrap() >= 15_000,
                "stays long enough to read and click"
            );
            for (part, max) in [("title", 42), ("body", 44)] {
                let text = note[part].as_str().unwrap_or_default();
                assert!(
                    !text.is_empty() && width(text) <= max,
                    "{part} must fit one line (busy {busy}): {text:?}"
                );
            }
        }
        assert!(
            toast_of(true)["notification"]["body"]
                .as_str()
                .unwrap()
                .contains("folder"),
            "the folder in use is named"
        );
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
    fn notes_must_match_their_sha256() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (en, other) = (b"## [0.1.0-beta.3] - 2026-11-03\n- English\n".to_vec(), b"## [0.1.0-beta.3] - 2026-11-03\n- Other\n".to_vec());
        let routes = vec![("/changelog.md".to_owned(), 200, other)];
        let server = serve_on(listener, routes, 2);
        let mut found: Manifest = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        let notes = |sha256: String| Some(manifest::Asset { url: format!("{base}/changelog.md"), sha256, size: None });
        // Served bytes that aren't what the signed manifest says are refused, not shown.
        found.notes = notes(crate::update::sha256(&en));
        let err = fetch_asset(found.notes.as_ref().unwrap()).unwrap_err();
        assert!(err.contains("changelog.md: checksum mismatch"), "{err}");
        found.notes = notes(crate::update::sha256(b"## [0.1.0-beta.3] - 2026-11-03\n- Other\n"));
        assert!(fetch_asset(found.notes.as_ref().unwrap()).is_ok());
        server.join().unwrap();
    }

    #[test]
    fn the_english_notes_are_fetched_unless_kept_and_other_languages_are_not() {
        let asset = |name: &str| manifest::Asset { url: format!("https://example.test/{name}"), sha256: "0".repeat(64), size: None };
        let mut found: Manifest = serde_json::from_slice(MANIFEST_FIXTURE).unwrap();
        found.notes = Some(asset("changelog.md"));
        found.localized_notes = [("zh-TW".to_owned(), asset("changelog.zh-TW.md"))].into();
        let version = found.version.clone();
        assert!(notes_to_fetch(&found, |_| false).unwrap().url.ends_with("/changelog.md"));
        assert!(notes_to_fetch(&found, |v| v == version).is_none());
        found.notes = None;
        assert!(notes_to_fetch(&found, |_| false).is_none());
    }

    #[test]
    fn keeps_follows_the_running_builds_channel() {
        let beta = serde_json::json!({"channel": "beta"});
        let stable = serde_json::json!({"channel": "stable"});
        for running in ["0.1.0-beta.2", "0.1.0"] {
            for settings in [serde_json::Value::Null, stable.clone()] {
                assert!(keeps_given(&settings, running, "0.2.0"), "{running} {settings}");
                assert!(!keeps_given(&settings, running, "0.2.0-beta.1"), "{running} {settings}");
            }
            assert!(keeps_given(&beta, running, "0.2.0"), "{running}");
        }
        // A beta build takes only stable releases even when set to beta: a
        // staged beta goes. A stable build set to beta keeps it.
        assert!(!keeps_given(&beta, "0.1.0-beta.2", "0.2.0-beta.1"));
        assert!(keeps_given(&beta, "0.1.0", "0.2.0-beta.1"));
    }
}
