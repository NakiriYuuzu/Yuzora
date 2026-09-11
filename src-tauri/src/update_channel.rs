//! Preview discovery only selects immutable, signed release metadata. Downloads
//! and installation stay inside the Tauri updater (including signature checks).
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::{Manager, ResourceId, Webview};
use tauri_plugin_updater::UpdaterExt;

const RELEASES_API: &str = "https://api.github.com/repos/NakiriYuuzu/Yuzora/releases";
const RELEASE_DOWNLOADS: &str = "https://github.com/NakiriYuuzu/Yuzora/releases/download";

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    assets: Vec<ReleaseAsset>,
}

fn newest_signed_release<'a>(
    releases: &'a [Release],
    current: &Version,
) -> Option<(&'a str, Version)> {
    releases
        .iter()
        .filter_map(|release| {
            if release.draft
                || !release
                    .assets
                    .iter()
                    .any(|asset| asset.name == "latest.json")
            {
                return None;
            }
            let version = Version::parse(
                release
                    .tag_name
                    .strip_prefix('v')
                    .unwrap_or(&release.tag_name),
            )
            .ok()?;
            version
                .cmp_precedence(current)
                .is_gt()
                .then_some((release.tag_name.as_str(), version))
        })
        .max_by(|a, b| a.1.cmp_precedence(&b.1))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: ResourceId,
    current_version: String,
    version: String,
    body: Option<String>,
    raw_json: serde_json::Value,
}

#[tauri::command]
pub async fn check_preview_update(webview: Webview) -> Result<Option<UpdateMetadata>, String> {
    let updater_enabled = webview
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("endpoints"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|endpoints| !endpoints.is_empty());
    if !updater_enabled {
        return Err("Automatic updates are disabled in this build".into());
    }
    let current = &webview.package_info().version;
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .user_agent("Yuzora-Updater")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let mut releases = Vec::new();
    // Do not assume GitHub's publication-date order is SemVer order. Paginate
    // before selecting, and fail explicitly if the bounded inventory is incomplete.
    for page in 1..=10 {
        let response = client
            .get(format!("{RELEASES_API}?per_page=100&page={page}"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?;
        let batch: Vec<Release> = response.json().await.map_err(|error| error.to_string())?;
        let finished = batch.len() < 100;
        releases.extend(batch);
        if finished {
            break;
        }
        if page == 10 {
            return Err("Update release inventory exceeds the supported page limit".into());
        }
    }
    let Some((tag, expected_version)) = newest_signed_release(&releases, current) else {
        return Ok(None);
    };
    let endpoint = format!("{RELEASE_DOWNLOADS}/{tag}/latest.json")
        .parse()
        .map_err(|error| format!("Invalid update endpoint: {error}"))?;
    let update = webview
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?
        .check()
        .await
        .map_err(|error| error.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    if update.version != expected_version.to_string() {
        return Err("Update metadata version does not match its release tag".into());
    }
    Ok(Some(UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        rid: webview.resources_table().add(update),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(tag: &str, draft: bool, signed_metadata: bool) -> Release {
        Release {
            tag_name: tag.into(),
            draft,
            assets: if signed_metadata {
                vec![ReleaseAsset {
                    name: "latest.json".into(),
                }]
            } else {
                vec![]
            },
        }
    }
    #[test]
    fn preview_selects_semver_order_including_stable_promotions() {
        let current = Version::parse("0.0.9-beta.3").unwrap();
        let releases = vec![
            release("v0.0.9-beta.4", false, true),
            release("v0.0.9", false, true),
        ];
        assert_eq!(
            newest_signed_release(&releases, &current).unwrap().0,
            "v0.0.9"
        );
        let releases = vec![
            release("v0.0.9", false, true),
            release("v0.0.10-beta.1", false, true),
        ];
        assert_eq!(
            newest_signed_release(&releases, &current).unwrap().0,
            "v0.0.10-beta.1"
        );
    }
    #[test]
    fn ignores_unsigned_old_betas_drafts_invalid_versions_and_downgrades() {
        let releases = vec![
            release("v0.0.9-beta.3", false, false),
            release("v1.0.0", true, true),
            release("not-semver", false, true),
            release("v0.0.8", false, true),
        ];
        assert!(newest_signed_release(&releases, &Version::parse("0.0.9").unwrap()).is_none());
    }
    #[test]
    fn beta_ten_sorts_after_beta_nine() {
        let releases = vec![
            release("v0.0.10-beta.9", false, true),
            release("v0.0.10-beta.10", false, true),
        ];
        assert_eq!(
            newest_signed_release(&releases, &Version::parse("0.0.10-beta.8").unwrap())
                .unwrap()
                .0,
            "v0.0.10-beta.10"
        );
    }
}
