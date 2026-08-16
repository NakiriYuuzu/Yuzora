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
// Commands here deliberately stay synchronous (main thread) while the rest of the
// app moved to `(async)`: the native webview ops must land on the main thread
// anyway, and main-thread execution serializes rapid-fire `preview_set_bounds` /
// `preview_set_visible` in invoke order — on the async runtime two bounds updates
// could apply out of order and leave the overlay misplaced.

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

pub struct PreviewWebviewState(pub Mutex<Option<Webview>>);

fn parse_web_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|e| e.to_string())?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!("unsupported preview scheme: {other}")),
    }
}

#[tauri::command]
pub fn preview_open_url(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let parsed = parse_web_url(&url)?;
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;

    if let Some(webview) = guard.as_ref() {
        webview.navigate(parsed).map_err(|e| e.to_string())?;
        let _ = webview.set_position(LogicalPosition::new(x, y));
        let _ = webview.set_size(LogicalSize::new(width, height));
        let _ = webview.show();
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "main window missing".to_string())?;
    let builder = WebviewBuilder::new(PREVIEW_LABEL, WebviewUrl::External(parsed));
    let webview = window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|e| e.to_string())?;
    *guard = Some(webview);
    Ok(())
}

#[tauri::command]
pub fn preview_set_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(webview) = guard.as_ref() {
        let _ = webview.set_position(LogicalPosition::new(x, y));
        let _ = webview.set_size(LogicalSize::new(width, height));
    }
    Ok(())
}

#[tauri::command]
pub fn preview_set_visible(
    visible: bool,
    state: tauri::State<'_, PreviewWebviewState>,
) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(webview) = guard.as_ref() {
        let _ = if visible {
            webview.show()
        } else {
            webview.hide()
        };
    }
    Ok(())
}

#[tauri::command]
pub fn preview_close(state: tauri::State<'_, PreviewWebviewState>) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(webview) = guard.take() {
        let _ = webview.close();
    }
    Ok(())
}

fn eval_history(state: &tauri::State<'_, PreviewWebviewState>, js: &str) -> Result<(), String> {
    let guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(webview) = guard.as_ref() {
        webview.eval(js).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn preview_back(state: tauri::State<'_, PreviewWebviewState>) -> Result<(), String> {
    eval_history(&state, "history.back()")
}

#[tauri::command]
pub fn preview_forward(state: tauri::State<'_, PreviewWebviewState>) -> Result<(), String> {
    eval_history(&state, "history.forward()")
}

#[tauri::command]
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
