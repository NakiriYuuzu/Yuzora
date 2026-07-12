// M3 Task 14: one-click managed install for the seven curated LSP servers.
//
// Three install routes (A9/A9' decision), keyed by the *active adapter* for a
// language, not the language itself (python's active adapter can be pyright=npm
// or pylsp=pip):
//   - binary x3 : rust-analyzer (.gz -> flate2), marksman / markdown-oxide (bare
//     binary) — official GitHub release asset -> ~/.yuzora/servers/ ; SHA256
//     recorded; unix chmod +x ; macOS quarantine removal.
//   - npm x3    : vtsls / pyright / typescript-language-server — `npm install
//     --prefix ~/.yuzora/servers/npm <pkg>` into a private prefix.
//   - pip x1    : pylsp — a private venv at ~/.yuzora/servers/pyenv + pip install.
//
// The download / subprocess execution never runs under `cargo test` (T15 does the
// live acceptance). Everything decidable without IO — route classification, asset
// URL assembly, SHA256 comparison, unpack routing, command + bin-path assembly,
// the missing-tool error branches, the in-flight guard, and the emitted-event
// terminal-state contract — is factored into pure functions and unit-tested here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::lsp_service::{LspProcessStatus, LspServerInfo};
use crate::{lsp_config, lsp_service};

mod plan;
// Preserve the historical `lsp_download::{BinaryServer, UnpackKind, InstallRoute}`
// paths; the execution layer below consumes the rest of the pure plan layer via
// `use plan::…`.
use plan::{
    asset_url, binary_command, binary_dest, binary_temp, build_plan, canonical_key, npm_bin_path,
    npm_install_args, npm_prefix, pip_install_args, quarantine_command, resolve_active, route_for,
    sha256_hex, sha256_matches, unpack_kind, venv_args, venv_bin_path, venv_dir, Plan,
};
pub use plan::{BinaryServer, InstallRoute, UnpackKind};

// ---- wire contract (camelCase; T5 `LspInstallProgress` depends on these keys) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallPhase {
    Download,
    Verify,
    Unpack,
    Npm,
    Pip,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspInstallProgress {
    pub language: String,
    pub phase: InstallPhase,
    pub percent: Option<u8>,
    pub message: Option<String>,
}

impl LspInstallProgress {
    fn new(
        language: &str,
        phase: InstallPhase,
        percent: Option<u8>,
        message: Option<&str>,
    ) -> Self {
        Self {
            language: language.to_string(),
            phase,
            percent,
            message: message.map(|m| m.to_string()),
        }
    }
}

// ---- per-language in-flight guard (pure over an injectable set) ----

fn try_reserve(set: &Mutex<HashSet<String>>, language: &str) -> bool {
    set.lock().unwrap().insert(language.to_string())
}

fn release(set: &Mutex<HashSet<String>>, language: &str) {
    set.lock().unwrap().remove(language);
}

struct InflightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    language: String,
}

impl<'a> InflightGuard<'a> {
    fn acquire(set: &'a Mutex<HashSet<String>>, language: &str) -> Option<Self> {
        if try_reserve(set, language) {
            Some(Self {
                set,
                language: language.to_string(),
            })
        } else {
            None
        }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        release(self.set, &self.language);
    }
}

// ---- terminal-state contract wrapper ----

/// Run `install`, then emit exactly one terminal phase last: `done` on Ok, `error`
/// on Err — and the return value mirrors it (Ok<->done, Err<->error). `install`
/// itself only emits non-terminal progress, so this structurally guarantees every
/// path ends with exactly one terminal event and an error phase implies an Err.
fn finalize(
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
    install: impl FnOnce() -> Result<LspServerInfo, String>,
) -> Result<LspServerInfo, String> {
    match install() {
        Ok(info) => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Done,
                Some(100),
                None,
            ));
            Ok(info)
        }
        Err(e) => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Error,
                None,
                Some(&e),
            ));
            Err(e)
        }
    }
}

// ---- execution (impure; not exercised by cargo test — T15 live acceptance) ----

fn servers_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".yuzora")
        .join("servers")
}

/// python3 (all platforms) then `python` (Windows only), resolved to an absolute
/// path via the shared `which` (mirrors lsp_service resolution order).
fn detect_python() -> Option<String> {
    lsp_service::which("python3").or_else(|| {
        if cfg!(windows) {
            lsp_service::which("python")
        } else {
            None
        }
    })
}

