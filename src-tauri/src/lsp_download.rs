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
const MAX_DIAGNOSTIC_BYTES: usize = 4096;

type OutputReader = std::thread::JoinHandle<Vec<u8>>;

fn diagnostic_capture_limit() -> usize {
    let home_context = dirs::home_dir()
        .map(|home| home.to_string_lossy().len())
        .unwrap_or(0);
    MAX_DIAGNOSTIC_BYTES.saturating_add(home_context.saturating_add(1))
}

fn read_bounded_tail(mut reader: impl std::io::Read, max: usize) -> Vec<u8> {
    let mut tail = Vec::with_capacity(max);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if read >= max {
            tail.clear();
            tail.extend_from_slice(&chunk[read - max..read]);
            continue;
        }
        let overflow = tail.len().saturating_add(read).saturating_sub(max);
        if overflow > 0 {
            tail.drain(..overflow);
        }
        tail.extend_from_slice(&chunk[..read]);
    }
    tail
}

fn collect_command_output(
    stdout_reader: &mut Option<OutputReader>,
    stderr_reader: &mut Option<OutputReader>,
) -> (Vec<u8>, Vec<u8>) {
    let stdout = stdout_reader
        .take()
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr_reader
        .take()
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    (stdout, stderr)
}

fn output_readers_finished(
    stdout_reader: &Option<OutputReader>,
    stderr_reader: &Option<OutputReader>,
) -> bool {
    stdout_reader
        .as_ref()
        .is_none_or(std::thread::JoinHandle::is_finished)
        && stderr_reader
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
}

fn mask_truncated_url_userinfo_prefix(input: &str) -> String {
    let authority_end = input
        .find(|c: char| c == '/' || c == '?' || c == '#' || c.is_whitespace())
        .unwrap_or(input.len());
    let authority = &input[..authority_end];
    if authority.contains("://") {
        return input.to_string();
    }
    match authority.rfind('@') {
        Some(at) => format!("<redacted>{}", &input[at..]),
        None => input.to_string(),
    }
}

fn diagnostic_text_tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn bound_sanitized_diagnostic(text: String) -> String {
    let tail = diagnostic_text_tail(&text, MAX_DIAGNOSTIC_BYTES);
    let masked = mask_truncated_url_userinfo_prefix(tail);
    if masked.len() <= MAX_DIAGNOSTIC_BYTES {
        return masked;
    }
    if let Some(rest) = masked.strip_prefix("<redacted>") {
        let rest = diagnostic_text_tail(rest, MAX_DIAGNOSTIC_BYTES - "<redacted>".len());
        return format!("<redacted>{rest}");
    }
    diagnostic_text_tail(&masked, MAX_DIAGNOSTIC_BYTES).to_string()
}

fn sanitize_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut text = mask_truncated_url_userinfo_prefix(text.trim());
    text = crate::logging::mask_url_userinfo(&text);
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if !home.is_empty() {
            text = text.replace(home.as_ref(), "~");
            text = text.replace(&home.replace('\\', "/"), "~");
            text = text.replace(&home.replace('/', "\\"), "~");
        }
    }
    bound_sanitized_diagnostic(text)
}

fn command_error(
    stage: &str,
    program: &str,
    outcome: &str,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let tool = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    let mut message = format!("{stage} 失敗（工具 {tool}；{outcome}）");
    let stderr = sanitize_diagnostic(stderr);
    if !stderr.is_empty() {
        message.push_str("\nstderr（已去敏，末尾）：");
        message.push_str(&stderr);
    }
    let stdout = sanitize_diagnostic(stdout);
    if !stdout.is_empty() {
        message.push_str("\nstdout（已去敏，末尾）：");
        message.push_str(&stdout);
    }
    message
}

