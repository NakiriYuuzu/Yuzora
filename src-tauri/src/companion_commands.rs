//! Tauri commands for the Companion (plan 4.2): settings, the embedded
//! Tailscale node, the gateway sidecar, pairing, devices, the Spaces the
//! gateway may read Git in and Web Push. Main window only,
//! like every command. Events are emitted to the main webview, and only it
//! holds the event capability (`capabilities/default.json`), so the Browser
//! child webview cannot listen to them.

use crate::{
    companion_net::{self as net, NetManager, NetOverview, Notice},
    companion_push::{Push, PushOverview},
    companion_service::{device_summary, CompanionManager, Launch, Status},
    companion_store::{
        device_id, valid_grants, valid_label, Devices, EnableFor, NetIdentity, Settings,
    },
    herdr_service::HerdrState,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::Duration,
};

pub const EVENT: &str = "companion://event";

type Emit = Arc<dyn Fn(Value) + Send + Sync>;

pub struct CompanionState {
    manager: Arc<CompanionManager>,
    net: Arc<NetManager>,
    dir: PathBuf,
    resource_dir: Option<PathBuf>,
    /// Serialises enable, disable, login and logout, and settings writes.
    ops: Arc<tokio::sync::Mutex<()>>,
    /// Where the Spaces' Git is read. Held while the running gateway is told,
    /// so a change cannot slip between an enable's init frame and its start.
    roots: Mutex<Vec<SpaceRoot>>,
    /// Orders the node being told to serve against a gateway leaving.
    forwarding: Arc<Forwarding>,
    /// Held for the life of the process by the one Yuzora instance that owns
    /// the Companion; another instance only reads.
    owner: Option<std::fs::File>,
}

/// The node may forward to the gateway's port from the moment it is told to
/// serve. A gateway that leaves (a halt, an expiry) holds that port until the
/// node has stopped forwarding, so the node must not be told to serve a
/// gateway that has begun to leave, nor miss one that is being told.
#[derive(Default)]
struct Forwarding {
    /// Held from the check that the gateway is still there, through the serve
    /// command and the node's answer: a gateway that leaves meanwhile waits
    /// for it, and its `unserve` follows the `serve`.
    serving: tokio::sync::Mutex<()>,
    /// How many gateways began to leave. Counted before the wait for `serving`,
    /// so an enable that takes it afterwards sees it. An expiry leaves the
    /// status running until the gateway exits, so the status alone cannot say.
    leaving: AtomicUsize,
}

impl CompanionState {
    pub fn new(app: &tauri::AppHandle, dir: PathBuf, resource_dir: Option<PathBuf>) -> Self {
        use tauri::Emitter;
        let emitter = app.clone();
        Self::with_emit(
            Arc::new(move |event| {
                let _ = emitter.emit_to("main", EVENT, event);
            }),
            app.package_info().version.to_string(),
            net::TIMING,
            dir.clone(),
            resource_dir,
            Push::system(dir),
        )
    }

