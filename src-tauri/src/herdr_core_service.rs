//! Tauri adapter for the experimental herdr-core runtime (`yuzora_host::herdr_core`).
//!
//! Every command is `herdr_core_*`, registered in `lib.rs`. Three are the control plane and
//! work while the switch is off (`herdr_core_status`, `herdr_core_set_enabled`,
//! `herdr_core_set_session`); all the others reject with the `disabled` code and neither
//! connect nor start anything. Errors are a [`CoreError`] (`{code, message, detail?}`).
//! A debug build refuses `herdr_core_set_session` with `dev_session_fixed` and writes nothing:
//! it always uses [`runtime_preferences::DEV_SESSION`], and the session field of the
//! preferences file is shared with the installed app.
//!
//! * Status: the `herdr-core:status` event (a [`CoreStatus`]) on every change, and
//!   `herdr_core_status` for the current one. An event, not a channel: the status exists
//!   with no subscriber (the settings card reads it with the switch off), and it survives
//!   a WebView reload, which orphans channels (their JS callbacks are gone).
//! * Attach: one `Channel<InvokeResponseBody>` per attach carrying raw output bytes as `Raw`
//!   bodies (an `ArrayBuffer` in JS) and control messages as `Json` bodies, in order. The
//!   `closed` message therefore always arrives after the last byte, which two channels could
//!   not promise.
//! * Input and the other calls to an attach (`resize`, `focus`, `facts`, `credit`, `detach`)
//!   only queue the call on the attach's lane and return: calls are written in the order they
//!   were made (a `detach` after an `input` writes the input first), and nothing waits on the
//!   socket. `herdr_core_input` takes the bytes as the raw request body and the attach id in
//!   the `x-attach-id` header; a WebView that cannot send raw bodies sends a JSON array of
//!   byte values instead, which is accepted too (see [`parse_input_request`]).
//! * Events and layout: `herdr_core_events_subscribe` takes a `Channel<CoreEvent>`.
//! * A reloaded WebView: Tauri's `Channel::send` does not fail for a page that was replaced,
//!   so the viewers and subscriptions it left behind are released when the main WebView starts
//!   loading a page ([`on_page_load`], hooked in `lib.rs`), and again by the frontend's own
//!   `herdr_core_release_all` at start-up. An attach or subscription still waiting for its
//!   answer when the page started loading is not registered yet, so it is released as it
//!   registers instead, and rejects with `page_replaced`.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request};
use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{Emitter, Manager};
use yuzora_host::herdr_core::client::{AttachRequest, DisplayFacts, LayoutSnapshot};
use yuzora_host::herdr_core::docs::Op;

use crate::runtime_preferences::{self, HerdrCoreSession, RuntimePreferencesState};

pub mod binary;
pub mod bundled_start;
pub mod error;
pub mod events;
#[cfg(all(test, unix))]
mod fake_core;
pub mod service;
pub mod status;
#[cfg(test)]
mod tests;

pub use error::CoreError;
pub use events::{AttachMessage, CoreEvent};
pub use service::{
    AppliedLayout, AttachInfo, AttachOut, Deps, HerdrCore, LaneCommand, EXIT_RELEASE_DEADLINE,
};
pub use status::{CoreStatus, STATUS_EVENT};

/// The attach header `herdr_core_input` reads the attach id from.
pub const ATTACH_ID_HEADER: &str = "x-attach-id";
/// Scrollback lines an attach asks for when the caller names none: only the main screen.
const DEFAULT_HISTORY: u32 = 0;

pub struct HerdrCoreState(pub Arc<HerdrCore>);

