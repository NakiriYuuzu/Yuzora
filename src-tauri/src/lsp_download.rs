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
//   - pip x1    : pylsp route shape retained, but the embedded catalog currently
//                 marks it unavailable pending complete reviewed dependency manifests.
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

pub mod catalog;
mod plan;
// Preserve the historical `lsp_download::{BinaryServer, UnpackKind, InstallRoute}`
// paths; the execution layer below consumes the rest of the pure plan layer via
// `use plan::…`.
#[cfg(target_os = "macos")]
use plan::quarantine_command;
use plan::{
    asset_url, binary_command, binary_dest, binary_language_id, binary_temp, build_plan,
    canonical_key, npm_bin_in_prefix, npm_bin_path, npm_ci_args, npm_prefix, pip_install_args,
    resolve_active, route_for, unpack_kind, venv_args, venv_binary, venv_dir, Plan,
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

/// python3 (all platforms) then `python` (Windows only), resolved from the
/// trusted app-process PATH only — never from relative entries or the managed
/// servers prefix (which could shadow the launcher).
fn detect_python() -> Option<String> {
    lsp_service::which_toolchain("python3").or_else(|| {
        if cfg!(windows) {
            lsp_service::which_toolchain("python")
        } else {
            None
        }
    })
}

// Subprocess timeout ceilings (M3F-2): a hung npm/pip/venv must not wedge the
// install thread forever — that would never drop the in-flight guard nor settle
// the frontend promise. Generous — these are hang guards, not perf targets.
const NPM_PIP_TIMEOUT_SECS: u64 = 600;
const VENV_TIMEOUT_SECS: u64 = 120;
const MAX_DIAGNOSTIC_BYTES: usize = 4096;
const OUTPUT_COLLECTION_TIMEOUT: Duration = Duration::from_millis(250);
const OUTPUT_COLLECTION_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct OutputReader {
    tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    handle: std::thread::JoinHandle<()>,
}

impl OutputReader {
    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    fn collect_until(self, deadline: std::time::Instant) -> Vec<u8> {
        while !self.handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(OUTPUT_COLLECTION_POLL_INTERVAL);
        }
        if self.handle.is_finished() {
            let _ = self.handle.join();
        }
        snapshot_output_tail(&self.tail)
    }
}

fn diagnostic_capture_limit() -> usize {
    let home_context = dirs::home_dir()
        .map(|home| home.to_string_lossy().len())
        .unwrap_or(0);
    MAX_DIAGNOSTIC_BYTES.saturating_add(home_context.saturating_add(1))
}

fn append_bounded_tail(tail: &mut Vec<u8>, bytes: &[u8], max: usize) {
    if bytes.len() >= max {
        tail.clear();
        tail.extend_from_slice(&bytes[bytes.len() - max..]);
        return;
    }
    let overflow = tail.len().saturating_add(bytes.len()).saturating_sub(max);
    if overflow > 0 {
        tail.drain(..overflow);
    }
    tail.extend_from_slice(bytes);
}

#[cfg(test)]
fn read_bounded_tail(mut reader: impl std::io::Read, max: usize) -> Vec<u8> {
    let mut tail = Vec::with_capacity(max);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        append_bounded_tail(&mut tail, &chunk[..read], max);
    }
    tail
}

fn spawn_output_reader(
    mut reader: impl std::io::Read + Send + 'static,
    max: usize,
) -> OutputReader {
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(max)));
    let thread_tail = std::sync::Arc::clone(&tail);
    let handle = std::thread::spawn(move || {
        let mut chunk = [0_u8; 1024];
        loop {
            let read = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let mut tail = thread_tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            append_bounded_tail(&mut tail, &chunk[..read], max);
        }
    });
    OutputReader { tail, handle }
}