/// Pinned expected SHA256 for a release asset, when known. Empty today: no
/// upstream ships a stable machine-readable checksum manifest to pin against
/// without TOFU, so the verify phase records the computed digest and this table
/// is the hardening hook — fill a row and equality is enforced (see sha256_matches).
fn expected_sha256(_server: BinaryServer, _os: &str, _arch: &str) -> Option<&'static str> {
    None
}

// Subprocess timeout ceilings (M3F-2): a hung npm/pip/venv must not wedge the
// install thread forever — that would never drop the in-flight guard nor settle
// the frontend promise. Generous — these are hang guards, not perf targets.
const NPM_PIP_TIMEOUT_SECS: u64 = 600;
const VENV_TIMEOUT_SECS: u64 = 120;

/// Run a subprocess to completion, killing and reaping it if it outlives
/// `timeout` (M3F-2). Mirrors git_service::run_git's deadline poll+kill loop so a
/// stalled child can't block the install thread indefinitely.
fn run_command(program: &str, args: &[String], timeout: Duration) -> Result<(), String> {
    // Inherited stdio (spawn default) matches the previous `.status()` behavior, so
    // npm/pip output still reaches the app's stdout/stderr — and an inherited fd
    // never fills an unread pipe, so the poll loop can't deadlock.
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    crate::process_kill::configure_new_group(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("執行 {program} 失敗：{e}"))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child
            .try_wait()
            .map_err(|e| format!("等待 {program} 失敗：{e}"))?
        {
            Some(status) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("{program} 以非零狀態結束（{:?}）", status.code()))
                };
            }
            None if std::time::Instant::now() > deadline => {
                let _ = crate::process_kill::kill_tree(&mut child);
                return Err(format!("{program} 逾時（{timeout:?}）"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Hard caps for a managed download (F4): connect/read timeouts and a total-size
/// ceiling so a hung or runaway response can't stall a worker or exhaust memory.
const CONNECT_TIMEOUT_SECS: u64 = 30;
const READ_TIMEOUT_SECS: u64 = 60;
const MAX_DOWNLOAD_BYTES: u64 = 300 * 1024 * 1024;

/// Whether an accumulated / declared download size has exceeded the hard cap (F4).
fn download_too_large(len: u64, max: u64) -> bool {
    len > max
}

fn download(
    url: &str,
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let cap_err = || {
        format!(
            "下載超過大小上限（{} MB）",
            MAX_DOWNLOAD_BYTES / 1024 / 1024
        )
    };
    emit(LspInstallProgress::new(
        language,
        InstallPhase::Download,
        Some(0),
        Some("下載中"),
    ));
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(CONNECT_TIMEOUT_SECS)))
        .timeout_recv_response(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
        .timeout_recv_body(Some(Duration::from_secs(READ_TIMEOUT_SECS)))
        .build();
    let agent: ureq::Agent = config.into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("下載失敗（{url}）：{e}"))?;
    let total: Option<u64> = resp
        .headers()
        .get("Content-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .filter(|t| *t > 0);
    // Reject an over-cap download upfront when the length is declared.
    if let Some(t) = total {
        if download_too_large(t, MAX_DOWNLOAD_BYTES) {
            return Err(cap_err());
        }
    }
    let mut reader = resp.body_mut().as_reader();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut last_pct = 0u8;
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| format!("下載讀取失敗：{e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        // Streaming cap: a chunked / unknown-length response can't be pre-checked.
        if download_too_large(buf.len() as u64, MAX_DOWNLOAD_BYTES) {
            return Err(cap_err());
        }
        if let Some(t) = total {
            let pct = ((buf.len() as u64) * 100 / t).min(100) as u8;
            if pct != last_pct {
                last_pct = pct;
                emit(LspInstallProgress::new(
                    language,
                    InstallPhase::Download,
                    Some(pct),
                    None,
                ));
            }
        }
    }
    Ok(buf)
}

fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut d = flate2::read::GzDecoder::new(bytes);
    let mut out = Vec::new();
    d.read_to_end(&mut out)
        .map_err(|e| format!("解壓 .gz 失敗：{e}"))?;
    Ok(out)
}

/// Download + verify + unpack + install a binary server; returns its (command,
/// resolved absolute path) landing spot under the servers root.
fn install_binary(
    server: BinaryServer,
    base: &Path,
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<(String, PathBuf), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let url = asset_url(server, os, arch)?;
    let unpack = unpack_kind(server, os);
    // F5: short-circuit an unsupported unpack (Windows rust-analyzer .zip) BEFORE
    // downloading the whole asset.
    if unpack == UnpackKind::Zip {
        return Err(
            "Windows rust-analyzer 以 .zip 發佈，程式內 zip 解壓尚未支援；請改用手動安裝。"
                .to_string(),
        );
    }
    let bytes = download(&url, language, emit)?;

    let digest = sha256_hex(&bytes);
    if let Some(expected) = expected_sha256(server, os, arch) {
        if !sha256_matches(&bytes, expected) {
            return Err(format!(
                "{} SHA256 校驗失敗（預期 {expected}，實得 {digest}）",
                binary_command(server)
            ));
        }
    }
    emit(LspInstallProgress::new(
        language,
        InstallPhase::Verify,
        Some(100),
        Some(&format!("SHA256 {}…", &digest[..16])),
    ));

    let binary = match unpack {
        UnpackKind::Gz => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Unpack,
                None,
                Some("解壓 .gz"),
            ));
            gunzip(&bytes)?
        }
        UnpackKind::Bare => bytes,
        UnpackKind::Zip => unreachable!("zip is short-circuited before download"),
    };

    // F3: write to a sibling temp, set perms / clear quarantine on it, then rename
    // into place atomically — a running server keeps the old inode (no SIGBUS).
    std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
    let dest = binary_dest(base, server, cfg!(windows));
    let tmp = binary_temp(&dest);
    std::fs::write(&tmp, &binary).map_err(|e| format!("寫入 {} 失敗：{e}", tmp.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod +x 失敗：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        let (prog, args) = quarantine_command(tmp.to_string_lossy().as_ref());
        // Best-effort: the attribute is often absent (non-zero exit) — not fatal.
        let _ = std::process::Command::new(prog).args(&args).status();
    }
    std::fs::rename(&tmp, &dest).map_err(|e| format!("換位 {} 失敗：{e}", dest.display()))?;

    Ok((binary_command(server).to_string(), dest))
}