impl HerdrCoreState {
    /// Builds the service for this app and applies the persisted switch: with it off nothing
    /// is connected or started.
    pub fn launch(
        app: tauri::AppHandle,
        data_dir: std::path::PathBuf,
        resource_dir: Option<std::path::PathBuf>,
        (enabled, session): (bool, &str),
    ) -> HerdrCoreState {
        let core = Arc::new(HerdrCore::new(Deps::production(move |status| {
            if let Err(error) = app.emit_to("main", STATUS_EVENT, status) {
                eprintln!("herdr-core status event failed: {error}");
            }
        })));
        core.configure_paths(data_dir, resource_dir);
        core.apply_config(enabled, session);
        HerdrCoreState(core)
    }
}

/// The one WebView that talks to herdr-core (the window Tauri creates from `tauri.conf.json`
/// is labelled `main`; the Browser panel's child webview is not).
const MAIN_WEBVIEW: &str = "main";

/// `tauri::Builder::on_page_load` hook (registered in `lib.rs`): releases what a replaced
/// page left behind, so no output is produced for nobody and the core's credit window is not
/// held by a dead viewer. Returns at once; the socket writes happen on another thread.
pub fn on_page_load<R: tauri::Runtime>(webview: &tauri::Webview<R>, payload: &PageLoadPayload<'_>) {
    let state = webview.try_state::<HerdrCoreState>();
    page_loaded(webview.label(), payload.event(), state.as_deref());
}

/// The viewers and subscriptions a page held are gone when the main WebView starts loading a
/// document (a reload, or the dev server's full refresh). `Finished` is too late: the new
/// page may already have created its own. Other WebViews (the Browser panel) never held any.
fn page_load_releases(label: &str, event: PageLoadEvent) -> bool {
    label == MAIN_WEBVIEW && event == PageLoadEvent::Started
}

fn page_loaded(label: &str, event: PageLoadEvent, state: Option<&HerdrCoreState>) {
    if let (true, Some(state)) = (page_load_releases(label, event), state) {
        state.0.release_for_reload();
    }
}

async fn blocking<T: Send + 'static>(
    state: &HerdrCoreState,
    operation: impl FnOnce(Arc<HerdrCore>) -> Result<T, CoreError> + Send + 'static,
) -> Result<T, CoreError> {
    let core = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || operation(core))
        .await
        .map_err(|e| CoreError::internal(e.to_string()))?
}

// ---------------------------------------------------------------------------------- control plane

#[tauri::command]
pub fn herdr_core_status(state: tauri::State<'_, HerdrCoreState>) -> CoreStatus {
    state.0.status_snapshot()
}

/// Persists the switch (the debug build's own field in a debug build) and applies it.
/// Turning it off releases every viewer and closes the connection; the core keeps running.
#[tauri::command]
pub async fn herdr_core_set_enabled(
    app: tauri::AppHandle,
    state: tauri::State<'_, HerdrCoreState>,
    preferences: tauri::State<'_, RuntimePreferencesState>,
    enabled: bool,
) -> Result<CoreStatus, CoreError> {
    let dir = preferences_dir(&app)?;
    let preferences = preferences.inner().clone();
    blocking(&state, move |core| {
        change_enabled(&dir, &preferences, &core, enabled)
    })
    .await
}

/// What `herdr_core_set_enabled` does: this build's own switch (a debug build's is not the
/// installed app's, see [`runtime_preferences::DEV_BUILD`]).
fn change_enabled(
    dir: &std::path::Path,
    preferences: &RuntimePreferencesState,
    core: &HerdrCore,
    enabled: bool,
) -> Result<CoreStatus, CoreError> {
    change_enabled_for(
        dir,
        preferences,
        core,
        enabled,
        runtime_preferences::DEV_BUILD,
    )
}

/// `dev`: change the debug builds' switch instead of the installed one.
fn change_enabled_for(
    dir: &std::path::Path,
    preferences: &RuntimePreferencesState,
    core: &HerdrCore,
    enabled: bool,
    dev: bool,
) -> Result<CoreStatus, CoreError> {
    runtime_preferences::set_herdr_core_enabled(dir, preferences, enabled, dev)
        .map_err(CoreError::preferences_unwritable)?;
    apply_preferences(core, preferences);
    Ok(core.status_snapshot())
}