fn snapshot_output_tail(tail: &std::sync::Mutex<Vec<u8>>) -> Vec<u8> {
    tail.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn collect_command_output(
    stdout_reader: &mut Option<OutputReader>,
    stderr_reader: &mut Option<OutputReader>,
    timeout: Duration,
) -> (Vec<u8>, Vec<u8>) {
    let deadline = std::time::Instant::now() + timeout;
    let stdout = stdout_reader
        .take()
        .map(|reader| reader.collect_until(deadline))
        .unwrap_or_default();
    let stderr = stderr_reader
        .take()
        .map(|reader| reader.collect_until(deadline))
        .unwrap_or_default();
    (stdout, stderr)
}

fn output_readers_finished(
    stdout_reader: &Option<OutputReader>,
    stderr_reader: &Option<OutputReader>,
) -> bool {
    stdout_reader.as_ref().is_none_or(OutputReader::is_finished)
        && stderr_reader.as_ref().is_none_or(OutputReader::is_finished)
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

/// How managed install subprocesses should be launched.
/// Pure over `(program, windows)` so unit tests can assert Windows `.cmd`/`.bat`
/// shims route through ComSpec while `.exe` / Unix binaries stay direct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandLaunchKind {
    Direct,
    WindowsCmdShell,
}

fn command_launch_kind(program: &str, windows: bool) -> CommandLaunchKind {
    if !windows {
        return CommandLaunchKind::Direct;
    }
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        CommandLaunchKind::WindowsCmdShell
    } else {
        CommandLaunchKind::Direct
    }
}

fn build_managed_command(program: &str, args: &[String]) -> std::process::Command {
    match command_launch_kind(program, cfg!(windows)) {
        CommandLaunchKind::Direct => {
            let mut cmd = std::process::Command::new(program);
            cmd.args(args);
            cmd
        }
        CommandLaunchKind::WindowsCmdShell => {
            #[cfg(windows)]
            {
                let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
                crate::process_kill::windows_batch_command(shell.as_os_str(), program, args)
            }
            #[cfg(not(windows))]
            {
                // Unreachable when cfg!(windows) is false; kept for exhaustiveness.
                let mut cmd = std::process::Command::new(program);
                cmd.args(args);
                cmd
            }
        }
    }
}

/// Run a subprocess to completion, killing and reaping it if it outlives
/// `timeout` (M3F-2). Mirrors git_service::run_git's deadline poll+kill loop so a
/// stalled child can't block the install thread indefinitely.
fn launcher_is_absolute(program: &str) -> bool {
    Path::new(program).is_absolute()
}