/// Execute a resolved plan; returns the installed server's (command, absolute
/// path) — the path lands where lsp_service::which resolves it (T4 order).
fn execute_plan(
    plan: Plan,
    base: &Path,
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<(String, PathBuf), String> {
    match plan {
        Plan::Binary(server) => install_binary(server, base, language, emit),
        Plan::Npm { npm, packages, bin } => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Npm,
                None,
                Some("npm install"),
            ));
            let prefix = npm_prefix(base);
            std::fs::create_dir_all(&prefix).map_err(|e| format!("建立 npm prefix 失敗：{e}"))?;
            run_command(
                &npm,
                &npm_install_args(&prefix, packages),
                Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
            )?;
            Ok((bin.to_string(), npm_bin_path(base, bin, cfg!(windows))))
        }
        Plan::Pip {
            python,
            package,
            bin,
        } => {
            std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Pip,
                None,
                Some("建立 venv"),
            ));
            run_command(
                &python,
                &venv_args(&venv_dir(base)),
                Duration::from_secs(VENV_TIMEOUT_SECS),
            )?;
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Pip,
                None,
                Some("pip install"),
            ));
            let pip = venv_bin_path(base, "pip", cfg!(windows));
            run_command(
                pip.to_string_lossy().as_ref(),
                &pip_install_args(package),
                Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
            )?;
            Ok((bin.to_string(), venv_bin_path(base, bin, cfg!(windows))))
        }
    }
}