/// Run a subprocess to completion, killing and reaping it if it outlives
/// `timeout` (M3F-2). Mirrors git_service::run_git's deadline poll+kill loop so a
/// stalled child can't block the install thread indefinitely.
fn run_command(
    stage: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<(), String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::process_kill::configure_background_process(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|error| command_error(stage, program, &format!("無法啟動：{error}"), &[], &[]))?;
    let capture_limit = diagnostic_capture_limit();
    let mut stdout_reader = child
        .stdout
        .take()
        .map(|stdout| std::thread::spawn(move || read_bounded_tail(stdout, capture_limit)));
    let mut stderr_reader = child
        .stderr
        .take()
        .map(|stderr| std::thread::spawn(move || read_bounded_tail(stderr, capture_limit)));
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::time::Instant::now() > deadline {
            let _ = crate::process_kill::kill_tree(&mut child);
            let (stdout, stderr) = collect_command_output(&mut stdout_reader, &mut stderr_reader);
            return Err(command_error(
                stage,
                program,
                &format!("逾時 {timeout:?}"),
                &stdout,
                &stderr,
            ));
        }
        if !output_readers_finished(&stdout_reader, &stderr_reader) {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, stderr) =
                    collect_command_output(&mut stdout_reader, &mut stderr_reader);
                return if status.success() {
                    Ok(())
                } else {
                    Err(command_error(
                        stage,
                        program,
                        &format!(
                            "exit {}",
                            status
                                .code()
                                .map_or("unknown".to_string(), |c| c.to_string())
                        ),
                        &stdout,
                        &stderr,
                    ))
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = crate::process_kill::kill_tree(&mut child);
                let (stdout, stderr) =
                    collect_command_output(&mut stdout_reader, &mut stderr_reader);
                return Err(command_error(
                    stage,
                    program,
                    &format!("等待程序失敗：{error}"),
                    &stdout,
                    &stderr,
                ));
            }
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

fn unzip_binary(bytes: &[u8], expected_name: &str) -> Result<Vec<u8>, String> {
    unzip_binary_with_limit(bytes, expected_name, MAX_DOWNLOAD_BYTES)
}

fn unzip_binary_with_limit(
    bytes: &[u8],
    expected_name: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("開啟 ZIP 失敗：{e}"))?;
    let mut entry = archive
        .by_name(expected_name)
        .map_err(|_| format!("ZIP 缺少預期執行檔 {expected_name}"))?;
    if entry.is_dir() || entry.size() > max_bytes {
        return Err(format!("ZIP 內的 {expected_name} 無效或超過大小上限"));
    }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut out)
        .map_err(|e| format!("解壓 {expected_name} 失敗：{e}"))?;
    if out.is_empty() {
        return Err(format!("ZIP 內的 {expected_name} 是空檔案"));
    }
    Ok(out)
}

fn remove_path(path: &Path) -> Result<(), String> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("清理 {} 失敗：{e}", path.display())),
    }
}

/// Start from an empty managed directory and remove it again on any failure, so
/// an interrupted npm/pip install never poisons the next retry.
fn with_clean_dir<T>(
    target: &Path,
    install: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    remove_path(target)?;
    std::fs::create_dir_all(target).map_err(|e| format!("建立 {} 失敗：{e}", target.display()))?;
    match install(target) {
        Ok(value) => Ok(value),
        Err(error) => match remove_path(target) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}；{cleanup}")),
        },
    }
}

fn managed_sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut sibling = path.as_os_str().to_os_string();
    sibling.push(suffix);
    PathBuf::from(sibling)
}

/// Build a replacement directory away from the live target, then swap it in.
/// A failed build never touches the previous successful target; a failed swap
/// restores it before returning the error.
fn replace_managed_dir(
    target: &Path,
    build: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let staging = managed_sibling(target, ".installing");
    let previous = managed_sibling(target, ".previous");
    remove_path(&staging)?;
    remove_path(&previous)?;
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("建立 {} 失敗：{e}", staging.display()))?;

    if let Err(error) = build(&staging) {
        return match remove_path(&staging) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}；{cleanup}")),
        };
    }

    let had_previous = target.exists();
    if had_previous {
        if let Err(error) = std::fs::rename(target, &previous) {
            let _ = remove_path(&staging);
            return Err(format!("備份 {} 失敗：{error}", target.display()));
        }
    }
    if let Err(error) = std::fs::rename(&staging, target) {
        let rollback = if had_previous {
            std::fs::rename(&previous, target)
                .map_err(|e| format!("；還原 {} 失敗：{e}", target.display()))
        } else {
            Ok(())
        };
        let _ = remove_path(&staging);
        return Err(format!(
            "換位 {} 失敗：{error}{}",
            target.display(),
            rollback.err().unwrap_or_default()
        ));
    }
    if had_previous {
        remove_path(&previous)?;
    }
    Ok(())
}