    fn with_emit(
        emit: Emit,
        version: String,
        timing: net::Timing,
        dir: PathBuf,
        resource_dir: Option<PathBuf>,
        push: Arc<Push>,
    ) -> Self {
        let owner = instance_lock(&dir);
        let ops = Arc::new(tokio::sync::Mutex::new(()));
        let gateway: Arc<OnceLock<Weak<CompanionManager>>> = Arc::default();
        let node: Arc<OnceLock<Weak<NetManager>>> = Arc::default();
        let cleanup = (ops.clone(), gateway.clone(), node.clone());
        let manager = CompanionManager::new(
            dir.clone(),
            leaving_first(emit.clone(), node.clone()),
            // Expiry, a crash or a halted gateway: nothing is left behind the
            // node, so it stops serving.
            Arc::new(move || {
                let (ops, gateway, node) = cleanup.clone();
                tauri::async_runtime::spawn(async move {
                    let _turn = ops.lock().await;
                    // An enable that took the lock first runs a new gateway
                    // behind the same node.
                    if gateway
                        .get()
                        .and_then(Weak::upgrade)
                        .is_some_and(|manager| !manager.idle())
                    {
                        return;
                    }
                    if let Some(net) = node.get().and_then(Weak::upgrade) {
                        net.unserve().await;
                        net.stop().await;
                    }
                });
            }),
            push,
        );
        let _ = gateway.set(Arc::downgrade(&manager));
        // Before a gateway lets go of its port (expiry) or is killed (a halt),
        // the node stops forwarding to it: its listener is closed, and when it
        // does not say so within its bound the node itself is stopped.
        let forwarding = Arc::new(Forwarding::default());
        let closing = (node.clone(), forwarding.clone());
        manager.on_closing(Arc::new(move || {
            let (node, forwarding) = closing.clone();
            Box::pin(async move {
                forwarding.leaving.fetch_add(1, Ordering::SeqCst);
                // A serve that is under way is answered first: this `unserve`
                // then follows it, and an enable that has not told the node
                // yet sees the gateway leaving.
                let _serving = forwarding.serving.lock().await;
                if let Some(net) = node.get().and_then(Weak::upgrade) {
                    if !net.unserve().await {
                        net.stop().await;
                    }
                }
            })
        }));
        let identity = Settings::load(&dir).ok().and_then(|settings| settings.net);
        let notices = (ops.clone(), dir.clone(), gateway, node.clone());
        let net = NetManager::with_timing(
            dir.join("net"),
            version,
            identity,
            emit,
            Arc::new(move |notice| {
                let (ops, dir, gateway, node) = notices.clone();
                match notice {
                    // The gateway is only reachable through the node.
                    Notice::Exited { requested: false } => {
                        if let Some(manager) = gateway.get().and_then(Weak::upgrade) {
                            manager.fail("companion-net-crashed");
                        }
                    }
                    Notice::Exited { requested: true } => {}
                    // The node ran, or has to log in: settings keep what it last
                    // said. Whatever the mode, so a node that logged in during
                    // an enable that then failed is still remembered.
                    Notice::Running | Notice::LoginRequired => {
                        let running = matches!(notice, Notice::Running);
                        // A gateway behind a node whose key expired cannot be
                        // reached: it is halted, and its halt stops the node.
                        // An enable that has not started one fails by itself.
                        if !running {
                            if let Some(manager) = gateway.get().and_then(Weak::upgrade) {
                                manager.fail("companion-net-login-required");
                            }
                        }
                        tauri::async_runtime::spawn(async move {
                            let _turn = ops.lock().await;
                            let Some(net) = node.get().and_then(Weak::upgrade) else {
                                return;
                            };
                            if let Err(code) = remember_node(&dir, net.identity()) {
                                log_warning("companion_net_remember", &code);
                            }
                            // A login is done once the node runs; an enable
                            // that took the node over keeps it running.
                            if running && net.mode() == Some(net::Mode::Login) {
                                net.stop().await;
                            }
                        });
                    }
                }
            }),
            timing,
        );
        let _ = node.set(Arc::downgrade(&net));
        Self {
            manager,
            net,
            dir,
            resource_dir,
            ops,
            roots: Mutex::default(),
            forwarding,
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
    /// App exit, bounded: the listener closes first, so nothing forwards to a
    /// gateway that is leaving (a node that does not say it has is stopped),
    /// then the gateway, then the node. A process that outlives this one sees
    /// its stdin close and stops itself.
    pub fn shutdown(&self) {
        if self.owner.is_none() {
            return;
        }
        let (manager, net) = (self.manager.clone(), self.net.clone());
        let (done, wait) = std::sync::mpsc::sync_channel(1);
        tauri::async_runtime::spawn(async move {
            if !net.unserve().await {
                net.stop().await;
            }
            if !manager.idle() {
                manager.stop().await;
            }
            net.stop().await;
            let _ = done.send(());
        });
        let _ = wait.recv_timeout(Duration::from_secs(5));
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

/// The gateway's events, with one change: a status that says the gateway is
/// going or gone (disable, expiry, a halt) first tells the node, which goes
/// next, that it is leaving. Otherwise the overview still says the node runs
/// for an enable while the status already reads disabled.
fn leaving_first(emit: Emit, node: Arc<OnceLock<Weak<NetManager>>>) -> Emit {
    Arc::new(move |event| {
        let ends = event["type"] == "status"
            && matches!(
                event["status"]["state"].as_str(),
                Some("stopping" | "disabled" | "failed")
            );
        if ends {
            if let Some(net) = node.get().and_then(Weak::upgrade) {
                net.leaving();
            }
        }
        emit(event)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionOverview {
    status: Status,
    settings: Settings,
    /// `policy.enableUntil` while a gateway runs. For `untilExit` it is a
    /// far-off backstop the UI does not show.
    enable_until: Option<String>,
    /// False when another Yuzora instance owns the Companion.
    owner: bool,
    net: NetOverview,
    push: PushOverview,
}

#[tauri::command]
pub fn companion_status(
    state: tauri::State<'_, CompanionState>,
) -> Result<CompanionOverview, String> {
    overview(&state)
}

fn overview(state: &CompanionState) -> Result<CompanionOverview, String> {
    Ok(CompanionOverview {
        status: state.manager.status(),
        settings: Settings::load(&state.dir)?,
        enable_until: state.manager.enable_until(),
        owner: state.owner.is_some(),
        net: state.net.overview(),
        push: state.manager.push().overview(),
    })
}

/// The Experimental opt-in. Turning it off disables a running Companion the
/// same way `companion_disable` does.
#[tauri::command]
pub async fn companion_experimental_set(
    state: tauri::State<'_, CompanionState>,
    enabled: bool,
) -> Result<CompanionOverview, String> {
    state.owner()?;
    let _turn = state.ops.lock().await;
    let mut settings = Settings::load(&state.dir)?;
    settings.experimental = enabled;
    settings.save(&state.dir)?;
    if !enabled {
        disable(&state).await;
    }
    overview(&state)
}

/// Starts the node to log it in to a tailnet. When it runs, the node is
/// remembered and the sidecar stops: it stays registered, offline.
#[tauri::command]
pub async fn companion_net_login(
    state: tauri::State<'_, CompanionState>,
) -> Result<NetOverview, String> {
    state.owner()?;
    let _turn = state.ops.lock().await;
    net_login(&state, &net::program()?)
}

fn net_login(state: &CompanionState, program: &Path) -> Result<NetOverview, String> {
    if !state.manager.idle() {
        return Err("companion-busy".into());
    }
    // A failure the gateway ended with is not about this sign-in.
    state.manager.clear_failure();
    state.net.start(program, net::Mode::Login)?;
    Ok(state.net.overview())
}

/// Gives up a login in progress.
#[tauri::command]
pub async fn companion_net_cancel(
    state: tauri::State<'_, CompanionState>,
) -> Result<NetOverview, String> {
    state.owner()?;
    let _turn = state.ops.lock().await;
    net_cancel(&state).await
}

async fn net_cancel(state: &CompanionState) -> Result<NetOverview, String> {
    if state.net.mode() == Some(net::Mode::Login) {
        state.net.stop().await;
    }
    Ok(state.net.overview())
}

/// Logs the node out of the tailnet and forgets it.
#[tauri::command]
pub async fn companion_net_logout(
    state: tauri::State<'_, CompanionState>,
) -> Result<NetOverview, String> {
    state.owner()?;
    let _turn = state.ops.lock().await;
    net_logout(&state, net::program).await
}

/// `program` is only looked for when the node has to be asked to log out.
async fn net_logout(
    state: &CompanionState,
    program: impl FnOnce() -> Result<PathBuf, String> + Send,
) -> Result<NetOverview, String> {
    if !state.manager.idle() {
        return Err("companion-busy".into());
    }
    state.net.logout(program).await?;
    let mut settings = Settings::load(&state.dir)?;
    settings.net = None;
    settings.save(&state.dir)?;
    Ok(state.net.overview())
}

/// What enable runs and hands to the gateway, resolved before anything starts.
struct Inputs {
    net_program: PathBuf,
    gateway_program: PathBuf,
    herdr_binary: PathBuf,
    web_root: PathBuf,
}

/// Brings the node up, then starts the gateway behind it, and only once it
/// listens opens the node's listener in front of it: the node never forwards
/// to a port nothing has bound.
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
    // and the listener it is served behind.
    if !state.manager.idle() {
        return Err("companion-already-running".into());
    }
    enable(&state, &herdr, window).await
}

async fn enable(
    state: &CompanionState,
    herdr: &HerdrState,
    window: EnableFor,
) -> Result<Status, String> {
    let settings = Settings::load(&state.dir)?;
    if !settings.experimental {
        return Err("companion-experimental-off".into());
    }
    let manager = herdr.0.clone();
    let herdr_binary = tauri::async_runtime::spawn_blocking(move || manager.resolve_binary())
        .await
        .map_err(|e| e.to_string())?
        .ok_or("companion-herdr-unavailable")?;
    let inputs = Inputs {
        net_program: net::program()?,
        gateway_program: sidecar_program()?,
        herdr_binary,
        web_root: web_root(state.resource_dir.as_deref())?,
    };
    run_enable(state, &inputs, settings, window).await
}

/// Each step needs the one before it; when one fails, everything started is
/// stopped again.
async fn run_enable(
    state: &CompanionState,
    inputs: &Inputs,
    settings: Settings,
    window: EnableFor,
) -> Result<Status, String> {
    // The card reads a stale failure beside a running enable as no enable.
    state.manager.clear_failure();
    let started = bring_up(state, inputs, settings, window).await;
    if started.is_err() {
        disable(state).await;
    }
    started
}

async fn bring_up(
    state: &CompanionState,
    inputs: &Inputs,
    mut settings: Settings,
    window: EnableFor,
) -> Result<Status, String> {
    state.net.start(&inputs.net_program, net::Mode::Enable)?;
    let node = state.net.wait_running().await?;
    if !node.https_capable {
        return Err("companion-net-https-disabled".into());
    }
    state.net.wait_cert().await?;
    let until = chrono::Utc::now()
        + chrono::Duration::from_std(window.as_std()).map_err(|e| e.to_string())?;
    // Shared by the gateway and the node: the gateway only serves requests
    // that come through the node.
    let proxy_secret = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let roots = state.roots.lock().unwrap().clone();
    // The OS credential store may ask the user.
    let manager = state.manager.clone();
    let push = tauri::async_runtime::spawn_blocking(move || manager.push_init())
        .await
        .map_err(|e| e.to_string())?;
    let init = build_init(
        &settings,
        &GatewayInit {
            identity: &node.identity,
            herdr_binary: &inputs.herdr_binary,
            web_root: &inputs.web_root,
            enable_until: &until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            proxy_secret: &proxy_secret,
            space_roots: &roots,
            push: &push,
        },
    );
    settings.duration = window;
    settings.net = Some(node.identity.clone());
    settings.save(&state.dir)?;
    // Gateways that leave from here on are this one.
    let leaving_before = state.forwarding.leaving.load(Ordering::SeqCst);
    state.manager.start(Launch {
        program: inputs.gateway_program.clone(),
        init,
    })?;
    let port = state.manager.until_ready().await?;
    // Roots set since the init frame was built reach the running gateway.
    {
        let current = state.roots.lock().unwrap();
        if *current != roots {
            state.manager.notify(&roots_frame(&current));
        }
    }
    // The gateway may have been halted, or its policy may have expired, since
    // it was ready: its port is free or about to be, and nothing it does
    // waits for a node it was never told about. So the node is not told.
    // Checked under the lock a leaving gateway takes before it unserves: it
    // either finds the serve command sent, or this finds it leaving.
    let _serving = state.forwarding.serving.lock().await;
    let status = state.manager.status();
    let leaving = state.forwarding.leaving.load(Ordering::SeqCst) != leaving_before;
    if leaving || status != (Status::Running { port }) {
        return Err(match status {
            Status::Failed { code } => code,
            // An expiry leaves the status running until the gateway exits.
            _ if leaving => "policy-expired".into(),
            _ => "companion-stopped".into(),
        });
    }
    state
        .net
        .serve(
            &format!("http://127.0.0.1:{port}"),
            &proxy_secret,
            &node.identity.login,
        )
        .await?;
    // A gateway that ended by itself while the node was told to serve is not
    // an enabled Companion: an error makes the caller unserve and stop now,
    // and carries the failure code to the UI.
    match state.manager.status() {
        running @ Status::Running { .. } => Ok(running),
        Status::Failed { code } => Err(code),
        _ => Err("companion-stopped".into()),
    }
}

#[tauri::command]
pub async fn companion_disable(state: tauri::State<'_, CompanionState>) -> Result<(), String> {
    state.owner()?;
    // Taken first so an enable cannot start in between.
    let _turn = state.ops.lock().await;
    disable(&state).await;
    Ok(())
}

/// The listener first: nothing may forward to a gateway that is leaving, so a
/// node that does not say its listener is closed is stopped before the gateway
/// lets go of its port. Then the gateway, then the node. Callers hold `ops`.
async fn disable(state: &CompanionState) {
    if !state.net.unserve().await {
        state.net.stop().await;
    }
    state.manager.stop().await;
    state.net.stop().await;
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
    pairing_qr(&created, chrono::Utc::now())
}

/// The gateway's pairing lives five minutes; an hour is generous slack.
const PAIRING_MAX_AHEAD: i64 = 3600;

/// A reply that cannot be cancelled or timed is never shown as a QR: the
/// cancel id and the deadline come from the gateway or the pairing is invalid.
fn pairing_qr(created: &Value, now: chrono::DateTime<chrono::Utc>) -> Result<PairingQr, String> {
    let invalid = || "companion-invalid-reply".to_string();
    let url = created["url"].as_str().ok_or_else(invalid)?;
    let pairing_id = created["pairingId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let expires_at = created["expiresAt"]
        .as_str()
        .filter(|at| crate::companion_store::valid_until(at))
        .ok_or_else(invalid)?;
    let ahead = chrono::DateTime::parse_from_rfc3339(expires_at)
        .map_err(|_| invalid())?
        .signed_duration_since(now)
        .num_seconds();
    if !(1..=PAIRING_MAX_AHEAD).contains(&ahead) {
        return Err(invalid());
    }
    Ok(PairingQr {
        pairing_id: pairing_id.into(),
        qr_svg: qr_svg(url)?,
        expires_at: expires_at.into(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneLink {
    /// The address the gateway serves. Holds no secret: a phone still logs
    /// in with its passkey.
    url: String,
    qr_svg: String,
}

/// Where a phone that is already paired opens the Companion again, without
/// a new pairing.
#[tauri::command]
pub fn companion_phone_link(state: tauri::State<'_, CompanionState>) -> Result<PhoneLink, String> {
    let identity = Settings::load(&state.dir)?.net;
    match (state.manager.status(), identity) {
        (Status::Running { .. }, Some(identity)) => phone_link(&identity),
        _ => Err("companion-not-running".into()),
    }
}

fn phone_link(identity: &NetIdentity) -> Result<PhoneLink, String> {
    let url = format!("https://{}/", identity.dns_name.to_ascii_lowercase());
    Ok(PhoneLink {
        qr_svg: qr_svg(&url)?,
        url,
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

/// The phones the gateway has connected right now. The UI keeps events for
/// immediacy and corrects them with this.
#[tauri::command]
pub async fn companion_peers(
    state: tauri::State<'_, CompanionState>,
) -> Result<crate::companion_service::PeersSnapshot, String> {
    state.manager.peers().await
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

/// Where a HERDR Space lives on this machine, for the gateway's read-only Git.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceRoot {
    session: String,
    workspace_id: String,
    root: String,
}

/// The gateway's own limits (`companion/src/git_bridge.rs`): init refuses to
/// start on roots it rejects, so none may pass here that it would not accept.
fn valid_space_roots(roots: &[SpaceRoot]) -> Result<(), String> {
    let mut seen = HashSet::new();
    let valid = roots.len() <= 512
        && roots.iter().all(|entry| {
            (1..=256).contains(&entry.session.len())
                && (1..=256).contains(&entry.workspace_id.len())
                && (1..=4096).contains(&entry.root.len())
                && Path::new(&entry.root).is_absolute()
                && seen.insert((&entry.session, &entry.workspace_id))
        });
    if valid {
        Ok(())
    } else {
        Err("companion-space-roots-invalid".into())
    }
}

fn roots_frame(roots: &[SpaceRoot]) -> Value {
    json!({"v": 1, "type": "spaces.roots", "roots": roots})
}

/// The folder behind each HERDR Space, as the desktop sees it. Replaces the
/// whole mapping; the running gateway is told, and the next start gets it in
/// its init frame. Another instance's gateway is not ours to tell.
#[tauri::command]
pub fn companion_space_roots_set(
    state: tauri::State<'_, CompanionState>,
    roots: Vec<SpaceRoot>,
) -> Result<(), String> {
    space_roots_set(&state, roots)
}

fn space_roots_set(state: &CompanionState, roots: Vec<SpaceRoot>) -> Result<(), String> {
    if state.owner.is_none() {
        return Ok(());
    }
    valid_space_roots(&roots)?;
    let mut current = state.roots.lock().unwrap();
    state.manager.notify(&roots_frame(&roots));
    *current = roots;
    Ok(())
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

fn remember_node(dir: &Path, identity: Option<NetIdentity>) -> Result<(), String> {
    let mut settings = Settings::load(dir)?;
    if settings.net == identity {
        return Ok(());
    }
    settings.net = identity;
    settings.save(dir)
}

/// What the gateway's init frame holds besides the settings.
struct GatewayInit<'a> {
    identity: &'a NetIdentity,
    herdr_binary: &'a Path,
    web_root: &'a Path,
    enable_until: &'a str,
    /// Shared with the node, which presents it on every forwarded request.
    proxy_secret: &'a str,
    space_roots: &'a [SpaceRoot],
    /// `null`, or the VAPID key and the devices with a subscription.
    push: &'a Value,
}

fn build_init(settings: &Settings, init: &GatewayInit<'_>) -> Value {
    json!({
        "v": 1,
        "type": "init",
        "herdrBinary": init.herdr_binary,
        "sessionAllowlist": settings.session_allowlist,
        "webRoot": init.web_root,
        "trustStore": yuzora_host::workspace_trust::default_store_path(),
        "spaceRoots": init.space_roots,
        "push": init.push,
        "listeners": {
            "loopback": {
                "port": settings.gateway_port,
                // The node serves port 443, which the Origin leaves out.
                "publicHost": init.identity.dns_name.to_ascii_lowercase(),
                "requiredTailscaleLogin": init.identity.login,
                "proxySecret": init.proxy_secret,
            },
            "lan": null,
        },
        "policy": {
            "enableUntil": init.enable_until,
            "viewIdleMin": 30,
            "viewAbsoluteHours": 12,
            "elevationMaxMin": 15,
            "fullSessionEnabled": settings.full_session_enabled,
            "maxDevices": 5,
        },
    })
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
