//! Tauri commands for the Companion (plan 4.2): settings, the Tailscale Serve
//! handler, the gateway sidecar, pairing and devices. Main window only, like
//! every command; events go to the main webview, never the Browser child.

use crate::{
    companion_service::{device_summary, CompanionManager, Launch, Status},
    companion_store::{
        device_id, valid_grants, valid_label, AppliedServe, Devices, EnableFor, Settings,
    },
    companion_tailscale as tailscale,
    herdr_service::HerdrState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

pub const EVENT: &str = "companion://event";

pub struct CompanionState {
    manager: Arc<CompanionManager>,
    dir: PathBuf,
    resource_dir: Option<PathBuf>,
    /// Serialises Serve changes and settings writes.
    ops: Arc<tokio::sync::Mutex<()>>,
    /// Held for the life of the process by the one Yuzora instance that owns
    /// the Companion; another instance only reads.
    owner: Option<std::fs::File>,
}
impl CompanionState {
    pub fn new(app: &tauri::AppHandle, dir: PathBuf, resource_dir: Option<PathBuf>) -> Self {
        use tauri::Emitter;
        let owner = instance_lock(&dir);
        let ops = Arc::new(tokio::sync::Mutex::new(()));
        let emitter = app.clone();
        let current: Arc<OnceLock<Weak<CompanionManager>>> = Arc::default();
        let cleanup = (dir.clone(), ops.clone(), current.clone());
        let manager = CompanionManager::new(
            dir.clone(),
            Arc::new(move |event| {
                let _ = emitter.emit_to("main", EVENT, event);
            }),
            // Expiry, a crash or a halted gateway: nothing serves the port any more.
            Arc::new(move || {
                let (dir, ops, current) = cleanup.clone();
                tauri::async_runtime::spawn(async move {
                    let _turn = ops.lock().await;
                    // An enable that took the lock first runs a new gateway
                    // behind the same handler.
                    if current
                        .get()
                        .and_then(Weak::upgrade)
                        .is_some_and(|manager| !manager.idle())
                    {
                        return;
                    }
                    if let Err(code) = remove_applied_serve(&dir).await {
                        log_warning("companion_serve_cleanup", &code);
                    }
                });
            }),
        );
        let _ = current.set(Arc::downgrade(&manager));
        // No handler may outlive its gateway: one left by a crash or a quit
        // points at a loopback port anyone can bind. Enable re-applies it.
        // The lock is taken here, before any command can enable.
        if owner.is_some() {
            let turn = ops.clone().try_lock_owned().expect("unshared at startup");
            let stale = dir.clone();
            tauri::async_runtime::spawn(async move {
                let _turn = turn;
                if let Err(code) = remove_applied_serve(&stale).await {
                    log_warning("companion_serve_cleanup", &code);
                }
            });
        }
        Self {
            manager,
            dir,
            resource_dir,
            ops,
            owner,
        }
    }
    /// Another running instance owns the Companion. Ownership is decided at
    /// startup: this one needs a restart after that one quits.
    fn owner(&self) -> Result<(), String> {
        self.owner
            .as_ref()
            .map(drop)
            .ok_or_else(|| "companion-other-instance".into())
    }
    /// App exit, bounded: the handler goes first (at most 5 s), so nothing
    /// forwards to a port the gateway is leaving, then the gateway. A handler
    /// left behind (or a killed process) is removed at the next start.
    pub fn shutdown(&self) {
        if self.owner.is_none() {
            return;
        }
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
        self.manager.stop_blocking();
    }
}

/// An advisory lock on `<dir>/instance.lock`; released when the process ends.
fn instance_lock(dir: &Path) -> Option<std::fs::File> {
    crate::companion_store::create_private_dir(dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
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
pub async fn companion_tailscale_remove(
    state: tauri::State<'_, CompanionState>,
) -> Result<(), String> {
    state.owner()?;
    let _turn = state.ops.lock().await;
    remove_applied_serve(&state.dir).await
}

/// Starts the gateway, and only once it listens applies the Serve handler
/// in front of it: the handler never points at a port nothing has bound.
/// Runs `tailscale serve`, so the user confirms the command shown by the probe.
#[tauri::command]
pub async fn companion_enable(
    state: tauri::State<'_, CompanionState>,
    herdr: tauri::State<'_, HerdrState>,
    window: EnableFor,
    confirm: bool,
) -> Result<Status, String> {
    if !confirm {
        return Err("companion-confirmation-required".into());
    }
    state.owner()?;
    let _turn = state.ops.lock().await;
    // Before anything changes: a running gateway keeps its own enableUntil
    // and the handler it is serving behind.
    if !state.manager.idle() {
        return Err("companion-already-running".into());
    }
    let started = enable(&state, &herdr, window).await;
    if started.is_err() {
        // Handler first, then the gateway behind it. A handler that may
        // still land after a CLI timeout keeps its record.
        if let Err(code) = remove_serve(&state.dir, false).await {
            log_warning("companion_serve_cleanup", &code);
        }
        state.manager.stop().await;
    }
    started
}

async fn enable(
    state: &CompanionState,
    herdr: &HerdrState,
    window: EnableFor,
) -> Result<Status, String> {
    let mut settings = Settings::load(&state.dir)?;
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
    // A record for another port or node is the only handle on a handler that
    // may still exist there; applying here would overwrite it.
    if settings.applied.as_ref().is_some_and(|applied| {
        (applied.https_port, applied.gateway_port) != (settings.https_port, settings.gateway_port)
            || node.dns_name.as_deref() != Some(applied.host.as_str())
    }) {
        return Err("companion-tailscale-remove-first".into());
    }
    let applied = AppliedServe {
        https_port: settings.https_port,
        gateway_port: settings.gateway_port,
        host: node.dns_name.clone().unwrap_or_default(),
        login: node.login.clone().unwrap_or_default(),
    };
    let manager = herdr.0.clone();
    let herdr_binary = tauri::async_runtime::spawn_blocking(move || manager.resolve_binary())
        .await
        .map_err(|e| e.to_string())?
        .ok_or("companion-herdr-unavailable")?;
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
    // Recorded before the gateway or the CLI runs: whatever is left behind,
    // even after a crash, cleanup can find and remove.
    settings.duration = window;
    settings.applied = Some(applied);
    settings.save(&state.dir)?;
    state.manager.start(launch)?;
    state.manager.until_ready().await?;
    if serve.port == tailscale::PortState::Free {
        tailscale::run(
            &cli.path,
            &tailscale::serve_args(settings.https_port, settings.gateway_port),
        )
        .await?;
    }
    // Without the https capability the CLI can exit 0 having applied nothing.
    settle(&settings, |port| port == tailscale::PortState::Ours).await?;
    Ok(state.manager.status())
}

#[tauri::command]
pub async fn companion_disable(state: tauri::State<'_, CompanionState>) -> Result<(), String> {
    state.owner()?;
    // Taken first so an enable cannot start in between.
    let _turn = state.ops.lock().await;
    // Handler first: nothing may forward to a port the gateway is leaving.
    // The gateway stops even if that fails; the record stays for a retry.
    let removed = remove_applied_serve(&state.dir).await;
    state.manager.stop().await;
    removed
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
    state.owner()?;
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
pub async fn companion_pairing_decide(
    state: tauri::State<'_, CompanionState>,
    pairing_id: String,
    decision: PairingDecision,
) -> Result<(), String> {
    state.owner()?;
    let kind = match decision {
        PairingDecision::Approve => "pairing.approve",
        PairingDecision::Reject => "pairing.reject",
        PairingDecision::Cancel => "pairing.cancel",
    };
    state
        .manager
        .pairing_decide(kind, &pairing_id)
        .await
        .map(drop)
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
    state.owner()?;
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
    state.owner()?;
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
    state.owner()?;
    if is_running(&state.manager) {
        return state.manager.device_command(frame).await.map(drop);
    }
    // Enable and disable change the status only while holding this.
    let _turn = state.ops.lock().await;
    state.manager.edit_stopped(&frame)
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
async fn settle(
    settings: &Settings,
    done: impl Fn(tailscale::PortState) -> bool,
) -> Result<TailscaleProbe, String> {
    for attempt in 0..5 {
        let current = probe(settings).await?;
        if current.serve.as_ref().is_some_and(|serve| done(serve.port)) {
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
            // A port the user also mounted something on stays, without our root.
            settle(&at_applied, |port| {
                !matches!(
                    port,
                    tailscale::PortState::Ours | tailscale::PortState::Shared
                )
            })
            .await?;
        }
        None if !forget_absent => return Err("companion-tailscale-unverified".into()),
        None => {}
    }
    settings.applied = None;
    settings.save(dir)
}

/// The `serve … off` arguments, or `None` once nothing of ours is left on
/// the node it was applied to (free, or our root replaced). Logged out, or another
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
