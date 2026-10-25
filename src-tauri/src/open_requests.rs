//! Files and folders the OS asks Yuzora to open (Finder "Open With", the Dock,
//! Explorer verbs). Windows passes them as argv, also forwarded from a second
//! instance; macOS sends `RunEvent::Opened`. A launch request can arrive before
//! the webview loads, so requests wait here until the frontend takes them; the
//! event only says "take now" and carries no paths.

use std::collections::VecDeque;
#[cfg(any(not(target_os = "macos"), test))]
use std::ffi::OsString;
#[cfg(any(not(target_os = "macos"), test))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub const AVAILABLE_EVENT: &str = "open-requests:available";
const MAX_PENDING: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenRequestKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    pub path: String,
    pub kind: OpenRequestKind,
}

#[derive(Default)]
pub struct OpenRequestState(Mutex<VecDeque<OpenRequest>>);

impl OpenRequestState {
    fn push(&self, requests: Vec<OpenRequest>) {
        let mut queue = self.0.lock().unwrap();
        for request in requests {
            if queue.iter().any(|pending| pending.path == request.path) {
                continue;
            }
            if queue.len() == MAX_PENDING {
                queue.pop_front();
            }
            queue.push_back(request);
        }
    }

    fn take(&self) -> Vec<OpenRequest> {
        self.0.lock().unwrap().drain(..).collect()
    }
}

/// Paths stay as the OS gave them (made absolute, not canonicalized): folders
/// are canonicalized later by the workspace open path, and files keep the
/// same form a Finder/Explorer drop delivers. Re-collecting the components
/// only drops the trailing separator of Finder's folder URLs.
fn classify(path: PathBuf) -> Option<OpenRequest> {
    let path: PathBuf = path.components().collect();
    let metadata = std::fs::metadata(&path).ok()?;
    let kind = if metadata.is_dir() {
        OpenRequestKind::Directory
    } else if metadata.is_file() {
        OpenRequestKind::File
    } else {
        return None;
    };
    Some(OpenRequest {
        path: path.into_os_string().into_string().ok()?,
        kind,
    })
}

fn unique(requests: impl Iterator<Item = OpenRequest>) -> Vec<OpenRequest> {
    let mut unique: Vec<OpenRequest> = Vec::new();
    for request in requests {
        if !unique.iter().any(|seen| seen.path == request.path) {
            unique.push(request);
        }
    }
    unique
}

/// argv[0] is the executable; flags are never paths an Explorer verb sends.
#[cfg(any(not(target_os = "macos"), test))]
fn requests_from_args<I>(args: I, cwd: &Path) -> Vec<OpenRequest>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    unique(
        args.into_iter()
            .skip(1)
            .map(Into::into)
            .filter(|arg| !arg.to_string_lossy().starts_with('-'))
            .filter_map(|arg| std::path::absolute(cwd.join(arg)).ok())
            .filter_map(classify),
    )
}

#[cfg(any(target_os = "macos", test))]
fn requests_from_urls(urls: &[tauri::Url]) -> Vec<OpenRequest> {
    unique(
        urls.iter()
            .filter(|url| url.scheme() == "file")
            .filter_map(|url| url.to_file_path().ok())
            .filter_map(classify),
    )
}

fn enqueue<R: Runtime>(app: &AppHandle<R>, requests: Vec<OpenRequest>) {
    if requests.is_empty() {
        return;
    }
    app.state::<OpenRequestState>().push(requests);
    let _ = app.emit_to("main", AVAILABLE_EVENT, ());
    // The startup window stays hidden until the frontend's first themed frame
    // (AppShell); only bring an already visible window forward.
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            if window.is_minimized().unwrap_or(false) {
                let _ = window.unminimize();
            }
            let _ = window.set_focus();
        }
    }
}

/// Windows argv; macOS hands documents over as `RunEvent::Opened` instead.
#[cfg(not(target_os = "macos"))]
pub fn enqueue_args<R, I>(app: &AppHandle<R>, args: I, cwd: &Path)
where
    R: Runtime,
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    enqueue(app, requests_from_args(args, cwd));
}

#[cfg(target_os = "macos")]
pub fn enqueue_urls<R: Runtime>(app: &AppHandle<R>, urls: &[tauri::Url]) {
    enqueue(app, requests_from_urls(urls));
}

#[tauri::command]
pub fn open_requests_take(state: tauri::State<'_, OpenRequestState>) -> Vec<OpenRequest> {
    state.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(requests: &[OpenRequest]) -> Vec<(&str, OpenRequestKind)> {
        requests
            .iter()
            .map(|request| (request.path.as_str(), request.kind))
            .collect()
    }

    #[test]
    fn args_skip_the_executable_and_flags_and_resolve_relative_paths() {
        let cwd = tempfile::tempdir().unwrap();
        let root = cwd.path().canonicalize().unwrap();
        std::fs::write(root.join("notes.md"), "x").unwrap();
        std::fs::create_dir(root.join("project")).unwrap();
        let file = root.join("notes.md").to_str().unwrap().to_string();
        let folder = root.join("project").to_str().unwrap().to_string();

        let requests = requests_from_args(
            [
                "yuzora.exe",
                "--flag",
                "notes.md",
                folder.as_str(),
                "missing.txt",
            ],
            &root,
        );

        assert_eq!(
            paths(&requests),
            vec![
                (file.as_str(), OpenRequestKind::File),
                (folder.as_str(), OpenRequestKind::Directory),
            ]
        );
    }

    #[test]
    fn args_keep_the_first_of_duplicate_paths() {
        let cwd = tempfile::tempdir().unwrap();
        let root = cwd.path().canonicalize().unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let absolute = root.join("a.txt").to_str().unwrap().to_string();

        let requests = requests_from_args(["yuzora", "a.txt", absolute.as_str()], &root);

        assert_eq!(
            paths(&requests),
            vec![(absolute.as_str(), OpenRequestKind::File)]
        );
    }

    #[test]
    fn urls_accept_only_existing_file_urls() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let folder = root.join("with space 中文");
        std::fs::create_dir(&folder).unwrap();
        let urls = [
            tauri::Url::from_directory_path(&folder).unwrap(),
            tauri::Url::parse("https://example.com/a.txt").unwrap(),
            tauri::Url::from_file_path(root.join("missing.txt")).unwrap(),
        ];

        let requests = requests_from_urls(&urls);

        // Finder folder URLs end in "/"; the queued path must not.
        assert_eq!(
            paths(&requests),
            vec![(folder.to_str().unwrap(), OpenRequestKind::Directory)]
        );
    }

    #[test]
    fn queue_dedupes_pending_paths_caps_its_size_and_drains_on_take() {
        let state = OpenRequestState::default();
        let request = |index: usize| OpenRequest {
            path: format!("/tmp/{index}"),
            kind: OpenRequestKind::File,
        };
        state.push(vec![request(0), request(0)]);
        state.push((1..=MAX_PENDING).map(request).collect());

        let taken = state.take();

        assert_eq!(taken.len(), MAX_PENDING);
        assert_eq!(taken.first(), Some(&request(1)));
        assert_eq!(taken.last(), Some(&request(MAX_PENDING)));
        assert!(state.take().is_empty());
    }
}
