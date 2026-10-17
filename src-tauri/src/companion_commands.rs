//! Tauri commands for the Companion (plan 4.2): settings, the Tailscale Serve
//! handler, the gateway sidecar, pairing and devices. Main window only, like
//! every command; events go to the main webview, never the Browser child.

use crate::{
    companion_service::{device_summary, CompanionManager, Launch, Status},
    companion_store::{device_id, AppliedServe, Devices, EnableFor, Settings},
    companion_tailscale as tailscale,
    herdr_service::HerdrState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub const EVENT: &str = "companion://event";

pub struct CompanionState {
    manager: Arc<CompanionManager>,
    dir: PathBuf,
    resource_dir: Option<PathBuf>,
    /// Serialises Serve changes and settings writes.
    ops: Arc<tokio::sync::Mutex<()>>,
}
impl CompanionState {
    pub fn new(app: &tauri::AppHandle, dir: PathBuf, resource_dir: Option<PathBuf>) -> Self {
        use tauri::Emitter;
        let ops = Arc::new(tokio::sync::Mutex::new(()));
        let emitter = app.clone();
        let cleanup = (dir.clone(), ops.clone());
        let manager = CompanionManager::new(
            dir.clone(),
            Arc::new(move |event| {
                let _ = emitter.emit_to("main", EVENT, event);
            }),
            // Expiry, a crash or a halted gateway: nothing serves the port any more.
            Arc::new(move || {
                let (dir, ops) = cleanup.clone();
                tauri::async_runtime::spawn(async move {
                    let _turn = ops.lock().await;
                    if let Err(code) = remove_applied_serve(&dir).await {
                        log_warning("companion_serve_cleanup", &code);
                    }
                });
            }),
        );
        // No handler may outlive its gateway: one left by a crash or a quit
        // points at a loopback port anyone can bind. Enable re-applies it.
        let stale = (dir.clone(), ops.clone());
        tauri::async_runtime::spawn(async move {
            let (dir, ops) = stale;
            let _turn = ops.lock().await;
            if let Err(code) = remove_applied_serve(&dir).await {
                log_warning("companion_serve_cleanup", &code);
            }
        });
        Self {
            manager,
            dir,
            resource_dir,
            ops,
        }
    }
    /// App exit: bounded, and a no-op once stopped. A handler this cannot
    /// remove in time (or a killed process) is removed at the next start.
    pub fn shutdown(&self) {
        self.manager.stop_blocking();
        let (dir, ops) = (self.dir.clone(), self.ops.clone());
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        tauri::async_runtime::spawn(async move {
            let _turn = ops.lock().await;
            if let Err(code) = remove_applied_serve(&dir).await {
                log_warning("companion_serve_cleanup", &code);
            }
            let _ = done.send(());
        });
        let _ = wait.recv_timeout(Duration::from_secs(5));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionOverview {
    status: Status,
    settings: Settings,
}

#[tauri::command]
pub fn companion_status(
    state: tauri::State<'_, CompanionState>,
) -> Result<CompanionOverview, String> {
    Ok(CompanionOverview {
        status: state.manager.status(),
        settings: Settings::load(&state.dir)?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleProbe {
    cli: Option<tailscale::Cli>,
    node: Option<tailscale::Node>,
    serve: Option<tailscale::Serve>,
    refusal: Option<tailscale::Refusal>,
    https_port: u16,
    gateway_port: u16,
    /// Shown verbatim in the confirmation dialog.
    command: String,
}

#[tauri::command]
pub async fn companion_tailscale_probe(
    state: tauri::State<'_, CompanionState>,
) -> Result<TailscaleProbe, String> {
    let settings = Settings::load(&state.dir)?;
    probe(&settings).await
}

#[tauri::command]
pub async fn companion_tailscale_apply(
    state: tauri::State<'_, CompanionState>,
    confirm: bool,
) -> Result<TailscaleProbe, String> {
    if !confirm {
        return Err("companion-confirmation-required".into());
    }
    let _turn = state.ops.lock().await;
    let mut settings = Settings::load(&state.dir)?;
    // Applying elsewhere would drop the only record of the older handler.
    if settings.applied.as_ref().is_some_and(|applied| {
        (applied.https_port, applied.gateway_port) != (settings.https_port, settings.gateway_port)
    }) {
        return Err("companion-tailscale-remove-first".into());
    }
    // Re-check right before running: the user confirmed an earlier probe.
    let before = probe(&settings).await?;
    let (Some(cli), Some(node), Some(serve)) = (&before.cli, &before.node, &before.serve) else {
        return Err("companion-tailscale-unavailable".into());
    };
    if let Some(refusal) = before.refusal {
        return Err(format!(
            "companion-tailscale-refused: {}",
            refusal_code(refusal)
        ));
    }
    // Recorded before the CLI runs: whatever it leaves behind, even after a
    // crash, cleanup can find and remove.
    settings.applied = Some(AppliedServe {
        https_port: settings.https_port,
        gateway_port: settings.gateway_port,
        host: node.dns_name.clone().unwrap_or_default(),
        login: node.login.clone().unwrap_or_default(),
    });
    settings.save(&state.dir)?;
    let applied = async {
        if serve.port == tailscale::PortState::Free {
            tailscale::run(
                &cli.path,
                &tailscale::serve_args(settings.https_port, settings.gateway_port),
            )
            .await?;
        }
        // Without the https capability the CLI can exit 0 having applied nothing.
        settle(&settings, tailscale::PortState::Ours).await
    }
    .await;
    if applied.is_err() {
        // The handler may still land after a timeout: keep the record unless
        // ours was seen and turned off.
        if let Err(code) = remove_serve(&state.dir, false).await {
            log_warning("companion_serve_cleanup", &code);
        }
    }
    applied
}

#[tauri::command]
pub async fn companion_tailscale_remove(
    state: tauri::State<'_, CompanionState>,
) -> Result<(), String> {
    let _turn = state.ops.lock().await;
    remove_applied_serve(&state.dir).await
}

#[tauri::command]
pub async fn companion_enable(
    state: tauri::State<'_, CompanionState>,
    herdr: tauri::State<'_, HerdrState>,
    window: EnableFor,
) -> Result<Status, String> {
    let _turn = state.ops.lock().await;
    let mut settings = Settings::load(&state.dir)?;
    let applied = settings
        .applied
        .clone()
        .ok_or("companion-tailscale-not-applied")?;
    // The handler and tailnet identity must still be the ones applied.
    let current = probe(&settings).await?;
    let still_ours = current.serve.as_ref().map(|s| s.port) == Some(tailscale::PortState::Ours)
        && settings.https_port == applied.https_port
        && settings.gateway_port == applied.gateway_port;
    let same_login =
        current.node.as_ref().and_then(|n| n.login.as_deref()) == Some(applied.login.as_str());
    if !still_ours || !same_login || current.refusal.is_some() {
        return Err("companion-tailscale-changed".into());
    }
    let manager = herdr.0.clone();
    let herdr_binary = tauri::async_runtime::spawn_blocking(move || manager.resolve_binary())
        .await
        .map_err(|e| e.to_string())?
        .ok_or("companion-herdr-unavailable")?;
    settings.duration = window;
    settings.save(&state.dir)?;
    let until = chrono::Utc::now()
        + chrono::Duration::from_std(window.as_std()).map_err(|e| e.to_string())?;
    let launch = Launch {
        program: sidecar_program()?,
        init: build_init(
            &settings,
            &applied,
            &herdr_binary,
            &web_root(state.resource_dir.as_deref())?,
            &until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )?,
    };
    state.manager.start(launch)?;
    Ok(state.manager.status())
}

#[tauri::command]
pub async fn companion_disable(state: tauri::State<'_, CompanionState>) -> Result<(), String> {
    // Taken first so an enable cannot start between the stop and the removal.
    let _turn = state.ops.lock().await;
    state.manager.stop().await;
    remove_applied_serve(&state.dir).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingQr {
    pairing_id: String,
    /// Encodes the pairing secret: render it, never store or log it.
    qr_svg: String,
    expires_at: String,
}

#[tauri::command]
pub async fn companion_pairing_create(
    state: tauri::State<'_, CompanionState>,
    path: String,
    add_to_device: Option<String>,
) -> Result<PairingQr, String> {
    if path != "tailscale" {
        return Err("companion-path-unsupported".into());
    }
    let created = state.manager.pairing_create(add_to_device).await?;
    let url = created["url"].as_str().ok_or("companion-invalid-reply")?;
    Ok(PairingQr {
        pairing_id: created["pairingId"].as_str().unwrap_or_default().into(),
        qr_svg: qr_svg(url)?,
        expires_at: created["expiresAt"].as_str().unwrap_or_default().into(),
    })
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingDecision {
    Approve,
    Reject,
    Cancel,
}

#[tauri::command]
pub fn companion_pairing_decide(
    state: tauri::State<'_, CompanionState>,
    pairing_id: String,
    decision: PairingDecision,
) -> Result<(), String> {
    let kind = match decision {
        PairingDecision::Approve => "pairing.approve",
        PairingDecision::Reject => "pairing.reject",
        PairingDecision::Cancel => "pairing.cancel",
    };
    state.manager.pairing_decide(kind, &pairing_id)
}

#[tauri::command]
pub fn companion_devices(state: tauri::State<'_, CompanionState>) -> Result<Vec<Value>, String> {
    state.manager.devices()
}

#[tauri::command]
pub async fn companion_device_update(
    state: tauri::State<'_, CompanionState>,
    device_id: String,
    label: Option<String>,
    grants: Option<Value>,
) -> Result<Value, String> {
    if let Some(label) = &label {
        valid_label(label)?;
    }
    if let Some(grants) = &grants {
        valid_grants(grants)?;
    }
    change_device(
        &state,
        json!({"type": "device.update", "deviceId": device_id, "label": label, "grants": grants}),
    )
    .await?;
    stored_device(&state.dir, &device_id)
}

#[tauri::command]
pub async fn companion_device_unlock(
    state: tauri::State<'_, CompanionState>,
    device_id: String,
) -> Result<Value, String> {
    change_device(
        &state,
        json!({"type": "device.unlock", "deviceId": device_id}),
    )
    .await?;
    stored_device(&state.dir, &device_id)
}

#[tauri::command]
pub async fn companion_device_revoke(
    state: tauri::State<'_, CompanionState>,
    device_id: String,
) -> Result<(), String> {
    change_device(
        &state,
        json!({"type": "device.revoke", "deviceId": device_id}),
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsSet {
    /// The gateway reads the allowlist from init only.
    restart_required: bool,
}

#[tauri::command]
pub async fn companion_sessions_set(
    state: tauri::State<'_, CompanionState>,
    sessions: Vec<String>,
) -> Result<SessionsSet, String> {
    if sessions
        .iter()
        .any(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
    {
        return Err("companion-session-invalid".into());
    }
    let _turn = state.ops.lock().await;
    let mut settings = Settings::load(&state.dir)?;
    settings.session_allowlist = sessions;
    settings.save(&state.dir)?;
    Ok(SessionsSet {
        restart_required: !matches!(
            state.manager.status(),
            Status::Disabled | Status::Failed { .. }
        ),
    })
}

/// While the gateway runs it owns device state and reports the change back;
/// once it is stopped the desktop edits the stored device directly. In
/// between, a starting gateway has already read the file and a stopping one
/// may still write it, so neither may be edited around.
async fn change_device(state: &CompanionState, frame: Value) -> Result<(), String> {
    if is_running(&state.manager) {
        return state.manager.device_command(frame).await.map(drop);
    }
    // Enable and disable change the status only while holding this.
    let _turn = state.ops.lock().await;
    edit_stopped(&state.manager, &state.dir, &frame)
}

fn edit_stopped(manager: &CompanionManager, dir: &Path, frame: &Value) -> Result<(), String> {
    if !manager.idle() {
        return Err("companion-busy".into());
    }
    Devices::load(dir)?.apply(frame)
}

fn stored_device(dir: &Path, id: &str) -> Result<Value, String> {
    Devices::load(dir)?
        .all()
        .iter()
        .find(|d| device_id(d) == Some(id))
        .map(device_summary)
        .ok_or_else(|| "companion-device-unknown".into())
}

fn is_running(manager: &CompanionManager) -> bool {
    matches!(manager.status(), Status::Running { .. })
}

async fn probe(settings: &Settings) -> Result<TailscaleProbe, String> {
    let command = std::iter::once("tailscale".to_string())
        .chain(tailscale::serve_args(
            settings.https_port,
            settings.gateway_port,
        ))
        .collect::<Vec<_>>()
        .join(" ");
    let mut result = TailscaleProbe {
        cli: tailscale::discover(),
        node: None,
        serve: None,
        refusal: None,
        https_port: settings.https_port,
        gateway_port: settings.gateway_port,
        command,
    };
    let Some(cli) = &result.cli else {
        return Ok(result);
    };
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let node =
        tailscale::parse_status(&tailscale::run(&cli.path, &args(&["status", "--json"])).await?)?;
    if let Some(host) = node.dns_name.clone() {
        let json = tailscale::run(&cli.path, &args(&["serve", "status", "--json"])).await?;
        let serve =
            tailscale::check_serve(&json, &host, settings.https_port, settings.gateway_port)?;
        result.refusal = tailscale::refusal(&node, &serve);
        result.serve = Some(serve);
    } else {
        result.refusal = Some(tailscale::Refusal::NoLogin);
    }
    result.node = Some(node);
    Ok(result)
}

/// Serve changes land asynchronously in tailscaled; re-read until they show.
async fn settle(settings: &Settings, want: tailscale::PortState) -> Result<TailscaleProbe, String> {
    for attempt in 0..5 {
        let current = probe(settings).await?;
        if current.serve.as_ref().map(|s| s.port) == Some(want) {
            return Ok(current);
        }
        tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
    }
    Err("companion-tailscale-unverified".into())
}

/// Turns off only the handler this desktop applied, and only while it is
/// still exactly ours. A handler someone replaced is no longer ours to touch.
/// The record is kept whenever ours might still be serving somewhere.
async fn remove_applied_serve(dir: &Path) -> Result<(), String> {
    remove_serve(dir, true).await
}

/// `forget_absent`: also drop the record when nothing of ours is found.
async fn remove_serve(dir: &Path, forget_absent: bool) -> Result<(), String> {
    let mut settings = Settings::load(dir)?;
    let Some(applied) = settings.applied.clone() else {
        return Ok(());
    };
    let at_applied = Settings {
        https_port: applied.https_port,
        gateway_port: applied.gateway_port,
        ..settings.clone()
    };
    let current = probe(&at_applied).await?;
    let cli = current
        .cli
        .as_ref()
        .ok_or("companion-tailscale-unavailable")?;
    let host = current.node.as_ref().and_then(|n| n.dns_name.as_deref());
    match removal(current.serve.as_ref(), host, &applied)? {
        Some(off) => {
            tailscale::run(&cli.path, &off).await?;
            settle(&at_applied, tailscale::PortState::Free).await?;
        }
        None if !forget_absent => return Err("companion-tailscale-unverified".into()),
        None => {}
    }
    settings.applied = None;
    settings.save(dir)
}

/// The `serve … off` arguments, or `None` once nothing of ours is left on
/// the node it was applied to (free, or replaced). Logged out, or another
/// profile or node name: ours may still be configured there, so keep the record.
fn removal(
    serve: Option<&tailscale::Serve>,
    host: Option<&str>,
    applied: &AppliedServe,
) -> Result<Option<Vec<String>>, String> {
    let serve = serve.ok_or("companion-tailscale-unavailable")?;
    if host != Some(applied.host.as_str()) {
        return Err("companion-tailscale-changed".into());
    }
    Ok(tailscale::off_args(serve, applied.https_port))
}

fn refusal_code(refusal: tailscale::Refusal) -> String {
    serde_json::to_value(refusal)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn public_host(host: &str, https_port: u16) -> String {
    let host = host.to_ascii_lowercase();
    // Browsers omit the default port from Origin; the gateway rejects ":443".
    if https_port == 443 {
        host
    } else {
        format!("{host}:{https_port}")
    }
}

fn build_init(
    settings: &Settings,
    applied: &AppliedServe,
    herdr_binary: &Path,
    web_root: &Path,
    enable_until: &str,
) -> Result<Value, String> {
    if applied.host.is_empty() || applied.login.is_empty() {
        return Err("companion-tailscale-not-applied".into());
    }
    Ok(json!({
        "v": 1,
        "type": "init",
        "herdrBinary": herdr_binary,
        "sessionAllowlist": settings.session_allowlist,
        "webRoot": web_root,
        "listeners": {
            "loopback": {
                "port": applied.gateway_port,
                "publicHost": public_host(&applied.host, applied.https_port),
                "requiredTailscaleLogin": applied.login,
            },
            "lan": null,
        },
        "policy": {
            "enableUntil": enable_until,
            "viewIdleMin": 30,
            "viewAbsoluteHours": 12,
            "elevationMaxMin": 15,
            "fullSessionEnabled": settings.full_session_enabled,
            "maxDevices": 5,
        },
    }))
}

fn sidecar_program() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("YUZORA_COMPANION_BIN") {
        return Ok(path.into());
    }
    let name = if cfg!(windows) {
        "yuzora-companion.exe"
    } else {
        "yuzora-companion"
    };
    let path = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(name)))
        .filter(|path| path.is_file());
    path.ok_or_else(|| "companion-unavailable".into())
}

fn web_root(resource_dir: Option<&Path>) -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("YUZORA_COMPANION_WEB") {
        return Ok(path.into());
    }
    resource_dir
        .map(|dir| dir.join("companion-web"))
        .filter(|root| root.is_dir())
        .ok_or_else(|| "companion-web-unavailable".into())
}

fn qr_svg(url: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|_| "companion-qr-failed")?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build())
}

/// The gateway's own label rule (`companion/src/devices.rs`): a label it
/// rejects would make init fail at the next start.
fn valid_label(label: &str) -> Result<(), String> {
    if !label.is_empty()
        && label.len() <= 80
        && label.trim() == label
        && label.chars().all(|c| !c.is_control() && !label_format(c))
    {
        Ok(())
    } else {
        Err("companion-label-invalid".into())
    }
}

// Mirrors the gateway's explicit Unicode Cf list.
fn label_format(c: char) -> bool {
    matches!(c as u32,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891 |
        0x08e2 | 0x180e | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 |
        0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd |
        0x13430..=0x1343f | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a |
        0xe0001 | 0xe0020..=0xe007f)
}

/// The gateway's timestamp rule (`companion/src/clock.rs`): chrono alone
/// accepts more RFC 3339 variants than the admin protocol.
fn valid_until(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(core) = bytes
        .strip_suffix(b"Z")
        .or_else(|| bytes.strip_suffix(b"+00:00"))
    else {
        return false;
    };
    let shape = core.len() >= 19
        && core[..19].iter().enumerate().all(|(i, byte)| match i {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            _ => byte.is_ascii_digit(),
        })
        && (core.len() == 19
            || ((21..=29).contains(&core.len())
                && core[19] == b'.'
                && core[20..].iter().all(u8::is_ascii_digit)));
    shape
        && chrono::DateTime::parse_from_rfc3339(value)
            .is_ok_and(|parsed| parsed.timestamp_subsec_nanos() < 1_000_000_000)
}

/// `{control: {until} | null, full: bool}`, exactly; init rejects anything else.
fn valid_grants(grants: &Value) -> Result<(), String> {
    let Some(object) = grants.as_object() else {
        return Err("companion-grants-invalid".into());
    };
    let control_ok = match object.get("control") {
        Some(Value::Null) => true,
        Some(Value::Object(control)) => {
            control.len() == 1
                && control
                    .get("until")
                    .and_then(Value::as_str)
                    .is_some_and(valid_until)
        }
        _ => false,
    };
    if object.len() == 2 && control_ok && object.get("full").is_some_and(Value::is_boolean) {
        Ok(())
    } else {
        Err("companion-grants-invalid".into())
    }
}

fn log_warning(event: &str, message: &str) {
    crate::logging::write_global(crate::logging::LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: "companion".to_string(),
        workspace_path: None,
        event: event.to_string(),
        message: message.to_string(),
        metadata: json!({}),
    });
}

#[cfg(test)]
mod tests;
