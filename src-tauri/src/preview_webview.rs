// P3 (a): an external-URL browser rendered as a child webview overlaid on the
// preview panel region (Tauri multiwebview, `unstable` feature). An `<iframe>`
// cannot host arbitrary https — remote `X-Frame-Options` / CSP `frame-ancestors`
// block embedding — so a real webview is layered over a placeholder <div> whose
// bounds the front-end tracks via ResizeObserver.
//
// The child webview is a native layer that paints above every DOM overlay, so the
// front-end hides it (`preview_set_visible(false)`) whenever any modal/popover is
// open (see the overlay gate) and closes it when the preview panel unmounts.
//
// Dispatch commands off the main thread: creating a WebView2 child from a
// synchronous command deadlocks on Windows. Tauri dispatches the native work to
// the main thread itself. The frontend's nativePreviewQueue preserves operation
// order; commands sharing the lifecycle mutex must not wait for it on the main
// thread while creation/close waits for native work to finish there.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewBuilder,
    WebviewEvent, WebviewUrl,
};

const PREVIEW_LABEL: &str = "preview-child";
const PREVIEW_FILE_DROP_EVENT: &str = "preview:file-drop";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewFileDropPayload {
    paths: Vec<String>,
}

pub fn forward_preview_file_drop(webview: &Webview, event: &WebviewEvent) {
    if webview.label() != PREVIEW_LABEL {
        return;
    }
    let WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event else {
        return;
    };
    let paths = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let _ = webview.emit(PREVIEW_FILE_DROP_EVENT, PreviewFileDropPayload { paths });
}

pub struct PreviewWebview {
    webview: Webview,
    session_id: Option<String>,
}

pub struct PreviewWebviewState(pub Mutex<Option<PreviewWebview>>);

fn focus_main_webview(app: &AppHandle) {
    if let Some(webview) = app.get_webview("main") {
        let _ = webview.set_focus();
    }
}

fn parse_web_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|e| e.to_string())?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!("unsupported preview scheme: {other}")),
    }
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // IPC keeps the existing bounds fields and adds session ownership.
pub fn preview_open_url(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    session_id: Option<String>,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let parsed = parse_web_url(&url)?;
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;

    // A new frontend owner must not inherit callbacks or history from a closed
    // or superseded session, even if its close command has not run yet.
    if guard
        .as_ref()
        .is_some_and(|preview| preview.session_id != session_id)
    {
        if let Some(previous) = guard.take() {
            previous.webview.close().map_err(|e| e.to_string())?;
        }
    }

    if let Some(preview) = guard.as_ref() {
        let webview = &preview.webview;
        webview.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = webview.set_position(LogicalPosition::new(x, y));
        let _ = webview.set_size(LogicalSize::new(width, height));
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "main window missing".to_string())?;
    let builder = WebviewBuilder::new(PREVIEW_LABEL, WebviewUrl::External(parsed))
        .focused(false)
        .on_navigation(|url| matches!(url.scheme(), "http" | "https"));
    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| e.to_string())?;
    let _ = webview.hide();
    *guard = Some(PreviewWebview {
        webview,
        session_id,
    });
    Ok(())
}

#[tauri::command(async)]
pub fn preview_set_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(preview) = guard.as_ref() {
        let webview = &preview.webview;
        let _ = webview.set_position(LogicalPosition::new(x, y));
        let _ = webview.set_size(LogicalSize::new(width, height));
    }
    Ok(())
}