fn npm_bin_in_prefix(prefix: &Path, bin: &str, windows: bool) -> PathBuf {
    let bin_dir = prefix.join("node_modules").join(".bin");
    if windows {
        bin_dir.join(format!("{bin}.cmd"))
    } else {
        bin_dir.join(bin)
    }
}

/// The npm prefix is shared by three curated adapters. Rebuild a clean staging
/// prefix with the requested package plus every already-usable curated adapter,
/// so a successful install preserves coexistence and a failed one leaves the
/// previous prefix untouched.
fn npm_transaction_packages(
    prefix: &Path,
    requested: &[&'static str],
    windows: bool,
) -> Vec<&'static str> {
    const CURATED: &[(&str, &[&str])] = &[
        ("vtsls", &["@vtsls/language-server"]),
        (
            "typescript-language-server",
            &["typescript-language-server", "typescript"],
        ),
        ("pyright-langserver", &["pyright"]),
    ];

    let mut packages = Vec::new();
    for (bin, existing_packages) in CURATED {
        if npm_bin_in_prefix(prefix, bin, windows).is_file() {
            for package in *existing_packages {
                if !packages.contains(package) {
                    packages.push(*package);
                }
            }
        }
    }
    for package in requested {
        if !packages.contains(package) {
            packages.push(*package);
        }
    }
    packages
}

static NPM_INSTALL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn npm_install_lock() -> &'static Mutex<()> {
    NPM_INSTALL_LOCK.get_or_init(|| Mutex::new(()))
}