/// Persists the session (`yuzora` or `default`) and, when the switch is on, reconnects to it.
/// A debug build rejects this with `dev_session_fixed` and writes nothing: its session is
/// always `yuzora-dev`, and the field belongs to the installed app that shares the file.
#[tauri::command]
pub async fn herdr_core_set_session(
    app: tauri::AppHandle,
    state: tauri::State<'_, HerdrCoreState>,
    preferences: tauri::State<'_, RuntimePreferencesState>,
    session: String,
) -> Result<CoreStatus, CoreError> {
    let session =
        HerdrCoreSession::parse(&session).ok_or_else(|| CoreError::invalid_session(&session))?;
    let dir = preferences_dir(&app)?;
    let preferences = preferences.inner().clone();
    blocking(&state, move |core| {
        change_session(&dir, &preferences, &core, session)
    })
    .await
}

/// What `herdr_core_set_session` does: refused in a debug build, which this is or is not
/// ([`runtime_preferences::DEV_BUILD`]).
fn change_session(
    dir: &std::path::Path,
    preferences: &RuntimePreferencesState,
    core: &HerdrCore,
    session: HerdrCoreSession,
) -> Result<CoreStatus, CoreError> {
    change_session_for(
        dir,
        preferences,
        core,
        session,
        runtime_preferences::DEV_BUILD,
    )
}

/// `dev`: this is a debug build.
fn change_session_for(
    dir: &std::path::Path,
    preferences: &RuntimePreferencesState,
    core: &HerdrCore,
    session: HerdrCoreSession,
    dev: bool,
) -> Result<CoreStatus, CoreError> {
    runtime_preferences::set_herdr_core_session(dir, preferences, session, dev)
        .map_err(CoreError::from_session_change)?;
    apply_preferences(core, preferences);
    Ok(core.status_snapshot())
}

fn preferences_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, CoreError> {
    app.path()
        .app_data_dir()
        .map_err(|e| CoreError::preferences_unwritable(e.to_string()))
}

fn apply_preferences(core: &HerdrCore, preferences: &RuntimePreferencesState) {
    let (enabled, session) = preferences.herdr_core();
    core.apply_config(enabled, session);
}

// ---------------------------------------------------------------------------------- terminals

/// `terminal.list`: `{terminals, hidden_finished}` as the core returns it. The answer has no
/// `machines` field (K10); do not depend on one.
#[tauri::command]
pub async fn herdr_core_terminal_list(
    state: tauri::State<'_, HerdrCoreState>,
    all: Option<bool>,
) -> Result<Value, CoreError> {
    blocking(&state, move |core| core.terminal_list(all.unwrap_or(false))).await
}

/// `terminal.run` for a shell in `cwd`, with `TERM`, `COLORTERM` and `TERM_PROGRAM=yuzora`. Returns
/// the core's terminal record (its `address` is the id to attach to).
#[tauri::command]
pub async fn herdr_core_terminal_run(
    state: tauri::State<'_, HerdrCoreState>,
    cwd: Option<String>,
) -> Result<Value, CoreError> {
    blocking(&state, move |core| core.terminal_run(cwd)).await
}

/// `terminal.kill` then `terminal.rm`. A terminal that is already gone is not an error.
#[tauri::command]
pub async fn herdr_core_terminal_close(
    state: tauri::State<'_, HerdrCoreState>,
    terminal: String,
) -> Result<(), CoreError> {
    blocking(&state, move |core| core.terminal_close(&terminal)).await
}

// ---------------------------------------------------------------------------------- attach

/// What a viewer's terminal looks like; the core answers OSC 10/11 and cell-size queries
/// from it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DisplayFactsArg {
    pub bg: Option<String>,
    pub fg: Option<String>,
    pub cursor: Option<String>,
    /// Cell size in pixels: `[width, height]`.
    pub cell: Option<[u32; 2]>,
}

