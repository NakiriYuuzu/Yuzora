//! Browser-only, read-only workspace resources. No filesystem paths are accepted from pages.
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, sync::Mutex};
use tauri::{AppHandle, Manager, Url};
use yuzora_host::protocol::{ConnectionOwner, Operation, MAX_FILE_BYTES};

pub const SCHEME: &str = "yuzora-preview";

#[derive(Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResourceSource {
    Local {
        workspace: String,
    },
    Sftp {
        session_id: String,
        root: String,
    },
    Runtime {
        owner: ConnectionOwner,
        workspace: String,
    },
}

#[derive(Default)]
pub struct PreviewResourceState(Mutex<ResourceRegistry>);

#[derive(Default)]
struct ResourceRegistry {
    sources: HashMap<String, ResourceSource>,
    active: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLease {
    pub id: String,
    pub url: String,
}

pub fn is_resource_url(url: &Url) -> bool {
    url.scheme() == SCHEME
        || (url.scheme() == "http"
            && url.host_str().is_some_and(|host| {
                host.strip_prefix("yuzora-preview.").is_some_and(|id| {
                    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            }))
}

fn resource_id(url: &Url) -> Option<&str> {
    let host = url.host_str()?;
    if url.scheme() == SCHEME {
        Some(host)
    } else {
        host.strip_prefix("yuzora-preview.")
    }
}

pub fn native_url(url: Url, windows: bool) -> Result<Url, String> {
    if windows && url.scheme() == SCHEME {
        // Wry rewrites initial URLs only. Explicit navigations of an existing
        // WebView2 must use its custom-protocol HTTP origin as well.
        Url::parse(
            &url.as_str()
                .replacen("yuzora-preview://", "http://yuzora-preview.", 1),
        )
        .map_err(|error| error.to_string())
    } else {
        Ok(url)
    }
}

/// Bind opaque resource access to the current Browser session. A close or owner
/// replacement revokes it; subframe navigation must not change the binding.
pub fn activate_url(app: &AppHandle, url: Option<&Url>) -> bool {
    let state = app.state::<PreviewResourceState>();
    let Ok(mut registry) = state.0.lock() else {
        return false;
    };
    if url.is_some_and(|url| !is_resource_url(url)) {
        return true;
    }
    let id = url.filter(|url| is_resource_url(url)).and_then(resource_id);
    if id.is_some_and(|id| !registry.sources.contains_key(id)) {
        return false;
    }
    registry.active = id.map(str::to_owned);
    true
}

pub fn allows_navigation(app: &AppHandle, url: &Url) -> bool {
    if !is_resource_url(url) {
        return true;
    }
    let state = app.state::<PreviewResourceState>();
    state
        .0
        .lock()
        .is_ok_and(|registry| registry.active.as_deref() == resource_id(url))
}

fn resource_path(url: &Url) -> Result<String, String> {
    let path = percent_encoding::percent_decode_str(url.path().trim_start_matches('/'))
        .decode_utf8()
        .map_err(|_| "preview-path-not-utf8")?
        .into_owned();
    crate::path_capability::SafeRelativePath::parse(&path).map_err(String::from)?;
    Ok(path)
}

async fn read_source(
    app: &AppHandle,
    source: ResourceSource,
    path: String,
) -> Result<Vec<u8>, String> {
    match source {
        ResourceSource::Local { workspace } => {
            let opened = app
                .state::<crate::path_capability::WorkspacePathState>()
                .0
                .open_file(&workspace, &path)
                .map_err(String::from)?;
            if opened.len > MAX_FILE_BYTES {
                return Err("file-too-large".into());
            }
            tauri::async_runtime::spawn_blocking(move || {
                let mut bytes = Vec::new();
                opened
                    .file
                    .take(MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > MAX_FILE_BYTES {
                    return Err("file-too-large".into());
                }
                Ok(bytes)
            })
            .await
            .map_err(|e| e.to_string())?
        }
        ResourceSource::Sftp { session_id, root } => {
            let ssh = app.state::<crate::ssh_service::SshState>().0.clone();
            let sftp = ssh.ensure_sftp(&session_id).await?;
            let canonical_root = sftp.canonicalize(&root).await.map_err(|e| e.to_string())?;
            if canonical_root != root {
                return Err("preview-workspace-changed".into());
            }
            let path = format!("{}/{}", root.trim_end_matches('/'), path);
            let canonical = sftp.canonicalize(&path).await.map_err(|e| e.to_string())?;
            if !canonical.starts_with(&format!("{}/", root.trim_end_matches('/'))) {
                return Err("preview-resource-outside-workspace".into());
            }
            crate::sftp_edit::read_regular(&sftp, &path).await
        }
        ResourceSource::Runtime { owner, workspace } => {
            let result = app
                .state::<crate::host_service::HostState>()
                .0
                .request(
                    owner,
                    Operation::FilesReadBase64 {
                        workspace,
                        path,
                        max_bytes: MAX_FILE_BYTES,
                    },
                )
                .await?;
            base64::engine::general_purpose::STANDARD
                .decode(result["data"].as_str().ok_or("invalid-preview-resource")?)
                .map_err(|e| e.to_string())
        }
    }
}

#[tauri::command]
pub async fn preview_resource_open(
    app: AppHandle,
    source: ResourceSource,
    path: String,
) -> Result<ResourceLease, String> {
    crate::path_capability::SafeRelativePath::parse(&path).map_err(String::from)?;
    read_source(&app, source.clone(), path.clone()).await?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut url = Url::parse(&format!("{SCHEME}://{id}/")).map_err(|e| e.to_string())?;
    url.set_path(
        &path
            .split('/')
            .map(|segment| {
                percent_encoding::utf8_percent_encode(segment, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("/"),
    );
    let state = app.state::<PreviewResourceState>();
    let mut registry = state.0.lock().map_err(|e| e.to_string())?;
    if registry.sources.len() >= 8 {
        return Err("preview-resource-limit".into());
    }
    registry.sources.insert(id.clone(), source);
    Ok(ResourceLease {
        id,
        url: url.into(),
    })
}

#[tauri::command]
pub fn preview_resource_close(id: String, state: tauri::State<'_, PreviewResourceState>) {
    if let Ok(mut registry) = state.0.lock() {
        registry.sources.remove(&id);
        if registry.active.as_deref() == Some(&id) {
            registry.active = None;
        }
    }
}

fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub async fn respond(
    app: AppHandle,
    label: String,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    let result = async {
        if label != "preview-child" || !matches!(request.method().as_str(), "GET" | "HEAD") {
            return Err("preview-resource-forbidden".to_string());
        }
        let url = Url::parse(&request.uri().to_string()).map_err(|e| e.to_string())?;
        let id = resource_id(&url).ok_or("preview-resource-missing")?;
        let state = app.state::<PreviewResourceState>();
        let source = {
            let registry = state.0.lock().map_err(|e| e.to_string())?;
            if registry.active.as_deref() != Some(id) {
                return Err("preview-resource-expired".into());
            }
            registry
                .sources
                .get(id)
                .cloned()
                .ok_or("preview-resource-expired")?
        };
        let path = resource_path(&url)?;
        let bytes = read_source(&app, source, path.clone()).await?;
        if state.0.lock().map_err(|e| e.to_string())?.active.as_deref() != Some(id) {
            return Err("preview-resource-expired".into());
        }
        Ok((path, bytes))
    }
    .await;
    match result {
        Ok((path, bytes)) => tauri::http::Response::builder()
            .header("Content-Type", content_type(&path))
            .header("Cache-Control", "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer")
            .header("Access-Control-Allow-Origin", "*")
            .body(if request.method() == "HEAD" {
                Vec::new()
            } else {
                bytes
            })
            .unwrap(),
        Err(error) => tauri::http::Response::builder()
            .status(404)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(error.into_bytes())
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn webview2_navigation_keeps_resource_paths_and_query() {
        let source = Url::parse(
            "yuzora-preview://0123456789abcdef0123456789abcdef/%E4%B8%AD%20a.html?x=1#card",
        )
        .unwrap();
        let target = native_url(source.clone(), true).unwrap();
        assert!(is_resource_url(&target));
        assert_eq!(resource_id(&source), resource_id(&target));
        assert_eq!(resource_path(&target).unwrap(), "中 a.html");
        assert_eq!(target.query(), Some("x=1"));
        assert_eq!(target.fragment(), Some("card"));
        assert_eq!(native_url(source.clone(), false).unwrap(), source);
    }
    #[test]
    fn resource_paths_decode_unicode_and_reject_escaped_separators() {
        assert_eq!(
            resource_path(
                &Url::parse("yuzora-preview://id/%E4%B8%AD%E6%96%87%20file.html").unwrap()
            )
            .unwrap(),
            "中文 file.html"
        );
        assert!(
            resource_path(&Url::parse("yuzora-preview://id/%2e%2e%2fsecret").unwrap()).is_err()
        );
        assert!(resource_path(&Url::parse("yuzora-preview://id/%00").unwrap()).is_err());
    }
}