fn require_installed_file(path: &Path, stage: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{stage} 完成但找不到預期執行檔 {}", path.display()))
    }
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
        UnpackKind::Zip => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Unpack,
                None,
                Some("解壓 ZIP"),
            ));
            unzip_binary(&bytes, &format!("{}.exe", binary_command(server)))?
        }
    };

    // F3: write to a sibling temp, set perms / clear quarantine on it, then rename
    // into place atomically — a running server keeps the old inode (no SIGBUS).
    std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
    let dest = binary_dest(base, server, cfg!(windows));
    let tmp = binary_temp(&dest);
    remove_path(&tmp)?;
    let install_result = (|| {
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
        remove_path(&dest)?;
        std::fs::rename(&tmp, &dest).map_err(|e| format!("換位 {} 失敗：{e}", dest.display()))?;
        require_installed_file(&dest, "binary install")
    })();
    if let Err(error) = install_result {
        let _ = remove_path(&tmp);
        let _ = remove_path(&dest);
        return Err(error);
    }

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
            let _npm_guard = npm_install_lock()
                .lock()
                .map_err(|_| "npm 安裝鎖已損毀".to_string())?;
            let transaction_packages = npm_transaction_packages(&prefix, packages, cfg!(windows));
            replace_managed_dir(&prefix, |staging| {
                run_command(
                    "npm install",
                    &npm,
                    &npm_install_args(staging, &transaction_packages),
                    Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
                )?;
                let installed = npm_bin_in_prefix(staging, bin, cfg!(windows));
                require_installed_file(&installed, "npm install")?;
                Ok(())
            })?;
            let installed = npm_bin_path(base, bin, cfg!(windows));
            require_installed_file(&installed, "npm install")?;
            Ok((bin.to_string(), installed))
        }
        Plan::Pip {
            python,
            package,
            bin,
        } => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Pip,
                None,
                Some("建立 venv"),
            ));
            std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
            let venv = venv_dir(base);
            with_clean_dir(&venv, |venv| {
                run_command(
                    "python venv",
                    &python,
                    &venv_args(venv),
                    Duration::from_secs(VENV_TIMEOUT_SECS),
                )?;
                emit(LspInstallProgress::new(
                    language,
                    InstallPhase::Pip,
                    None,
                    Some("pip install"),
                ));
                let pip = venv_bin_path(base, "pip", cfg!(windows));
                require_installed_file(&pip, "python venv")?;
                run_command(
                    "pip install",
                    pip.to_string_lossy().as_ref(),
                    &pip_install_args(package),
                    Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
                )?;
                let installed = venv_bin_path(base, bin, cfg!(windows));
                require_installed_file(&installed, "pip install")?;
                Ok((bin.to_string(), installed))
            })
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
    use std::io::Write;
    use std::sync::Arc;

    fn zip_fixture(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn windows_rust_analyzer_zip_extracts_only_expected_executable() {
        let archive = zip_fixture(&[
            ("README.txt", b"not the server"),
            ("rust-analyzer.exe", b"MZ-server"),
        ]);

        assert_eq!(
            unzip_binary(&archive, "rust-analyzer.exe").unwrap(),
            b"MZ-server"
        );
        assert!(unzip_binary(&archive, "missing.exe").is_err());
    }

    #[test]
    fn windows_rust_analyzer_zip_rejects_empty_and_oversized_executable() {
        let empty = zip_fixture(&[("rust-analyzer.exe", b"")]);
        assert!(unzip_binary(&empty, "rust-analyzer.exe").is_err());

        let oversized = zip_fixture(&[("rust-analyzer.exe", b"MZ-server")]);
        assert!(unzip_binary_with_limit(&oversized, "rust-analyzer.exe", 4).is_err());
    }

    #[test]
    fn failed_managed_target_is_removed_and_retry_starts_clean() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("npm");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("stale"), b"partial").unwrap();

        let first = with_clean_dir(&target, |dir| {
            assert!(
                !dir.join("stale").exists(),
                "retry must remove stale partial state"
            );
            std::fs::write(dir.join("half-installed"), b"partial").unwrap();
            Err::<(), String>("npm failed".into())
        });
        assert!(first.is_err());
        assert!(
            !target.exists(),
            "a failed install must leave no managed target"
        );

        let second = with_clean_dir(&target, |dir| {
            assert!(!dir.join("half-installed").exists());
            std::fs::write(dir.join("server.cmd"), b"ok").unwrap();
            Ok(())
        });
        assert!(second.is_ok());
        assert!(target.join("server.cmd").is_file());
    }

    #[test]
    fn npm_transaction_preserves_existing_curated_adapter() {
        let root = tempfile::tempdir().unwrap();
        let prefix = npm_prefix(root.path());
        let bin_dir = prefix.join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("vtsls"), b"existing").unwrap();

        let packages = npm_transaction_packages(&prefix, &["pyright"], false);
        assert!(packages.contains(&"@vtsls/language-server"));
        assert!(packages.contains(&"pyright"));

        replace_managed_dir(&prefix, |staging| {
            let staging_bin = staging.join("node_modules").join(".bin");
            std::fs::create_dir_all(&staging_bin).unwrap();
            std::fs::write(staging_bin.join("vtsls"), b"reinstalled").unwrap();
            std::fs::write(staging_bin.join("pyright-langserver"), b"installed").unwrap();
            Ok(())
        })
        .unwrap();

        assert!(prefix.join("node_modules/.bin/vtsls").is_file());
        assert!(prefix
            .join("node_modules/.bin/pyright-langserver")
            .is_file());
    }

    #[test]
    fn failed_npm_transaction_preserves_previous_success_and_removes_staging() {
        let root = tempfile::tempdir().unwrap();
        let prefix = npm_prefix(root.path());
        std::fs::create_dir_all(&prefix).unwrap();
        std::fs::write(prefix.join("previous-success"), b"ok").unwrap();

        let result = replace_managed_dir(&prefix, |staging| {
            std::fs::write(staging.join("partial"), b"bad").unwrap();
            Err("npm failed".to_string())
        });

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(prefix.join("previous-success")).unwrap(),
            b"ok"
        );
        assert!(!managed_sibling(&prefix, ".installing").exists());
        assert!(!managed_sibling(&prefix, ".previous").exists());
    }

    #[test]
    fn npm_install_lock_serializes_shared_prefix() {
        let _guard = npm_install_lock().lock().unwrap();
        let blocked = std::thread::spawn(|| npm_install_lock().try_lock().is_err())
            .join()
            .unwrap();
        assert!(blocked);
    }

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
        assert!(run_command("probe", "true", &[], Duration::from_secs(5)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_err_on_nonzero_exit() {
        assert!(run_command("probe", "false", &[], Duration::from_secs(5)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_failure_is_actionable_and_redacted() {
        let home = dirs::home_dir().unwrap();
        let script = format!(
            "printf '%s\\n' 'https://user:secret-token@example.com/pkg' >&2; printf '%s\\n' '{}' >&2; exit 7",
            home.display()
        );
        let error = run_command(
            "npm install",
            "/bin/sh",
            &["-c".to_string(), script],
            Duration::from_secs(5),
        )
        .unwrap_err();

        assert!(error.contains("npm install"));
        assert!(error.contains("sh"));
        assert!(error.contains('7'));
        assert!(error.contains("stderr"));
        assert!(error.contains("<redacted>"));
        assert!(!error.contains("secret-token"));
        assert!(!error.contains(home.to_string_lossy().as_ref()));
    }

    #[cfg(unix)]
    #[test]
    fn run_command_keeps_only_a_bounded_stderr_tail() {
        let output = format!(
            "prefix-marker{}tail-marker",
            "x".repeat(MAX_DIAGNOSTIC_BYTES + 512)
        );
        let script = format!("printf '%s' '{output}' >&2; exit 9");
        let error = run_command(
            "pip install",
            "/bin/sh",
            &["-c".to_string(), script],
            Duration::from_secs(5),
        )
        .unwrap_err();

        assert!(error.contains("tail-marker"));
        assert!(!error.contains("prefix-marker"));
        assert!(error.len() <= MAX_DIAGNOSTIC_BYTES + 512);
    }

    #[test]
    fn diagnostic_tail_redacts_url_userinfo_split_at_capture_boundary() {
        let retained = "user:secret-token@example.com/pkg";
        let capture_limit = diagnostic_capture_limit();
        let output = format!(
            "https://{retained}{}",
            "x".repeat(capture_limit - retained.len())
        );
        let raw_tail = read_bounded_tail(std::io::Cursor::new(output), capture_limit);
        let diagnostic = sanitize_diagnostic(&raw_tail);

        assert!(diagnostic.starts_with("<redacted>"));
        assert!(!diagnostic.contains("secret-token"));
        assert!(diagnostic.len() <= MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn diagnostic_tail_redacts_home_split_at_display_boundary() {
        let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        let midpoint = home.len() / 2;
        let split = (0..=midpoint)
            .rev()
            .find(|index| home.is_char_boundary(*index))
            .unwrap_or(0);
        let suffix = "x".repeat(MAX_DIAGNOSTIC_BYTES - (home.len() - split));
        let output = format!("{home}{suffix}");
        let raw_tail = read_bounded_tail(std::io::Cursor::new(output), diagnostic_capture_limit());
        let diagnostic = sanitize_diagnostic(&raw_tail);

        assert!(home.is_char_boundary(split));
        assert!(diagnostic.starts_with('~'));
        assert!(!diagnostic.contains(&home[split..]));
    }

    #[test]
    fn diagnostic_home_midpoint_uses_unicode_char_boundary() {
        let home = "/tmp/使用者";
        let midpoint = home.len() / 2;
        let split = (0..=midpoint)
            .rev()
            .find(|index| home.is_char_boundary(*index))
            .unwrap_or(0);

        assert_eq!(split, "/tmp/".len());
        assert!(home.is_char_boundary(split));
        assert_eq!(&home[split..], "使用者");
    }

    #[cfg(unix)]
    #[test]
    fn run_command_times_out_and_kills() {
        // M3F-2: a hung child must be killed and surfaced as Err quickly, never
        // block the install thread for the child's full lifetime (mirrors
        // git_service::run_git_times_out_and_kills).
        let started = std::time::Instant::now();
        let r = run_command(
            "timeout probe",
            "sleep",
            &["30".to_string()],
            Duration::from_millis(300),
        );
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
            "timeout tree probe",
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

    #[cfg(unix)]
    #[test]
    fn run_command_deadline_kills_descendant_after_parent_exits() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("orphan.pid");
        let script = format!("sleep 2 & echo $! > {}; exit 0", pid_file.display());
        let started = std::time::Instant::now();
        let result = run_command(
            "orphan pipe probe",
            "sh",
            &["-c".to_string(), script],
            Duration::from_millis(150),
        );

        assert!(
            result.is_err(),
            "inherited pipes must remain deadline-bound"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "reader collection must not wait for the descendant's pipe EOF"
        );
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file exists")
            .trim()
            .parse()
            .expect("pid is numeric");
        let alive = unsafe { libc::kill(pid as libc::pid_t, 0) == 0 };
        assert!(
            !alive,
            "descendant {pid} must be killed with the process group"
        );
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