impl From<DisplayFactsArg> for DisplayFacts {
    fn from(facts: DisplayFactsArg) -> DisplayFacts {
        DisplayFacts {
            bg: facts.bg,
            fg: facts.fg,
            cursor: facts.cursor,
            cell: facts.cell,
        }
    }
}

/// Opens a viewer on `terminal` (`t7` or a name). `on_data` receives the output as raw bytes
/// and the end of the stream as a JSON `{"type":"closed","end":{...}}`. `history`: scrollback
/// lines to show first (the main screen only; default 0).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn herdr_core_attach(
    state: tauri::State<'_, HerdrCoreState>,
    terminal: String,
    cols: u16,
    rows: u16,
    history: Option<u32>,
    display: Option<DisplayFactsArg>,
    on_data: Channel<InvokeResponseBody>,
) -> Result<AttachInfo, CoreError> {
    blocking(&state, move |core| {
        let request = AttachRequest {
            terminal,
            cols,
            rows,
            history: history.unwrap_or(DEFAULT_HISTORY),
            display: display.unwrap_or_default().into(),
        };
        core.attach(&request, channel_sender(on_data))
    })
    .await
}

/// `false` means the frontend is gone for good (the WebView was destroyed): `Channel::send`
/// does not fail for a page that was merely replaced, which `on_page_load` covers.
fn channel_sender(channel: Channel<InvokeResponseBody>) -> service::AttachSend {
    Arc::new(move |out| {
        let body = match out {
            AttachOut::Bytes(bytes) => InvokeResponseBody::Raw(bytes),
            AttachOut::Message(message) => match serde_json::to_string(&message) {
                Ok(json) => InvokeResponseBody::Json(json),
                Err(_) => return true,
            },
        };
        channel.send(body).is_ok()
    })
}

/// Set once the JSON-array fallback of [`parse_input_request`] has been logged.
static JSON_INPUT_LOGGED: AtomicBool = AtomicBool::new(false);

/// True the first time it is called with `flag`, false after.
fn first_time(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::Relaxed)
}

/// The attach id and the bytes of an input request: the id in [`ATTACH_ID_HEADER`], the
/// bytes as the raw request body.
///
/// Once Tauri's custom-protocol IPC fails, the whole page falls back to `postMessage`, which
/// cannot carry a `Uint8Array` as a raw body: it arrives as a JSON array of byte values.
/// That is accepted too (every value must be 0-255) and logged once, so a page stuck on the
/// fallback still types.
pub fn parse_input_request(
    headers: &tauri::http::HeaderMap,
    body: &InvokeBody,
) -> Result<(String, Vec<u8>), CoreError> {
    let id = headers
        .get(ATTACH_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CoreError::invalid_input(format!("missing {ATTACH_ID_HEADER} header")))?;
    match body {
        InvokeBody::Raw(bytes) => Ok((id.to_string(), bytes.clone())),
        InvokeBody::Json(Value::Array(values)) => {
            if first_time(&JSON_INPUT_LOGGED) {
                eprintln!(
                    "herdr-core input arrives as a JSON array: the WebView fell back from the \
                     custom protocol to postMessage; accepting it"
                );
            }
            let bytes = values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|byte| u8::try_from(byte).ok())
                        .ok_or_else(|| {
                            CoreError::invalid_input(format!(
                                "herdr_core_input byte {value} is not an integer from 0 to 255"
                            ))
                        })
                })
                .collect::<Result<Vec<u8>, CoreError>>()?;
            Ok((id.to_string(), bytes))
        }
        InvokeBody::Json(_) => Err(CoreError::invalid_input(
            "herdr_core_input takes its bytes as the raw request body or an array of byte values",
        )),
    }
}

/// Typed or pasted bytes, as a terminal expects them (the core never encodes keys).
#[tauri::command]
pub fn herdr_core_input(
    state: tauri::State<'_, HerdrCoreState>,
    request: Request<'_>,
) -> Result<(), CoreError> {
    let (attach_id, bytes) = parse_input_request(request.headers(), request.body())?;
    state.0.lane(&attach_id, LaneCommand::Input(bytes))
}