/// Resolve the active adapter for a language, then run its install plan. Emits
/// only non-terminal progress; `finalize` owns the terminal done/error event.
fn do_install(
    workspace: Option<&str>,
    language: &str,
    base: &Path,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<LspServerInfo, String> {
    let cfg = lsp_config::load_from(&lsp_config::config_path());
    let ws_canonical = canonical_key(workspace);
    let server_id = resolve_active(&cfg, ws_canonical.as_deref(), language)
        .ok_or_else(|| format!("找不到 {language} 的 LSP adapter"))?;
    let route = route_for(language, &server_id)?;
    let npm = lsp_service::which("npm");
    let python = detect_python();
    let plan = build_plan(language, route, npm, python)?;
    let (command, path) = execute_plan(plan, base, language, emit)?;
    Ok(LspServerInfo {
        // F6: echo the raw workspace so LspBridge (which compares to the frontend's
        // raw workspacePath) receives the server-status emit; None -> empty, where
        // the returned value stays the primary channel via setServerInfo.
        workspace: workspace.map(str::to_string).unwrap_or_default(),
        language: language.to_string(),
        server_id,
        command,
        path: Some(path.to_string_lossy().into_owned()),
        status: LspProcessStatus::Stopped,
        last_startup_log: None,
        last_error: None,
        restart_count: 0,
    })
}

// ---- tauri command ----

static INFLIGHT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn inflight() -> &'static Mutex<HashSet<String>> {
    INFLIGHT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// T14 tenth handler: install the active managed server for `language`, honoring a
/// workspace override. The whole blocking chain (ureq download, npm/pip subprocesses)
/// runs on a blocking worker via `spawn_blocking` so it never stalls the async
/// runtime / other IPC (A2 — a sync `#[command]` would block Tauri's main thread).
/// Streams `lsp:install-progress`; on success emits `lsp:server-status`.
#[tauri::command]
pub async fn lsp_install_server(
    app: tauri::AppHandle,
    workspace: Option<String>,
    language: String,
) -> Result<LspServerInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        install_blocking(&app, workspace.as_deref(), &language)
    })
    .await
    .map_err(|e| format!("安裝背景執行緒異常：{e}"))?
}