#[tauri::command(async)]
pub fn preview_set_visible(
    app: AppHandle,
    visible: bool,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(preview) = guard.as_ref() {
        let webview = &preview.webview;
        let _ = if visible {
            webview.show()
        } else {
            webview.hide()
        };
    }
    if !visible {
        focus_main_webview(&app);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn preview_close(
    app: AppHandle,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    let close_result = if let Some(webview) = guard.take() {
        webview.webview.close().map_err(|e| e.to_string())
    } else {
        Ok(())
    };
    drop(guard);
    focus_main_webview(&app);
    close_result
}

fn eval_history(state: &tauri::State<'_, PreviewWebviewState>, js: &str) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(preview) = guard.as_ref() {
        let webview = &preview.webview;
        webview.eval(js).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewNavigationSnapshot {
    session_id: String,
    url: String,
    can_go_back: bool,
    can_go_forward: bool,
}

#[derive(Clone, Copy)]
enum NavigationAction {
    Read,
    Back,
    Forward,
}

// Run only inside with_webview's main-thread callback. Never consult page JS:
// these flags describe the browser's real history, including SPA and redirects.
fn native_snapshot(
    view: tauri::webview::PlatformWebview,
    action: NavigationAction,
) -> Result<(String, bool, bool), String> {
    #[cfg(target_os = "macos")]
    unsafe {
        let view: &objc2_web_kit::WKWebView = &*view.inner().cast();
        match action {
            NavigationAction::Read => {}
            NavigationAction::Back => {
                view.goBack();
            }
            NavigationAction::Forward => {
                view.goForward();
            }
        }
        let url = view
            .URL()
            .and_then(|url| url.absoluteString())
            .ok_or("Preview URL unavailable")?
            .to_string();
        Ok((url, view.canGoBack(), view.canGoForward()))
    }
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::WebViewExt;
        let view = view.inner();
        match action {
            NavigationAction::Read => {}
            NavigationAction::Back => view.go_back(),
            NavigationAction::Forward => view.go_forward(),
        }
        let url = view.uri().ok_or("Preview URL unavailable")?.to_string();
        Ok((url, view.can_go_back(), view.can_go_forward()))
    }
    #[cfg(windows)]
    unsafe {
        let view = view
            .controller()
            .CoreWebView2()
            .map_err(|e| e.to_string())?;
        match action {
            NavigationAction::Read => {}
            NavigationAction::Back => view.GoBack().map_err(|e| e.to_string())?,
            NavigationAction::Forward => view.GoForward().map_err(|e| e.to_string())?,
        }
        let mut back = Default::default();
        let mut forward = Default::default();
        view.CanGoBack(&mut back).map_err(|e| e.to_string())?;
        view.CanGoForward(&mut forward).map_err(|e| e.to_string())?;
        let mut source = Default::default();
        view.Source(&mut source).map_err(|e| e.to_string())?;
        let url = source.to_string().map_err(|e| e.to_string());
        windows_sys::Win32::System::Com::CoTaskMemFree(source.0.cast());
        Ok((url?, back.as_bool(), forward.as_bool()))
    }
}

async fn read_or_navigate(
    app: AppHandle,
    session_id: Option<String>,
    action: NavigationAction,
) -> Result<PreviewNavigationSnapshot, String> {
    let state = app.state::<PreviewWebviewState>();
    let (webview, owner) = {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        let preview = guard.as_ref().ok_or("Preview is closed")?;
        if session_id.is_some() && preview.session_id != session_id {
            return Err("Preview owner changed".into());
        }
        (preview.webview.clone(), preview.session_id.clone())
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let callback_app = app.clone();
    let callback_owner = owner.clone();
    webview
        .with_webview(move |view| {
            let current = callback_app.state::<PreviewWebviewState>();
            let result = (|| {
                // This callback runs on the main thread. A lifecycle command
                // can hold the mutex while waiting for that same thread, so
                // reject a racing snapshot/action instead of blocking it.
                let guard = current.0.try_lock().map_err(|e| e.to_string())?;
                if guard
                    .as_ref()
                    .is_none_or(|preview| preview.session_id != callback_owner)
                {
                    return Err("Preview owner changed".into());
                }
                drop(guard);
                native_snapshot(view, action)
            })();
            let _ = sender.send(result);
        })
        .map_err(|e| e.to_string())?;
    let (url, can_go_back, can_go_forward) = receiver.await.map_err(|e| e.to_string())??;
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if guard
        .as_ref()
        .is_none_or(|preview| preview.session_id != owner)
    {
        return Err("Preview owner changed".into());
    }
    parse_web_url(&url)?;
    Ok(PreviewNavigationSnapshot {
        session_id: owner.unwrap_or_default(),
        url,
        can_go_back,
        can_go_forward,
    })
}

#[tauri::command]
pub async fn preview_navigation_state(
    app: AppHandle,
    session_id: String,
) -> Result<PreviewNavigationSnapshot, String> {
    read_or_navigate(app, Some(session_id), NavigationAction::Read).await
}

#[tauri::command]
pub async fn preview_back(app: AppHandle, session_id: Option<String>) -> Result<(), String> {
    read_or_navigate(app, session_id, NavigationAction::Back)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn preview_forward(app: AppHandle, session_id: Option<String>) -> Result<(), String> {
    read_or_navigate(app, session_id, NavigationAction::Forward)
        .await
        .map(|_| ())
}

#[tauri::command(async)]
pub fn preview_reload(state: tauri::State<'_, PreviewWebviewState>) -> Result<(), String> {
    eval_history(&state, "location.reload()")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_web_url_accepts_http_https_and_rejects_others() {
        assert!(parse_web_url("http://localhost:5173").is_ok());
        assert!(parse_web_url("https://example.com/page").is_ok());
        assert!(parse_web_url("file:///etc/passwd").is_err());
        assert!(parse_web_url("javascript:alert(1)").is_err());
        assert!(parse_web_url("not a url").is_err());
    }
}