#[tauri::command]
pub fn herdr_core_resize(
    state: tauri::State<'_, HerdrCoreState>,
    attach_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), CoreError> {
    state.0.lane(&attach_id, LaneCommand::Resize(cols, rows))
}

#[tauri::command]
pub fn herdr_core_focus(
    state: tauri::State<'_, HerdrCoreState>,
    attach_id: String,
    focused: bool,
) -> Result<(), CoreError> {
    state.0.lane(&attach_id, LaneCommand::Focus(focused))
}

#[tauri::command]
pub fn herdr_core_facts(
    state: tauri::State<'_, HerdrCoreState>,
    attach_id: String,
    facts: DisplayFactsArg,
) -> Result<(), CoreError> {
    state.0.lane(&attach_id, LaneCommand::Facts(facts.into()))
}

/// `bytes` of output were written into the renderer (dropped paths included). Not crediting
/// stalls the stream at the 256 KiB window.
#[tauri::command]
pub fn herdr_core_credit(
    state: tauri::State<'_, HerdrCoreState>,
    attach_id: String,
    bytes: u32,
) -> Result<(), CoreError> {
    state
        .0
        .lane(&attach_id, LaneCommand::Credit(bytes as usize))
}

/// Releases the viewer; the terminal keeps running. Releasing one that is gone is not an
/// error.
#[tauri::command]
pub fn herdr_core_detach(
    state: tauri::State<'_, HerdrCoreState>,
    attach_id: String,
) -> Result<(), CoreError> {
    state.0.detach(&attach_id)
}

/// Frontend start-up: releases the viewers and event subscriptions a reloaded WebView left
/// behind. Call it before creating any attach. (The host also does this when the main WebView
/// starts loading a page, see [`on_page_load`]; this covers whatever that missed.)
#[tauri::command]
pub async fn herdr_core_release_all(
    state: tauri::State<'_, HerdrCoreState>,
) -> Result<(), CoreError> {
    blocking(&state, |core| core.release_all()).await
}

// ---------------------------------------------------------------------------------- layout

/// Reads the layout document now.
#[tauri::command]
pub async fn herdr_core_layout_get(
    state: tauri::State<'_, HerdrCoreState>,
) -> Result<LayoutSnapshot, CoreError> {
    blocking(&state, |core| core.layout_get()).await
}

/// Applies one layout operation to the latest document (read, apply, write; on a conflict
/// read again and apply again). It does not end the terminals it removes from the layout:
/// `removedTerminals` names them, and the caller decides.
#[tauri::command]
pub async fn herdr_core_layout_apply(
    state: tauri::State<'_, HerdrCoreState>,
    op: Op,
) -> Result<AppliedLayout, CoreError> {
    blocking(&state, move |core| core.layout_apply(&op)).await
}

// ---------------------------------------------------------------------------------- events

/// Subscribes to terminals and layout (see [`CoreEvent`]): the current snapshot and layout
/// first, then every change, across reconnects. Returns the subscription id. The subscription
/// ends when it is released, the switch goes off, or the main WebView starts loading a page
/// (a reload orphans the channel without making `send` fail).
#[tauri::command]
pub async fn herdr_core_events_subscribe(
    state: tauri::State<'_, HerdrCoreState>,
    on_event: Channel<CoreEvent>,
) -> Result<String, CoreError> {
    blocking(&state, move |core| {
        core.subscribe(Arc::new(move |event| on_event.send(event.clone()).is_ok()))
    })
    .await
}

#[tauri::command]
pub async fn herdr_core_events_release(
    state: tauri::State<'_, HerdrCoreState>,
    subscription_id: String,
) -> Result<(), CoreError> {
    blocking(&state, move |core| core.unsubscribe(&subscription_id)).await
}