fn restricted_path_for(launcher: &Path) -> std::ffi::OsString {
    let mut dirs = Vec::new();
    if let Some(parent) = launcher.parent() {
        dirs.push(parent.to_path_buf());
    }
    if let Some(node) = lsp_service::which_toolchain("node") {
        if let Some(parent) = Path::new(&node).parent() {
            dirs.push(parent.to_path_buf());
        }
    }
    #[cfg(unix)]
    {
        dirs.push(PathBuf::from("/usr/bin"));
        dirs.push(PathBuf::from("/bin"));
    }
    #[cfg(windows)]
    {
        if let Ok(root) = std::env::var("SYSTEMROOT") {
            dirs.push(PathBuf::from(root).join("System32"));
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn apply_restricted_install_env(cmd: &mut std::process::Command, launcher: &Path, cwd: &Path) {
    cmd.env_clear();
    cmd.current_dir(cwd);
    cmd.env("PATH", restricted_path_for(launcher));
    cmd.env("HOME", cwd);
    cmd.env("TMPDIR", cwd);
    cmd.env("TEMP", cwd);
    cmd.env("TMP", cwd);
    cmd.env("PYTHONNOUSERSITE", "1");
    cmd.env("PYTHONSAFEPATH", "1");
    cmd.env("PIP_REQUIRE_HASHES", "1");
    cmd.env("PIP_ONLY_BINARY", ":all:");
    cmd.env("NPM_CONFIG_IGNORE_SCRIPTS", "true");
    cmd.env("NPM_CONFIG_AUDIT", "false");
    cmd.env("NPM_CONFIG_FUND", "false");
    cmd.env("NPM_CONFIG_UPDATE_NOTIFIER", "false");
    for key in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        for key in [
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "WINDIR",
            "ComSpec",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "HOMEDRIVE",
            "HOMEPATH",
            "PATHEXT",
        ] {
            if let Ok(value) = std::env::var(key) {
                cmd.env(key, value);
            }
        }
    }
}

fn run_restricted_command(
    stage: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
    cwd: &Path,
) -> Result<(), String> {
    if !launcher_is_absolute(program) {
        return Err(format!("{stage} 拒絕相對路徑啟動器：{program}"));
    }
    run_command_inner(stage, program, args, timeout, Some(cwd))
}

#[cfg(all(test, unix))]
fn run_command(
    stage: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<(), String> {
    run_command_inner(stage, program, args, timeout, None)
}

fn run_command_inner(
    stage: &str,
    program: &str,
    args: &[String],
    timeout: Duration,
    restricted_cwd: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = build_managed_command(program, args);
    if let Some(cwd) = restricted_cwd {
        apply_restricted_install_env(&mut cmd, Path::new(program), cwd);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::process_kill::configure_background_process(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|error| command_error(stage, program, &format!("無法啟動：{error}"), &[], &[]))?;
    let capture_limit = diagnostic_capture_limit();
    let mut stdout_reader = child
        .stdout
        .take()
        .map(|stdout| spawn_output_reader(stdout, capture_limit));
    let mut stderr_reader = child
        .stderr
        .take()
        .map(|stderr| spawn_output_reader(stderr, capture_limit));
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::time::Instant::now() > deadline {
            let _ = crate::process_kill::kill_tree(&mut child);
            let (stdout, stderr) = collect_command_output(
                &mut stdout_reader,
                &mut stderr_reader,
                OUTPUT_COLLECTION_TIMEOUT,
            );
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
                let (stdout, stderr) = collect_command_output(
                    &mut stdout_reader,
                    &mut stderr_reader,
                    OUTPUT_COLLECTION_TIMEOUT,
                );
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
                let (stdout, stderr) = collect_command_output(
                    &mut stdout_reader,
                    &mut stderr_reader,
                    OUTPUT_COLLECTION_TIMEOUT,
                );
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

fn open_nonexecutable_temp(path: &Path) -> Result<std::fs::File, String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
        .map_err(|e| format!("建立 {} 失敗：{e}", path.display()))
}

fn download_to_file(
    url: &str,
    dest: &Path,
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<String, String> {
    use sha2::Digest;
    use std::io::{Read, Write};
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
    if let Some(t) = total {
        if download_too_large(t, MAX_DOWNLOAD_BYTES) {
            return Err(cap_err());
        }
    }
    let mut file = open_nonexecutable_temp(dest)?;
    let mut hasher = sha2::Sha256::new();
    let mut reader = resp.body_mut().as_reader();
    let mut chunk = [0u8; 64 * 1024];
    let mut written = 0u64;
    let mut last_pct = 0u8;
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| format!("下載讀取失敗：{e}"))?;
        if n == 0 {
            break;
        }
        written = written.saturating_add(n as u64);
        if download_too_large(written, MAX_DOWNLOAD_BYTES) {
            drop(file);
            let _ = remove_path(dest);
            return Err(cap_err());
        }
        file.write_all(&chunk[..n])
            .map_err(|e| format!("寫入 {} 失敗：{e}", dest.display()))?;
        hasher.update(&chunk[..n]);
        if let Some(t) = total {
            let pct = (written * 100 / t).min(100) as u8;
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
    file.flush()
        .map_err(|e| format!("寫入 {} 失敗：{e}", dest.display()))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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

fn managed_pip_launcher(venv: &Path, bin: &str, windows: bool) -> PathBuf {
    if windows {
        venv.join("Scripts").join(format!("{bin}.cmd"))
    } else {
        venv.join("bin").join(bin)
    }
}

fn remove_python_bytecode(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)
        .map_err(|e| format!("讀取 Python install tree {} 失敗：{e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("讀取 Python install entry 失敗：{e}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("讀取 Python install file type 失敗：{e}"))?;
        if file_type.is_dir() {
            if entry.file_name() == "__pycache__" {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| format!("清理 Python bytecode {} 失敗：{e}", path.display()))?;
            } else {
                remove_python_bytecode(&path)?;
            }
        } else if file_type.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("pyc")
        {
            std::fs::remove_file(&path)
                .map_err(|e| format!("清理 Python bytecode {} 失敗：{e}", path.display()))?;
        }
    }
    Ok(())
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
#[cfg(test)]
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

fn replace_managed_file(dest: &Path, prepared: &Path) -> Result<(), String> {
    if !dest.exists() {
        std::fs::rename(prepared, dest)
            .map_err(|e| format!("換位 {} 失敗：{e}", dest.display()))?;
        return Ok(());
    }
    let previous = managed_sibling(dest, ".previous");
    remove_path(&previous)?;
    if let Err(error) = std::fs::rename(dest, &previous) {
        return Err(format!("備份 {} 失敗：{error}", dest.display()));
    }
    if let Err(error) = std::fs::rename(prepared, dest) {
        let rollback = std::fs::rename(&previous, dest)
            .map_err(|e| format!("；還原 {} 失敗：{e}", dest.display()));
        return Err(format!(
            "換位 {} 失敗：{error}{}",
            dest.display(),
            rollback.err().unwrap_or_default()
        ));
    }
    let _ = remove_path(&previous);
    Ok(())
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

fn catalog_unpack(kind: &str) -> Result<UnpackKind, String> {
    match kind {
        "gz" => Ok(UnpackKind::Gz),
        "bare" => Ok(UnpackKind::Bare),
        "zip" => Ok(UnpackKind::Zip),
        other => Err(format!("不受支援的 unpack `{other}`")),
    }
}

fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod +x 失敗：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        let (prog, args) = quarantine_command(path.to_string_lossy().as_ref());
        let _ = std::process::Command::new(prog).args(&args).status();
    }
    let _ = path;
    Ok(())
}

fn unpack_verified_download(
    download_tmp: &Path,
    unpacked_tmp: &Path,
    unpack: UnpackKind,
    expected_name: &str,
    language: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<(), String> {
    let bytes = std::fs::read(download_tmp)
        .map_err(|e| format!("讀取 {} 失敗：{e}", download_tmp.display()))?;
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
            unzip_binary(&bytes, expected_name)?
        }
    };
    std::fs::write(unpacked_tmp, binary)
        .map_err(|e| format!("寫入 {} 失敗：{e}", unpacked_tmp.display()))
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
    let (catalog_language, server_id) = binary_language_id(server);
    let identity = catalog::require_binary(catalog_language, server_id, os, arch)?;
    let url = asset_url(server, os, arch)?;
    let unpack = catalog_unpack(&identity.unpack).unwrap_or_else(|_| unpack_kind(server, os));
    if url != identity.url {
        return Err(format!(
            "{server_id} catalog URL 與安裝計畫不一致，拒絕安裝"
        ));
    }

    std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
    let dest = binary_dest(base, server, cfg!(windows));
    let download_tmp = managed_sibling(&dest, ".download");
    let unpacked_tmp = binary_temp(&dest);
    remove_path(&download_tmp)?;
    remove_path(&unpacked_tmp)?;

    let install_result = (|| {
        let digest = download_to_file(&url, &download_tmp, language, emit)?;
        if !catalog::sha256_matches(&digest, &identity.sha256) {
            return Err(format!(
                "{server_id} SHA256 校驗失敗（預期 {}，實得 {digest}）",
                identity.sha256
            ));
        }
        emit(LspInstallProgress::new(
            language,
            InstallPhase::Verify,
            Some(100),
            Some(&format!("SHA256 {}…", &digest[..16.min(digest.len())])),
        ));
        unpack_verified_download(
            &download_tmp,
            &unpacked_tmp,
            unpack,
            &identity.executable,
            language,
            emit,
        )?;
        make_executable(&unpacked_tmp)?;
        replace_managed_file(&dest, &unpacked_tmp)?;
        require_installed_file(&dest, "binary install")?;
        let catalog = catalog::embedded_catalog()?;
        catalog::write_provenance(
            catalog,
            catalog_language,
            server_id,
            os,
            arch,
            &dest,
            &catalog::CatalogIdentity::Binary(identity),
        )?;
        Ok(())
    })();
    let _ = remove_path(&download_tmp);
    let _ = remove_path(&unpacked_tmp);
    install_result?;
    Ok((binary_command(server).to_string(), dest))
}

/// Execute a resolved plan; returns the installed server's (command, absolute
/// path) — the path lands where lsp_service::which resolves it (T4 order).
fn execute_plan(
    plan: Plan,
    base: &Path,
    language: &str,
    server_id: &str,
    emit: &dyn Fn(LspInstallProgress),
) -> Result<(String, PathBuf), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match plan {
        Plan::Binary(server) => install_binary(server, base, language, emit),
        Plan::Npm {
            npm,
            packages: _,
            bin,
        } => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Npm,
                None,
                Some("npm ci"),
            ));
            if !launcher_is_absolute(&npm) {
                return Err(format!("npm 啟動器必須是絕對路徑：{npm}"));
            }
            let identity = catalog::require_npm(language, server_id, os, arch)?;
            let prefix = npm_prefix(base, server_id);
            let _npm_guard = npm_install_lock()
                .lock()
                .map_err(|_| "npm 安裝鎖已損毀".to_string())?;
            replace_managed_dir(&prefix, |staging| {
                std::fs::write(staging.join("package.json"), &identity.package_json)
                    .map_err(|e| format!("寫入 package.json 失敗：{e}"))?;
                std::fs::write(staging.join("package-lock.json"), &identity.package_lock)
                    .map_err(|e| format!("寫入 package-lock.json 失敗：{e}"))?;
                let mut args = npm_ci_args(staging, identity.allow_scripts);
                args.extend([
                    "--cache".to_string(),
                    staging.join(".npm-cache").to_string_lossy().into_owned(),
                    "--userconfig".to_string(),
                    staging.join(".npmrc.user").to_string_lossy().into_owned(),
                    "--globalconfig".to_string(),
                    staging.join(".npmrc.global").to_string_lossy().into_owned(),
                ]);
                std::fs::write(staging.join(".npmrc.user"), "").ok();
                std::fs::write(staging.join(".npmrc.global"), "").ok();
                run_restricted_command(
                    "npm ci",
                    &npm,
                    &args,
                    Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
                    staging,
                )?;
                let _ = remove_path(&staging.join(".npm-cache"));
                let _ = remove_path(&staging.join(".npmrc.user"));
                let _ = remove_path(&staging.join(".npmrc.global"));
                let installed = npm_bin_in_prefix(staging, bin, cfg!(windows));
                require_installed_file(&installed, "npm ci")?;
                catalog::write_npm_launcher(&installed, &identity)?;
                catalog::verify_installed(
                    catalog::embedded_catalog()?,
                    language,
                    server_id,
                    &installed,
                    os,
                    arch,
                )?;
                Ok(())
            })?;
            let installed = npm_bin_path(base, server_id, bin, cfg!(windows));
            require_installed_file(&installed, "npm ci")?;
            catalog::write_provenance(
                catalog::embedded_catalog()?,
                language,
                server_id,
                os,
                arch,
                &installed,
                &catalog::CatalogIdentity::Npm(identity),
            )?;
            Ok((bin.to_string(), installed))
        }
        Plan::Pip {
            python,
            package: _,
            bin,
        } => {
            emit(LspInstallProgress::new(
                language,
                InstallPhase::Pip,
                None,
                Some("建立 venv"),
            ));
            if !launcher_is_absolute(&python) {
                return Err(format!("python 啟動器必須是絕對路徑：{python}"));
            }
            let identity = catalog::require_pip(language, server_id, os, arch)?;
            std::fs::create_dir_all(base).map_err(|e| format!("建立 servers 目錄失敗：{e}"))?;
            let venv = venv_dir(base);
            replace_managed_dir(&venv, |staging| {
                run_restricted_command(
                    "python venv",
                    &python,
                    &venv_args(staging),
                    Duration::from_secs(VENV_TIMEOUT_SECS),
                    base,
                )?;
                emit(LspInstallProgress::new(
                    language,
                    InstallPhase::Pip,
                    None,
                    Some("pip install"),
                ));
                let pip = venv_binary(staging, "pip", cfg!(windows));
                require_installed_file(&pip, "python venv")?;
                let requirements = staging.join("requirements.txt");
                std::fs::write(&requirements, &identity.requirements)
                    .map_err(|e| format!("寫入 requirements.txt 失敗：{e}"))?;
                run_restricted_command(
                    "pip install",
                    pip.to_string_lossy().as_ref(),
                    &pip_install_args(&requirements),
                    Duration::from_secs(NPM_PIP_TIMEOUT_SECS),
                    staging,
                )?;
                let generated = venv_binary(staging, bin, cfg!(windows));
                require_installed_file(&generated, "pip install")?;
                remove_python_bytecode(staging)?;
                let installed = managed_pip_launcher(staging, bin, cfg!(windows));
                catalog::write_pip_launcher(&installed, &identity)?;
                catalog::verify_installed(
                    catalog::embedded_catalog()?,
                    language,
                    server_id,
                    &installed,
                    os,
                    arch,
                )?;
                Ok(())
            })?;
            let installed = managed_pip_launcher(&venv, bin, cfg!(windows));
            require_installed_file(&installed, "pip install")?;
            catalog::write_provenance(
                catalog::embedded_catalog()?,
                language,
                server_id,
                os,
                arch,
                &installed,
                &catalog::CatalogIdentity::Pip(identity),
            )?;
            Ok((bin.to_string(), installed))
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
    let npm = lsp_service::which_toolchain("npm");
    let python = detect_python();
    let plan = build_plan(language, route, npm, python)?;
    let (command, path) = execute_plan(plan, base, language, &server_id, emit)?;
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

pub fn is_managed_route(language: &str, server_id: &str) -> bool {
    route_for(language, server_id).is_ok()
}

/// Re-verify a managed install before launch. Custom / unmanaged commands are
/// left untouched so workspace-trust semantics stay with the caller.
pub fn verify_managed_install(
    language: &str,
    server_id: &str,
    installed: &str,
) -> Result<(), String> {
    if !is_managed_route(language, server_id) {
        return Ok(());
    }
    catalog::verify_installed(
        catalog::embedded_catalog()?,
        language,
        server_id,
        Path::new(installed),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .map(|_| ())
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
    fn isolated_npm_prefix_does_not_clobber_sibling_server() {
        let root = tempfile::tempdir().unwrap();
        let vtsls = npm_prefix(root.path(), "vtsls");
        let pyright = npm_prefix(root.path(), "pyright");
        std::fs::create_dir_all(vtsls.join("node_modules/.bin")).unwrap();
        std::fs::write(vtsls.join("node_modules/.bin/vtsls"), b"existing").unwrap();

        replace_managed_dir(&pyright, |staging| {
            let staging_bin = staging.join("node_modules").join(".bin");
            std::fs::create_dir_all(&staging_bin).unwrap();
            std::fs::write(staging_bin.join("pyright-langserver"), b"installed").unwrap();
            Ok(())
        })
        .unwrap();

        assert_eq!(
            std::fs::read(vtsls.join("node_modules/.bin/vtsls")).unwrap(),
            b"existing"
        );
        assert!(pyright
            .join("node_modules/.bin/pyright-langserver")
            .is_file());
    }

    #[test]
    fn failed_npm_transaction_preserves_previous_success_and_removes_staging() {
        let root = tempfile::tempdir().unwrap();
        let prefix = npm_prefix(root.path(), "vtsls");
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
    fn digest_mismatch_removes_temp_and_preserves_existing_install() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("rust-analyzer");
        std::fs::write(&dest, b"good-install").unwrap();
        let download_tmp = managed_sibling(&dest, ".download");
        let unpacked_tmp = binary_temp(&dest);
        std::fs::write(&download_tmp, b"tampered-bytes").unwrap();
        let expected = catalog::sha256_hex(b"not-these-bytes");
        let actual = catalog::sha256_file(&download_tmp).unwrap();
        assert!(!catalog::sha256_matches(&actual, &expected));
        let _ = remove_path(&download_tmp);
        let _ = remove_path(&unpacked_tmp);
        assert_eq!(std::fs::read(&dest).unwrap(), b"good-install");
        assert!(!download_tmp.exists());
        assert!(!unpacked_tmp.exists());
    }

    #[test]
    fn replace_managed_file_failure_preserves_dest() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("marksman");
        std::fs::write(&dest, b"good").unwrap();
        let prepared = root.path().join("missing-prepared");
        let err = replace_managed_file(&dest, &prepared).unwrap_err();
        assert!(err.contains("換位") || err.contains("失敗"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"good");
    }

    #[cfg(unix)]
    #[test]
    fn download_temp_is_not_executable() {
        let root = tempfile::tempdir().unwrap();
        let dest = root.path().join("asset.download");
        let file = open_nonexecutable_temp(&dest).unwrap();
        drop(file);
        std::fs::write(&dest, b"bytes").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "download temp must stay non-executable");
    }

    #[test]
    fn missing_catalog_identity_never_launches() {
        let err = catalog::resolve_identity(
            catalog::embedded_catalog().unwrap(),
            "go",
            "gopls",
            "macos",
            "aarch64",
        )
        .unwrap_err();
        assert!(err.contains("缺少已審核"), "{err}");
        assert!(verify_managed_install("go", "gopls", "/tmp/gopls").is_ok());
    }

    #[test]
    fn unavailable_pylsp_catalog_route_fails_before_creating_or_running_venv() {
        let root = tempfile::tempdir().unwrap();
        let python = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let error = execute_plan(
            Plan::Pip {
                python,
                package: "python-lsp-server",
                bin: "pylsp",
            },
            root.path(),
            "python",
            "pylsp",
            &|_| {},
        )
        .unwrap_err();

        assert!(
            error.contains("complete reviewed dependency content manifests"),
            "{error}"
        );
        assert!(!venv_dir(root.path()).exists());
    }

    #[test]
    fn restricted_env_has_absolute_path_and_no_workspace_shadowing() {
        let cwd = PathBuf::from("/tmp/yuzora-staging");
        let launcher = PathBuf::from("/usr/local/bin/npm");
        let path = restricted_path_for(&launcher);
        let path = path.to_string_lossy();
        assert!(path.contains("/usr/local/bin") || path.contains("/usr/bin"));
        assert!(!path
            .split(':')
            .any(|part| part == "." || part.starts_with("./")));
        assert!(!path.contains("workspace"));
        let _ = cwd;
    }

    #[test]
    fn absolute_launcher_guard_rejects_relative_program() {
        let cwd = tempfile::tempdir().unwrap();
        let err =
            run_restricted_command("npm ci", "npm", &[], Duration::from_millis(10), cwd.path())
                .unwrap_err();
        assert!(err.contains("相對路徑"), "{err}");
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

    // ---- run_command launch plan / exit / timeout (M3F-2) ----

    #[test]
    fn windows_batch_shims_route_through_cmd_while_direct_executables_stay_direct() {
        assert_eq!(
            command_launch_kind(r"C:\Program Files\nodejs\npm.cmd", true),
            CommandLaunchKind::WindowsCmdShell
        );
        assert_eq!(
            command_launch_kind(r"C:\Tools\install.BAT", true),
            CommandLaunchKind::WindowsCmdShell
        );
        assert_eq!(
            command_launch_kind(r"C:\Program Files\nodejs\node.exe", true),
            CommandLaunchKind::Direct
        );
        assert_eq!(
            command_launch_kind(r"C:\Tools\npm.cmd", false),
            CommandLaunchKind::Direct
        );
    }

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
    fn bounded_output_collection_does_not_wait_for_descendant_pipe_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("pipe-holder.pid");
        let script = format!(
            "printf 'retained-output\\n'; sleep 30 & echo $! > {}; exit 0",
            pid_file.display()
        );
        let mut child = std::process::Command::new("sh")
            .args(["-c", &script])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let capture_limit = diagnostic_capture_limit();
        let mut stdout_reader = child
            .stdout
            .take()
            .map(|stdout| spawn_output_reader(stdout, capture_limit));
        let mut stderr_reader = child
            .stderr
            .take()
            .map(|stderr| spawn_output_reader(stderr, capture_limit));

        assert!(child.wait().unwrap().success(), "direct process must exit");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("descendant pid file exists")
            .trim()
            .parse()
            .expect("descendant pid is numeric");
        struct DescendantGuard(u32);
        impl Drop for DescendantGuard {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
                }
            }
        }
        let descendant = DescendantGuard(pid);
        assert!(
            !output_readers_finished(&stdout_reader, &stderr_reader),
            "descendant must still retain the inherited pipes"
        );

        let started = std::time::Instant::now();
        let (stdout, _stderr) = collect_command_output(
            &mut stdout_reader,
            &mut stderr_reader,
            Duration::from_millis(50),
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "output collection must stay bounded while pipe EOF is unavailable"
        );
        assert!(
            String::from_utf8_lossy(&stdout).contains("retained-output"),
            "already-read diagnostic output should be preserved"
        );

        unsafe {
            libc::kill(descendant.0 as libc::pid_t, libc::SIGKILL);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(descendant.0 as libc::pid_t, 0) == 0 }
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            unsafe { libc::kill(descendant.0 as libc::pid_t, 0) },
            0,
            "descendant must be gone after test cleanup"
        );
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

    #[test]
    #[ignore = "live network install smoke; does not touch ~/.yuzora"]
    fn live_install_smoke_binary_and_npm_in_isolated_home() {
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => unsafe {
                        std::env::set_var("HOME", value);
                    },
                    None => unsafe {
                        std::env::remove_var("HOME");
                    },
                }
            }
        }
        let home = tempfile::tempdir().expect("isolated HOME");
        let _restore = RestoreHome(std::env::var_os("HOME"));
        // SAFETY: ignored smoke is single-threaded; Drop restores HOME.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        let base = servers_dir();
        assert!(
            base.starts_with(home.path()),
            "install root must stay under isolated HOME, got {}",
            base.display()
        );
        let binary =
            do_install(None, "markdown", &base, &|_progress| {}).expect("marksman binary install");
        let npm = do_install(None, "python", &base, &|_progress| {}).expect("pyright npm install");
        assert_eq!(binary.server_id, "marksman");
        assert_eq!(npm.server_id, "pyright");
        let binary_path = PathBuf::from(binary.path.expect("marksman path"));
        let npm_path = PathBuf::from(npm.path.expect("pyright path"));
        assert!(binary_path.starts_with(&base), "{}", binary_path.display());
        assert!(npm_path.starts_with(&base), "{}", npm_path.display());
        assert!(binary_path.is_file(), "{}", binary_path.display());
        assert!(npm_path.is_file(), "{}", npm_path.display());
        verify_managed_install("markdown", "marksman", binary_path.to_str().unwrap())
            .expect("verify marksman provenance");
        verify_managed_install("python", "pyright", npm_path.to_str().unwrap())
            .expect("verify pyright provenance");
    }
}