/// The blocking install body (runs on the `spawn_blocking` worker). The in-flight
/// guard is acquired and dropped entirely here — never held across an await point.
fn install_blocking(
    app: &tauri::AppHandle,
    workspace: Option<&str>,
    language: &str,
) -> Result<LspServerInfo, String> {
    use tauri::Emitter;
    // A concurrent same-language install returns Err without emitting (an emit would
    // pollute the running install's progress stream, which the UI keys by language).
    let _guard = InflightGuard::acquire(inflight(), language)
        .ok_or_else(|| format!("{language} 的安裝正在進行中"))?;

    let app_emit = app.clone();
    let emit = move |p: LspInstallProgress| {
        let _ = app_emit.emit("lsp:install-progress", p);
    };
    let base = servers_dir();
    let info = finalize(language, &emit, || {
        do_install(workspace, language, &base, &emit)
    })?;
    // Best-effort refresh for Settings/StatusBar (the returned value is the primary
    // channel; LspBridge only receives this when the echoed workspace matches).
    let _ = app.emit("lsp:server-status", info.clone());
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn download_too_large_rejects_over_cap_only() {
        // W6A-F4: at-or-under the cap is allowed; strictly over is rejected.
        assert!(!download_too_large(0, 10));
        assert!(!download_too_large(10, 10));
        assert!(download_too_large(11, 10));
    }

    // ---- run_command exit / timeout (M3F-2; unix shell utilities) ----

    #[cfg(unix)]
    #[test]
    fn run_command_ok_on_success() {
        assert!(run_command("true", &[], Duration::from_secs(5)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_err_on_nonzero_exit() {
        assert!(run_command("false", &[], Duration::from_secs(5)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_times_out_and_kills() {
        // M3F-2: a hung child must be killed and surfaced as Err quickly, never
        // block the install thread for the child's full lifetime (mirrors
        // git_service::run_git_times_out_and_kills).
        let started = std::time::Instant::now();
        let r = run_command("sleep", &["30".to_string()], Duration::from_millis(300));
        assert!(r.is_err(), "a timed-out subprocess must return Err");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "must kill on timeout, not wait for the child to finish"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_timeout_kills_grandchild() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("grandchild.pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let started = std::time::Instant::now();
        let r = run_command(
            "sh",
            &["-c".to_string(), script],
            Duration::from_millis(300),
        );
        assert!(r.is_err(), "a timed-out subprocess must return Err");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "must kill on timeout, not wait for the grandchild to finish"
        );
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file exists")
            .trim()
            .parse()
            .expect("pid is numeric");
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("grandchild {pid} still exists after timeout");
    }

    // ---- in-flight guard ----

    #[test]
    fn try_reserve_blocks_duplicate_until_released() {
        let set = Mutex::new(HashSet::new());
        assert!(try_reserve(&set, "python"));
        assert!(
            !try_reserve(&set, "python"),
            "same language must be blocked"
        );
        assert!(
            try_reserve(&set, "rust"),
            "a different language is unaffected"
        );
        release(&set, "python");
        assert!(
            try_reserve(&set, "python"),
            "released language can re-reserve"
        );
    }

    #[test]
    fn inflight_guard_releases_on_drop() {
        let set = Mutex::new(HashSet::new());
        {
            let g = InflightGuard::acquire(&set, "python");
            assert!(g.is_some());
            assert!(
                InflightGuard::acquire(&set, "python").is_none(),
                "second acquire while held must fail"
            );
        }
        assert!(
            InflightGuard::acquire(&set, "python").is_some(),
            "guard drop must release the language"
        );
    }

    // ---- LspInstallProgress serde (camelCase + lowercase phase) ----

    #[test]
    fn install_progress_serializes_camel_case_round_trip() {
        let p = LspInstallProgress {
            language: "python".into(),
            phase: InstallPhase::Npm,
            percent: Some(42),
            message: Some("installing".into()),
        };
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        assert_eq!(v["language"], "python");
        assert_eq!(v["phase"], "npm");
        assert_eq!(v["percent"], 42);
        assert_eq!(v["message"], "installing");
        let back: LspInstallProgress = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);

        let q = LspInstallProgress {
            language: "rust".into(),
            phase: InstallPhase::Done,
            percent: None,
            message: None,
        };
        let vq: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert!(vq["percent"].is_null());
        assert!(vq["message"].is_null());
        assert_eq!(vq["phase"], "done");
    }

    #[test]
    fn install_phase_serializes_all_lowercase() {
        for (phase, s) in [
            (InstallPhase::Download, "download"),
            (InstallPhase::Verify, "verify"),
            (InstallPhase::Unpack, "unpack"),
            (InstallPhase::Npm, "npm"),
            (InstallPhase::Pip, "pip"),
            (InstallPhase::Done, "done"),
            (InstallPhase::Error, "error"),
        ] {
            assert_eq!(
                serde_json::to_value(phase).unwrap(),
                serde_json::Value::from(s)
            );
        }
    }

    // ---- terminal-state contract (injectable emit harness) ----

    fn capturing() -> (
        Arc<Mutex<Vec<LspInstallProgress>>>,
        impl Fn(LspInstallProgress),
    ) {
        let events: Arc<Mutex<Vec<LspInstallProgress>>> = Default::default();
        let sink = events.clone();
        (events, move |p| sink.lock().unwrap().push(p))
    }

    fn stub_info(language: &str) -> LspServerInfo {
        LspServerInfo {
            workspace: String::new(),
            language: language.into(),
            server_id: "x".into(),
            command: "x".into(),
            path: None,
            status: LspProcessStatus::Stopped,
            last_startup_log: None,
            last_error: None,
            restart_count: 0,
        }
    }

    fn terminal_count(events: &[LspInstallProgress]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e.phase, InstallPhase::Done | InstallPhase::Error))
            .count()
    }

    #[test]
    fn finalize_success_ends_with_exactly_one_done() {
        let (events, emit) = capturing();
        let out = finalize("python", &emit, || {
            emit(LspInstallProgress::new(
                "python",
                InstallPhase::Npm,
                Some(50),
                Some("installing"),
            ));
            Ok(stub_info("python"))
        });
        assert!(out.is_ok());
        let ev = events.lock().unwrap();
        assert_eq!(ev.last().unwrap().phase, InstallPhase::Done);
        assert_eq!(terminal_count(&ev), 1, "exactly one terminal phase");
        assert!(
            ev.iter().all(|e| e.phase != InstallPhase::Error),
            "success path must not emit error"
        );
    }

    #[test]
    fn finalize_failure_emits_one_error_and_returns_err() {
        let (events, emit) = capturing();
        let out = finalize("python", &emit, || {
            emit(LspInstallProgress::new(
                "python",
                InstallPhase::Download,
                None,
                None,
            ));
            Err::<LspServerInfo, String>("boom".into())
        });
        assert!(out.is_err(), "an emitted error must imply an Err return");
        let ev = events.lock().unwrap();
        assert_eq!(ev.last().unwrap().phase, InstallPhase::Error);
        assert_eq!(terminal_count(&ev), 1, "exactly one terminal phase");
        assert!(
            ev.iter().all(|e| e.phase != InstallPhase::Done),
            "failure path must not emit done"
        );
        assert_eq!(ev.last().unwrap().message.as_deref(), Some("boom"));
    }
}
