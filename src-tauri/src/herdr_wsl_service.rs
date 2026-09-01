//! Explicit Windows/WSL integration setup for the bundled Experimental Plugin.
//!
//! WSL remains a pane workload behind the Windows-native Herdr runtime. These
//! commands run only after an explicit Settings toggle; app startup stays
//! read-only and never registers the Plugin or mutates a distro home.

#[cfg(any(windows, test))]
use base64::{engine::general_purpose::STANDARD, Engine as _};
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(any(windows, test))]
use std::io::Read;
#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};
#[cfg(any(windows, test))]
use std::process::Child;
#[cfg(windows)]
use std::process::{Command, Stdio};
use std::sync::Mutex;
#[cfg(any(windows, test))]
use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
use crate::herdr_service::{HerdrManager, HerdrState};
#[cfg(any(windows, test))]
use crate::process_kill;

#[cfg(any(windows, test))]
const PLUGIN_ID: &str = "yuzora-wsl-agents";
#[cfg(any(windows, test))]
const PLUGIN_ROOT_RELATIVE: &str = "herdr-plugins/yuzora-wsl-agents";
#[cfg(any(windows, test))]
const HELPER_RELATIVE: &str = "scripts/manage-bundled-plugin.ps1";
#[cfg(any(windows, test))]
const ADAPTER_MANAGER_RELATIVE: &str = "scripts/manage-adapters.ps1";
#[cfg(any(windows, test))]
const PLUGIN_MANIFEST_RELATIVE: &str = "herdr-plugin.toml";
#[cfg(windows)]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(any(windows, test))]
const POWERSHELL_UTF8_PRELUDE: &str = r#"$utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8
$ProgressPreference = 'SilentlyContinue'
$ErrorActionPreference = 'Stop'"#;

static WSL_INTEGRATION_TRANSACTION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HerdrWslAdapterStatus {
    #[default]
    Unknown,
    Current,
    Absent,
    Drifted,
    Outdated,
    MissingPrerequisite,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWslIntegrationInfo {
    pub platform_supported: bool,
    pub bundle_available: bool,
    pub active: bool,
    pub linked: bool,
    pub enabled: bool,
    pub owns_registration: bool,
    pub adapter_status: HerdrWslAdapterStatus,
    pub plugin_version: Option<String>,
    pub bundled_path: Option<String>,
    pub linked_path: Option<String>,
    pub herdr_path: Option<String>,
    pub reason: Option<String>,
}

#[cfg(not(windows))]
impl HerdrWslIntegrationInfo {
    fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            platform_supported: false,
            bundle_available: false,
            active: false,
            linked: false,
            enabled: false,
            owns_registration: false,
            adapter_status: HerdrWslAdapterStatus::Unknown,
            plugin_version: None,
            bundled_path: None,
            linked_path: None,
            herdr_path: None,
            reason: Some(reason.into()),
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundledPluginStatus {
    plugin_id: String,
    bundled_path: String,
    herdr_path: String,
    linked: bool,
    enabled: bool,
    linked_path: Option<String>,
    owns_registration: bool,
    version: Option<String>,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdapterTargetScope {
    Configured,
    AllInstalled,
}

#[cfg(windows)]
struct ProcessOutput {
    stdout: String,
}

fn with_transaction_lock<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _guard = WSL_INTEGRATION_TRANSACTION_LOCK
        .lock()
        .map_err(|_| "WSL integration transaction lock is poisoned".to_string())?;
    operation()
}

#[cfg(any(windows, test))]
fn plugin_root(resource_dir: &Path) -> PathBuf {
    resource_dir.join(PLUGIN_ROOT_RELATIVE)
}

#[cfg(any(windows, test))]
fn required_bundle_paths(root: &Path) -> [PathBuf; 3] {
    [
        root.join(PLUGIN_MANIFEST_RELATIVE),
        root.join(HELPER_RELATIVE),
        root.join(ADAPTER_MANAGER_RELATIVE),
    ]
}

#[cfg(any(windows, test))]
fn bundle_is_available(root: &Path) -> bool {
    required_bundle_paths(root)
        .iter()
        .all(|path| path.is_file())
}

#[cfg(any(windows, test))]
fn parse_helper_status(stdout: &str) -> Result<BundledPluginStatus, String> {
    let line = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{') && line.ends_with('}'))
        .ok_or_else(|| "bundled WSL Plugin helper returned no JSON status".to_string())?;
    let status: BundledPluginStatus = serde_json::from_str(line)
        .map_err(|error| format!("bundled WSL Plugin helper returned invalid JSON: {error}"))?;
    if status.plugin_id != PLUGIN_ID {
        return Err(format!(
            "bundled WSL Plugin helper returned unexpected id {}",
            status.plugin_id
        ));
    }
    Ok(status)
}

#[cfg(any(windows, test))]
fn adapter_status_token(line: &str) -> Option<HerdrWslAdapterStatus> {
    let value = line
        .trim()
        .strip_prefix("installed ")
        .or_else(|| line.trim().strip_prefix("uninstalled "))
        .unwrap_or_else(|| line.trim());
    match value {
        "current" => Some(HerdrWslAdapterStatus::Current),
        "absent" => Some(HerdrWslAdapterStatus::Absent),
        "drifted" => Some(HerdrWslAdapterStatus::Drifted),
        "outdated" => Some(HerdrWslAdapterStatus::Outdated),
        "missing-prerequisite" => Some(HerdrWslAdapterStatus::MissingPrerequisite),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn parse_adapter_status(stdout: &str) -> HerdrWslAdapterStatus {
    let statuses = stdout
        .lines()
        .filter_map(adapter_status_token)
        .collect::<Vec<_>>();
    let Some(first) = statuses.first().copied() else {
        return HerdrWslAdapterStatus::Unknown;
    };
    if statuses.iter().all(|status| *status == first) {
        first
    } else {
        HerdrWslAdapterStatus::Mixed
    }
}

#[cfg(any(windows, test))]
fn integration_info_from_status(
    helper: BundledPluginStatus,
    adapter_status: HerdrWslAdapterStatus,
    reason: Option<String>,
) -> HerdrWslIntegrationInfo {
    let active = helper.linked
        && helper.enabled
        && helper.owns_registration
        && adapter_status == HerdrWslAdapterStatus::Current;
    HerdrWslIntegrationInfo {
        platform_supported: true,
        bundle_available: true,
        active,
        linked: helper.linked,
        enabled: helper.enabled,
        owns_registration: helper.owns_registration,
        adapter_status,
        plugin_version: helper.version,
        bundled_path: Some(helper.bundled_path),
        linked_path: helper.linked_path,
        herdr_path: Some(helper.herdr_path),
        reason,
    }
}

#[cfg(any(windows, test))]
fn decode_process_text(bytes: Vec<u8>, label: &str) -> Result<String, String> {
    let looks_utf16_le = bytes.starts_with(&[0xff, 0xfe])
        || bytes
            .iter()
            .take(64)
            .skip(1)
            .step_by(2)
            .any(|byte| *byte == 0);
    if looks_utf16_le {
        let body = if bytes.starts_with(&[0xff, 0xfe]) {
            &bytes[2..]
        } else {
            &bytes[..]
        };
        if !body.len().is_multiple_of(2) {
            return Err(format!("{label} returned malformed UTF-16LE output"));
        }
        let units = body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map_err(|_| format!("{label} returned invalid UTF-16LE output"));
    }
    String::from_utf8(bytes)
        .map(|text| text.trim_start_matches('\u{feff}').to_string())
        .map_err(|_| format!("{label} returned non-UTF-8 output"))
}

#[cfg(any(windows, test))]
fn powershell_utf8_script(body: &str) -> String {
    format!("{POWERSHELL_UTF8_PRELUDE}\n{body}")
}

#[cfg(any(windows, test))]
fn encode_powershell_command(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    STANDARD.encode(bytes)
}

#[cfg(windows)]
fn format_process_failure(
    program: &Path,
    status: std::process::ExitStatus,
    stderr: &str,
) -> String {
    let detail = stderr.trim();
    if detail.is_empty() {
        format!(
            "{} exited with code {}",
            program.display(),
            status.code().unwrap_or(-1)
        )
    } else {
        format!(
            "{} exited with code {}: {detail}",
            program.display(),
            status.code().unwrap_or(-1)
        )
    }
}

/// Capture helper/adapter output through temp files instead of pipes.
///
/// Windows PowerShell can leave a descendant or console host holding an
/// inherited stdout/stderr pipe after the direct child exits.
/// `wait_bounded_child` then joins reader threads that never see EOF, so
/// Settings `herdr_wsl_integration_get` hangs past `COMMAND_TIMEOUT`.
/// File-backed capture remains readable while a writer handle is still open.
#[cfg(any(windows, test))]
fn capture_len(capture: &tempfile::NamedTempFile, label: &str) -> Result<u64, String> {
    capture
        .as_file()
        .metadata()
        .map(|metadata| metadata.len())
        .map_err(|error| format!("failed to stat {label} capture: {error}"))
}

#[cfg(any(windows, test))]
fn oversized_capture_label(
    stdout_capture: &tempfile::NamedTempFile,
    stderr_capture: &tempfile::NamedTempFile,
    max_bytes: usize,
) -> Result<Option<&'static str>, String> {
    let max = max_bytes as u64;
    if capture_len(stdout_capture, "stdout")? > max {
        Ok(Some("stdout"))
    } else if capture_len(stderr_capture, "stderr")? > max {
        Ok(Some("stderr"))
    } else {
        Ok(None)
    }
}

#[cfg(any(windows, test))]
fn capture_exceeded_error(label: &str, max_bytes: usize) -> String {
    format!("{label} capture exceeded {max_bytes} bytes")
}

#[cfg(any(windows, test))]
fn append_oversized_context(base: String, oversized: Option<&str>, max_bytes: usize) -> String {
    match oversized {
        Some(label) => format!("{base}; {}", capture_exceeded_error(label, max_bytes)),
        None => base,
    }
}

#[cfg(any(windows, test))]
fn read_capped_capture(
    capture: &tempfile::NamedTempFile,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    let file = capture
        .reopen()
        .map_err(|error| format!("failed to reopen {label} capture: {error}"))?;
    let len = file
        .metadata()
        .map_err(|error| format!("failed to stat {label} capture: {error}"))?
        .len();
    if len > max_bytes as u64 {
        return Err(capture_exceeded_error(label, max_bytes));
    }
    let mut limited = file.take((max_bytes as u64).saturating_add(1));
    let mut bytes = Vec::with_capacity(max_bytes.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {label} capture: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(capture_exceeded_error(label, max_bytes));
    }
    Ok(bytes)
}

#[cfg(any(windows, test))]
fn wait_bounded_file_backed_child(
    child: &mut Child,
    process_tree: &mut process_kill::ProcessTreeGuard,
    timeout: Duration,
    stdout_capture: &tempfile::NamedTempFile,
    stderr_capture: &tempfile::NamedTempFile,
    max_bytes: usize,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), String> {
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut wait_error = None;
    let mut cleanup_error = None;
    let mut oversized = None;
    let mut size_error = None;
    loop {
        match oversized_capture_label(stdout_capture, stderr_capture, max_bytes) {
            Ok(Some(label)) => {
                oversized = Some(label);
                cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                break;
            }
            Err(error) => {
                size_error = Some(error);
                cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                break;
            }
            Ok(None) => match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    wait_error = Some(error);
                    cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                    break;
                }
            },
        }
    }
    let status = process_kill::reap_process_tree(child, process_tree);
    if let Some(error) = cleanup_error {
        return Err(append_oversized_context(
            format!("process-tree cleanup failed: {error}"),
            oversized,
            max_bytes,
        ));
    }
    let status = status.map_err(|error| {
        append_oversized_context(format!("reap failed: {error}"), oversized, max_bytes)
    })?;
    if let Some(error) = wait_error {
        return Err(append_oversized_context(
            format!("wait failed: {error}"),
            oversized,
            max_bytes,
        ));
    }
    if let Some(error) = size_error {
        return Err(error);
    }
    if let Some(label) = oversized {
        return Err(capture_exceeded_error(label, max_bytes));
    }
    let stdout = read_capped_capture(stdout_capture, max_bytes, "stdout")?;
    let stderr = read_capped_capture(stderr_capture, max_bytes, "stderr")?;
    if timed_out {
        return Err("timed out waiting for WSL integration command".into());
    }
    Ok((stdout, stderr, status))
}

#[cfg(windows)]
fn run_bounded_program(
    program: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
) -> Result<ProcessOutput, String> {
    let stdout_capture = tempfile::NamedTempFile::new().map_err(|error| {
        format!(
            "failed to create stdout capture for {}: {error}",
            program.display()
        )
    })?;
    let stderr_capture = tempfile::NamedTempFile::new().map_err(|error| {
        format!(
            "failed to create stderr capture for {}: {error}",
            program.display()
        )
    })?;
    let stdout = stdout_capture.as_file().try_clone().map_err(|error| {
        format!(
            "failed to clone stdout capture for {}: {error}",
            program.display()
        )
    })?;
    let stderr = stderr_capture.as_file().try_clone().map_err(|error| {
        format!(
            "failed to clone stderr capture for {}: {error}",
            program.display()
        )
    })?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    for (key, value) in envs {
        command.env(key, value);
    }
    command.env_remove("HERDR_PLUGIN_CONTEXT_JSON");
    process_kill::configure_background_process(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to launch {}: {error}", program.display()))?;
    let mut process_tree = process_kill::attach_process_tree(&mut child).map_err(|error| {
        format!(
            "process containment failed for {}: {error}",
            program.display()
        )
    })?;
    let (stdout, stderr, status) = wait_bounded_file_backed_child(
        &mut child,
        &mut process_tree,
        COMMAND_TIMEOUT,
        &stdout_capture,
        &stderr_capture,
        MAX_NDJSON_LINE_BYTES,
    )?;
    let stdout = decode_process_text(stdout, &format!("{} stdout", program.display()))?;
    let stderr = decode_process_text(stderr, &format!("{} stderr", program.display()))?;
    if !status.success() {
        return Err(format_process_failure(program, status, &stderr));
    }
    Ok(ProcessOutput { stdout })
}

#[cfg(windows)]
fn powershell_executable() -> PathBuf {
    let candidate = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if candidate.is_file() {
        candidate
    } else {
        PathBuf::from("powershell.exe")
    }
}

#[cfg(windows)]
fn run_powershell(script: &str, envs: &[(OsString, OsString)]) -> Result<ProcessOutput, String> {
    let args = [
        OsString::from("-NoLogo"),
        OsString::from("-NoProfile"),
        OsString::from("-NonInteractive"),
        OsString::from("-ExecutionPolicy"),
        OsString::from("Bypass"),
        OsString::from("-EncodedCommand"),
        OsString::from(encode_powershell_command(&powershell_utf8_script(script))),
    ];
    run_bounded_program(&powershell_executable(), &args, envs)
}

#[cfg(windows)]
fn run_helper(root: &Path, binary: &Path, action: &str) -> Result<BundledPluginStatus, String> {
    const SCRIPT: &str =
        "& $env:YUZORA_WSL_HELPER_PATH -Action $env:YUZORA_WSL_ACTION -HerdrPath $env:YUZORA_HERDR_PATH";
    let envs = [
        (
            OsString::from("YUZORA_WSL_HELPER_PATH"),
            root.join(HELPER_RELATIVE).into_os_string(),
        ),
        (OsString::from("YUZORA_WSL_ACTION"), OsString::from(action)),
        (
            OsString::from("YUZORA_HERDR_PATH"),
            binary.as_os_str().to_owned(),
        ),
    ];
    let output = run_powershell(SCRIPT, &envs)?;
    parse_helper_status(&output.stdout)
}

#[cfg(windows)]
fn plugin_config_dir(binary: &Path) -> Result<PathBuf, String> {
    let output = run_bounded_program(
        binary,
        &[
            OsString::from("plugin"),
            OsString::from("config-dir"),
            OsString::from(PLUGIN_ID),
        ],
        &[],
    )?;
    let path = output.stdout.trim();
    if path.is_empty() {
        return Err("Herdr returned an empty Plugin config directory".into());
    }
    Ok(PathBuf::from(path))
}

#[cfg(windows)]
fn run_adapter_action(
    root: &Path,
    binary: &Path,
    action: &str,
    scope: AdapterTargetScope,
) -> Result<HerdrWslAdapterStatus, String> {
    const CONFIGURED_SCRIPT: &str =
        "& $env:YUZORA_WSL_ADAPTER_MANAGER_PATH -Action $env:YUZORA_WSL_ACTION";
    const ALL_INSTALLED_SCRIPT: &str = "& $env:YUZORA_WSL_ADAPTER_MANAGER_PATH -Action $env:YUZORA_WSL_ACTION -AllInstalledDistros";
    let config_dir = plugin_config_dir(binary)?;
    let envs = [
        (
            OsString::from("YUZORA_WSL_ADAPTER_MANAGER_PATH"),
            root.join(ADAPTER_MANAGER_RELATIVE).into_os_string(),
        ),
        (OsString::from("YUZORA_WSL_ACTION"), OsString::from(action)),
        (
            OsString::from("HERDR_PLUGIN_ROOT"),
            root.as_os_str().to_owned(),
        ),
        (
            OsString::from("HERDR_PLUGIN_CONFIG_DIR"),
            config_dir.into_os_string(),
        ),
    ];
    let script = match scope {
        AdapterTargetScope::Configured => CONFIGURED_SCRIPT,
        AdapterTargetScope::AllInstalled => ALL_INSTALLED_SCRIPT,
    };
    let output = run_powershell(script, &envs)?;
    Ok(parse_adapter_status(&output.stdout))
}

#[cfg(windows)]
fn status_on_windows(manager: &HerdrManager) -> Result<HerdrWslIntegrationInfo, String> {
    let Some(resource_dir) = manager.resource_dir_path() else {
        return Ok(HerdrWslIntegrationInfo {
            platform_supported: true,
            bundle_available: false,
            active: false,
            linked: false,
            enabled: false,
            owns_registration: false,
            adapter_status: HerdrWslAdapterStatus::Unknown,
            plugin_version: None,
            bundled_path: None,
            linked_path: None,
            herdr_path: None,
            reason: Some("Yuzora resource directory is unavailable".into()),
        });
    };
    let root = plugin_root(&resource_dir);
    if !bundle_is_available(&root) {
        return Ok(HerdrWslIntegrationInfo {
            platform_supported: true,
            bundle_available: false,
            active: false,
            linked: false,
            enabled: false,
            owns_registration: false,
            adapter_status: HerdrWslAdapterStatus::Unknown,
            plugin_version: None,
            bundled_path: Some(root.to_string_lossy().into_owned()),
            linked_path: None,
            herdr_path: None,
            reason: Some("Bundled Yuzora WSL Agents Plugin is unavailable".into()),
        });
    }
    let Some(binary) = manager.resolve_binary() else {
        return Ok(HerdrWslIntegrationInfo {
            platform_supported: true,
            bundle_available: true,
            active: false,
            linked: false,
            enabled: false,
            owns_registration: false,
            adapter_status: HerdrWslAdapterStatus::Unknown,
            plugin_version: None,
            bundled_path: Some(root.to_string_lossy().into_owned()),
            linked_path: None,
            herdr_path: None,
            reason: Some("The active Herdr binary is unavailable".into()),
        });
    };
    let helper = run_helper(&root, &binary, "status")?;
    if helper.linked && !helper.owns_registration {
        return Ok(integration_info_from_status(
            helper,
            HerdrWslAdapterStatus::Unknown,
            Some(format!(
                "Plugin id {PLUGIN_ID} is registered from another root"
            )),
        ));
    }
    let scope = if helper.linked {
        AdapterTargetScope::Configured
    } else {
        AdapterTargetScope::AllInstalled
    };
    let (adapter_status, reason) = match run_adapter_action(&root, &binary, "status", scope) {
        Ok(status) => {
            let reason = if !helper.linked && status != HerdrWslAdapterStatus::Absent {
                Some(format!(
                    "Plugin is not linked, but owned adapter state is {status:?}; use manual recovery before enabling"
                ))
            } else {
                None
            };
            (status, reason)
        }
        Err(error) => (HerdrWslAdapterStatus::Unknown, Some(error)),
    };
    Ok(integration_info_from_status(helper, adapter_status, reason))
}

#[cfg(any(windows, test))]
trait IntegrationTransactionOps {
    fn status(&mut self) -> Result<HerdrWslIntegrationInfo, String>;
    fn link(&mut self) -> Result<(), String>;
    fn install_configured(&mut self) -> Result<HerdrWslAdapterStatus, String>;
    fn uninstall_all(&mut self) -> Result<HerdrWslAdapterStatus, String>;
    fn unlink(&mut self) -> Result<(), String>;
}

#[cfg(any(windows, test))]
fn rollback_new_enable(
    ops: &mut impl IntegrationTransactionOps,
    primary_error: String,
) -> Result<HerdrWslIntegrationInfo, String> {
    match ops.uninstall_all() {
        Ok(HerdrWslAdapterStatus::Absent) => match ops.unlink() {
            Ok(()) => Err(format!("{primary_error}; new Plugin registration was rolled back")),
            Err(error) => Err(format!(
                "{primary_error}; adapter rollback reached absent, but Plugin unlink failed: {error}"
            )),
        },
        Ok(status) => Err(format!(
            "{primary_error}; adapter rollback ended in {status:?}, so the new Plugin registration remains linked for recovery"
        )),
        Err(error) => Err(format!(
            "{primary_error}; adapter rollback failed: {error}; the new Plugin registration remains linked for recovery"
        )),
    }
}

#[cfg(any(windows, test))]
fn set_integration_transaction(
    ops: &mut impl IntegrationTransactionOps,
    before: HerdrWslIntegrationInfo,
    requested_enabled: bool,
) -> Result<HerdrWslIntegrationInfo, String> {
    if !before.bundle_available {
        return Err(before
            .reason
            .unwrap_or_else(|| "Bundled Yuzora WSL Agents Plugin is unavailable".into()));
    }
    if before.linked && !before.owns_registration {
        return Err(format!(
            "refusing to change Plugin id {PLUGIN_ID} because it is registered from another root"
        ));
    }

    if requested_enabled {
        if before.active {
            return Ok(before);
        }
        if before.linked {
            return Err(
                "An owned Plugin registration already exists but is not fully active; automatic repair is refused to preserve its prior state"
                    .into(),
            );
        }
        if before.adapter_status != HerdrWslAdapterStatus::Absent {
            return Err(format!(
                "WSL adapter preflight is {:?}; automatic enable requires absent across all installed distros",
                before.adapter_status
            ));
        }

        if let Err(error) = ops.link() {
            return rollback_new_enable(ops, format!("WSL Plugin link failed: {error}"));
        }
        match ops.install_configured() {
            Ok(HerdrWslAdapterStatus::Current) => {}
            Ok(status) => {
                return rollback_new_enable(
                    ops,
                    format!("WSL Pi adapter install ended in {status:?}"),
                );
            }
            Err(error) => {
                return rollback_new_enable(ops, format!("WSL Pi adapter install failed: {error}"));
            }
        }
        match ops.status() {
            Ok(after) if after.active => Ok(after),
            Ok(after) => rollback_new_enable(
                ops,
                after.reason.unwrap_or_else(|| {
                    format!(
                        "WSL integration did not become active (adapter={:?})",
                        after.adapter_status
                    )
                }),
            ),
            Err(error) => {
                rollback_new_enable(ops, format!("WSL integration verification failed: {error}"))
            }
        }
    } else {
        if !before.linked && before.adapter_status == HerdrWslAdapterStatus::Absent {
            return Ok(before);
        }
        let adapter_status = ops.uninstall_all()?;
        if adapter_status != HerdrWslAdapterStatus::Absent {
            return Err(format!(
                "WSL Pi adapter uninstall ended in {adapter_status:?}; Plugin remains linked"
            ));
        }
        if before.linked {
            ops.unlink()?;
        }
        let mut after = ops.status()?;
        after.adapter_status = HerdrWslAdapterStatus::Absent;
        after.active = false;
        Ok(after)
    }
}

#[cfg(windows)]
struct WindowsIntegrationOps<'a> {
    manager: &'a HerdrManager,
    root: PathBuf,
    binary: PathBuf,
}

#[cfg(windows)]
impl IntegrationTransactionOps for WindowsIntegrationOps<'_> {
    fn status(&mut self) -> Result<HerdrWslIntegrationInfo, String> {
        status_on_windows(self.manager)
    }

    fn link(&mut self) -> Result<(), String> {
        run_helper(&self.root, &self.binary, "link").map(|_| ())
    }

    fn install_configured(&mut self) -> Result<HerdrWslAdapterStatus, String> {
        run_adapter_action(
            &self.root,
            &self.binary,
            "install",
            AdapterTargetScope::Configured,
        )
    }

    fn uninstall_all(&mut self) -> Result<HerdrWslAdapterStatus, String> {
        run_adapter_action(
            &self.root,
            &self.binary,
            "uninstall",
            AdapterTargetScope::AllInstalled,
        )
    }

    fn unlink(&mut self) -> Result<(), String> {
        run_helper(&self.root, &self.binary, "unlink").map(|_| ())
    }
}

#[cfg(not(windows))]
fn integration_status_unlocked(_manager: &HerdrManager) -> Result<HerdrWslIntegrationInfo, String> {
    Ok(HerdrWslIntegrationInfo::unsupported(
        "WSL Plugin integration is available only on Windows",
    ))
}

#[cfg(windows)]
fn integration_status_unlocked(manager: &HerdrManager) -> Result<HerdrWslIntegrationInfo, String> {
    status_on_windows(manager)
}

#[cfg(not(windows))]
fn set_integration_unlocked(
    _manager: &HerdrManager,
    _enabled: bool,
) -> Result<HerdrWslIntegrationInfo, String> {
    Err("WSL Plugin integration is available only on Windows".into())
}

#[cfg(windows)]
fn set_integration_unlocked(
    manager: &HerdrManager,
    requested_enabled: bool,
) -> Result<HerdrWslIntegrationInfo, String> {
    let before = status_on_windows(manager)?;
    let root = before
        .bundled_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| "Bundled Yuzora WSL Agents Plugin path is unavailable".to_string())?;
    let binary = before
        .herdr_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| manager.resolve_binary())
        .ok_or_else(|| "The active Herdr binary is unavailable".to_string())?;
    let mut ops = WindowsIntegrationOps {
        manager,
        root,
        binary,
    };
    set_integration_transaction(&mut ops, before, requested_enabled)
}

fn integration_status(manager: &HerdrManager) -> Result<HerdrWslIntegrationInfo, String> {
    with_transaction_lock(|| integration_status_unlocked(manager))
}

fn set_integration(
    manager: &HerdrManager,
    enabled: bool,
) -> Result<HerdrWslIntegrationInfo, String> {
    with_transaction_lock(|| set_integration_unlocked(manager, enabled))
}

#[tauri::command]
pub async fn herdr_wsl_integration_get(
    state: tauri::State<'_, HerdrState>,
) -> Result<HerdrWslIntegrationInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || integration_status(&manager))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn herdr_wsl_integration_set(
    state: tauri::State<'_, HerdrState>,
    enabled: bool,
) -> Result<HerdrWslIntegrationInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || set_integration(&manager, enabled))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn integration_fixture(
        linked: bool,
        enabled: bool,
        owns_registration: bool,
        adapter_status: HerdrWslAdapterStatus,
    ) -> HerdrWslIntegrationInfo {
        HerdrWslIntegrationInfo {
            platform_supported: true,
            bundle_available: true,
            active: linked
                && enabled
                && owns_registration
                && adapter_status == HerdrWslAdapterStatus::Current,
            linked,
            enabled,
            owns_registration,
            adapter_status,
            plugin_version: Some("0.1.0".into()),
            bundled_path: Some("bundle".into()),
            linked_path: linked.then(|| "bundle".into()),
            herdr_path: Some("herdr".into()),
            reason: None,
        }
    }

    struct FakeOps {
        calls: Vec<&'static str>,
        status_result: Result<HerdrWslIntegrationInfo, String>,
        install_result: Result<HerdrWslAdapterStatus, String>,
        uninstall_result: Result<HerdrWslAdapterStatus, String>,
        link_result: Result<(), String>,
        unlink_result: Result<(), String>,
    }

    impl FakeOps {
        fn successful() -> Self {
            Self {
                calls: Vec::new(),
                status_result: Ok(integration_fixture(
                    true,
                    true,
                    true,
                    HerdrWslAdapterStatus::Current,
                )),
                install_result: Ok(HerdrWslAdapterStatus::Current),
                uninstall_result: Ok(HerdrWslAdapterStatus::Absent),
                link_result: Ok(()),
                unlink_result: Ok(()),
            }
        }
    }

    impl IntegrationTransactionOps for FakeOps {
        fn status(&mut self) -> Result<HerdrWslIntegrationInfo, String> {
            self.calls.push("status");
            self.status_result.clone()
        }

        fn link(&mut self) -> Result<(), String> {
            self.calls.push("link");
            self.link_result.clone()
        }

        fn install_configured(&mut self) -> Result<HerdrWslAdapterStatus, String> {
            self.calls.push("install-configured");
            self.install_result.clone()
        }

        fn uninstall_all(&mut self) -> Result<HerdrWslAdapterStatus, String> {
            self.calls.push("uninstall-all");
            self.uninstall_result.clone()
        }

        fn unlink(&mut self) -> Result<(), String> {
            self.calls.push("unlink");
            self.unlink_result.clone()
        }
    }

    #[test]
    fn parses_owned_helper_status_from_the_last_json_line() {
        let status = parse_helper_status(
            "diagnostic\n{\"action\":\"status\",\"pluginId\":\"yuzora-wsl-agents\",\"bundledPath\":\"C:\\\\Yuzora\\\\plugin\",\"herdrPath\":\"C:\\\\Yuzora\\\\herdr.exe\",\"linked\":true,\"enabled\":true,\"linkedPath\":\"C:\\\\Yuzora\\\\plugin\",\"ownsRegistration\":true,\"version\":\"0.1.0\"}\n",
        )
        .unwrap();
        assert!(status.linked);
        assert!(status.enabled);
        assert!(status.owns_registration);
        assert_eq!(status.version.as_deref(), Some("0.1.0"));
        assert_eq!(status.bundled_path, r"C:\Yuzora\plugin");
        assert_eq!(status.herdr_path, r"C:\Yuzora\herdr.exe");
        assert_eq!(status.linked_path.as_deref(), Some(r"C:\Yuzora\plugin"));
    }

    #[test]
    fn rejects_a_helper_status_for_another_plugin() {
        let error = parse_helper_status(
            "{\"pluginId\":\"other\",\"bundledPath\":\"x\",\"herdrPath\":\"h\",\"linked\":false,\"enabled\":false,\"linkedPath\":null,\"ownsRegistration\":false,\"version\":null}",
        )
        .unwrap_err();
        assert!(error.contains("unexpected id"), "{error}");
    }

    #[test]
    fn full_activation_requires_owned_enabled_registration_and_current_adapter() {
        let helper = parse_helper_status(
            "{\"pluginId\":\"yuzora-wsl-agents\",\"bundledPath\":\"bundle\",\"herdrPath\":\"herdr\",\"linked\":true,\"enabled\":true,\"linkedPath\":\"bundle\",\"ownsRegistration\":true,\"version\":\"0.1.0\"}",
        )
        .unwrap();
        assert!(integration_info_from_status(helper, HerdrWslAdapterStatus::Current, None).active);
    }

    #[test]
    fn decodes_utf8_bom_and_utf16le_powershell_output() {
        assert_eq!(
            decode_process_text(b"\xef\xbb\xbf{\"ok\":true}".to_vec(), "stdout").unwrap(),
            "{\"ok\":true}"
        );
        let text = "測試 current\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_process_text(bytes, "stdout").unwrap(), text);
    }

    #[test]
    fn powershell_encoded_command_round_trips_utf16_with_non_ascii_text() {
        let body = "$value = 'C:\\\\使用者\\\\Yuzora'; Write-Output $value";
        let script = powershell_utf8_script(body);
        let bytes = STANDARD.decode(encode_powershell_command(&script)).unwrap();
        let units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        let decoded = String::from_utf16(&units).unwrap();
        assert_eq!(decoded, script);
        assert!(decoded.contains("[Console]::OutputEncoding = $utf8"));
        assert!(decoded.contains("$OutputEncoding = $utf8"));
        assert!(decoded.contains(body));
    }

    #[cfg(windows)]
    #[test]
    fn windows_powershell_51_emits_non_ascii_output_as_utf8() {
        let output = run_powershell(r"Write-Output 'C:\使用者\Yuzora'", &[]).unwrap();
        assert_eq!(output.stdout.trim(), r"C:\使用者\Yuzora");
    }

    #[test]
    fn aggregates_adapter_status_without_guessing_mixed_distros() {
        assert_eq!(
            parse_adapter_status("distro=Ubuntu action=status\ncurrent\n"),
            HerdrWslAdapterStatus::Current
        );
        assert_eq!(
            parse_adapter_status(
                "distro=A action=status\ncurrent\ndistro=B action=status\nabsent\n"
            ),
            HerdrWslAdapterStatus::Mixed
        );
        assert_eq!(
            parse_adapter_status("distro=Ubuntu action=install\ninstalled current\n"),
            HerdrWslAdapterStatus::Current
        );
        assert_eq!(
            parse_adapter_status("distro=Ubuntu action=uninstall\nuninstalled absent\n"),
            HerdrWslAdapterStatus::Absent
        );
    }

    #[test]
    fn bundle_availability_requires_the_exact_runtime_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = plugin_root(dir.path());
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        for path in required_bundle_paths(&root) {
            std::fs::write(path, "fixture").unwrap();
        }
        assert!(bundle_is_available(&root));
        std::fs::remove_file(root.join(ADAPTER_MANAGER_RELATIVE)).unwrap();
        assert!(!bundle_is_available(&root));
    }

    #[test]
    fn enable_refuses_preexisting_inactive_owned_registration_without_mutation() {
        let mut ops = FakeOps::successful();
        let before = integration_fixture(true, false, true, HerdrWslAdapterStatus::Current);
        let error = set_integration_transaction(&mut ops, before, true).unwrap_err();
        assert!(error.contains("automatic repair is refused"), "{error}");
        assert!(ops.calls.is_empty());
    }

    #[test]
    fn foreign_registration_performs_zero_mutations() {
        let mut ops = FakeOps::successful();
        let before = integration_fixture(true, true, false, HerdrWslAdapterStatus::Unknown);
        let error = set_integration_transaction(&mut ops, before, true).unwrap_err();
        assert!(error.contains("another root"), "{error}");
        assert!(ops.calls.is_empty());
    }

    #[test]
    fn uncertain_link_failure_runs_idempotent_cleanup() {
        let mut ops = FakeOps::successful();
        ops.link_result = Err("status after link failed".into());
        let before = integration_fixture(false, false, false, HerdrWslAdapterStatus::Absent);
        let error = set_integration_transaction(&mut ops, before, true).unwrap_err();
        assert!(error.contains("rolled back"), "{error}");
        assert_eq!(ops.calls, ["link", "uninstall-all", "unlink"]);
    }

    #[test]
    fn failed_new_install_rolls_back_only_the_new_registration() {
        let mut ops = FakeOps::successful();
        ops.install_result = Err("install failed".into());
        let before = integration_fixture(false, false, false, HerdrWslAdapterStatus::Absent);
        let error = set_integration_transaction(&mut ops, before, true).unwrap_err();
        assert!(error.contains("rolled back"), "{error}");
        assert_eq!(
            ops.calls,
            ["link", "install-configured", "uninstall-all", "unlink"]
        );
    }

    #[test]
    fn failed_enable_cleanup_keeps_registration_linked_for_recovery() {
        let mut ops = FakeOps::successful();
        ops.install_result = Err("install failed".into());
        ops.uninstall_result = Ok(HerdrWslAdapterStatus::Drifted);
        let before = integration_fixture(false, false, false, HerdrWslAdapterStatus::Absent);
        let error = set_integration_transaction(&mut ops, before, true).unwrap_err();
        assert!(error.contains("remains linked for recovery"), "{error}");
        assert_eq!(ops.calls, ["link", "install-configured", "uninstall-all"]);
    }

    #[test]
    fn disable_uninstalls_all_distros_before_unlinking() {
        let mut ops = FakeOps::successful();
        ops.status_result = Ok(integration_fixture(
            false,
            false,
            false,
            HerdrWslAdapterStatus::Absent,
        ));
        let before = integration_fixture(true, true, true, HerdrWslAdapterStatus::Current);
        let result = set_integration_transaction(&mut ops, before, false).unwrap();
        assert!(!result.active);
        assert_eq!(ops.calls, ["uninstall-all", "unlink", "status"]);
    }

    #[test]
    fn disable_does_not_unlink_when_any_distro_is_not_absent() {
        let mut ops = FakeOps::successful();
        ops.uninstall_result = Ok(HerdrWslAdapterStatus::Mixed);
        let before = integration_fixture(true, true, true, HerdrWslAdapterStatus::Current);
        let error = set_integration_transaction(&mut ops, before, false).unwrap_err();
        assert!(error.contains("Plugin remains linked"), "{error}");
        assert_eq!(ops.calls, ["uninstall-all"]);
    }

    #[test]
    fn transaction_lock_serializes_concurrent_mutations() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(5));
        let mut threads = Vec::new();
        for _ in 0..4 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let barrier = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                barrier.wait();
                with_transaction_lock(|| {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn file_backed_capture_reads_bytes_while_a_writer_handle_remains_open() {
        use std::io::Write;

        let capture = tempfile::NamedTempFile::new().unwrap();
        let mut writer = capture.as_file().try_clone().unwrap();
        writer.write_all(b"still-open\n").unwrap();
        writer.flush().unwrap();
        let bytes = read_capped_capture(&capture, 64, "stdout").unwrap();
        assert_eq!(&bytes, b"still-open\n");
        drop(writer);
    }

    #[test]
    fn file_backed_capture_rejects_output_past_the_byte_cap() {
        use std::io::Write;

        let capture = tempfile::NamedTempFile::new().unwrap();
        let mut writer = capture.as_file().try_clone().unwrap();
        writer.write_all(&[b'x'; 8]).unwrap();
        writer.flush().unwrap();
        let error = read_capped_capture(&capture, 4, "stdout").unwrap_err();
        assert!(error.contains("exceeded"), "{error}");
    }

    #[test]
    fn capture_len_uses_open_handle_metadata() {
        use std::io::Write;

        let capture = tempfile::NamedTempFile::new().unwrap();
        let mut writer = capture.as_file().try_clone().unwrap();
        writer.write_all(&[b'x'; 8]).unwrap();
        writer.flush().unwrap();
        assert_eq!(capture_len(&capture, "stdout").unwrap(), 8);
        assert_eq!(
            oversized_capture_label(&capture, &capture, 4).unwrap(),
            Some("stdout")
        );
        assert_eq!(
            oversized_capture_label(&capture, &capture, 8).unwrap(),
            None
        );
    }

    #[test]
    fn read_capped_capture_returns_without_waiting_for_open_writer_to_close() {
        use std::io::Write;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let capture = tempfile::NamedTempFile::new().unwrap();
        let mut writer = capture.as_file().try_clone().unwrap();
        writer.write_all(&[b'x'; 64]).unwrap();
        writer.flush().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let writer_thread = thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                let _ = writer.write_all(&[b'y'; 32]);
                let _ = writer.flush();
                thread::sleep(Duration::from_millis(10));
            }
        });
        let started = Instant::now();
        let error = read_capped_capture(&capture, 32, "stdout").unwrap_err();
        assert!(
            error.contains("stdout capture exceeded 32 bytes"),
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "bounded read waited for writer close: {:?}",
            started.elapsed()
        );
        stop.store(true, Ordering::SeqCst);
        writer_thread.join().unwrap();
    }

    #[test]
    fn oversized_wait_preserves_cleanup_and_reap_failure_precedence() {
        assert_eq!(
            append_oversized_context(
                "process-tree cleanup failed: boom".into(),
                Some("stdout"),
                32
            ),
            "process-tree cleanup failed: boom; stdout capture exceeded 32 bytes"
        );
        assert_eq!(
            append_oversized_context("reap failed: gone".into(), Some("stderr"), 8),
            "reap failed: gone; stderr capture exceeded 8 bytes"
        );
        assert_eq!(
            append_oversized_context("wait failed: closed".into(), Some("stdout"), 4),
            "wait failed: closed; stdout capture exceeded 4 bytes"
        );
        assert_eq!(
            append_oversized_context("process-tree cleanup failed: boom".into(), None, 32),
            "process-tree cleanup failed: boom"
        );
        assert_eq!(
            capture_exceeded_error("stdout", 32),
            "stdout capture exceeded 32 bytes"
        );
    }

    fn spawn_output_holder_with_lingering_descendant(
        keepalive: &Path,
        stdout: std::fs::File,
        stderr: std::fs::File,
    ) -> Child {
        #[cfg(unix)]
        {
            Command::new("sh")
                .arg("-c")
                .arg("printf 'ready\\n'; while [ -f \"$1\" ]; do sleep 0.05; done &")
                .arg("holder")
                .arg(keepalive)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn unix output holder")
        }
        #[cfg(windows)]
        {
            let path = keepalive.display().to_string().replace('\'', "''");
            let script = format!(
                "Write-Output 'ready'; Start-Process -NoNewWindow -FilePath \"$env:SystemRoot\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -ArgumentList @('-NoProfile','-Command',\"while (Test-Path -LiteralPath '{path}') {{ Start-Sleep -Milliseconds 50 }}\")"
            );
            Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &script,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn windows output holder")
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (keepalive, stdout, stderr);
            panic!("unsupported platform for descendant-handle regression");
        }
    }

    #[test]
    fn file_backed_wait_returns_after_child_exits_while_descendant_holds_output() {
        let stdout_capture = tempfile::NamedTempFile::new().unwrap();
        let stderr_capture = tempfile::NamedTempFile::new().unwrap();
        let keepalive = tempfile::NamedTempFile::new().unwrap();
        let stdout = stdout_capture.as_file().try_clone().unwrap();
        let stderr = stderr_capture.as_file().try_clone().unwrap();
        let mut child =
            spawn_output_holder_with_lingering_descendant(keepalive.path(), stdout, stderr);
        let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
        let started = Instant::now();
        let (stdout, _stderr, status) = wait_bounded_file_backed_child(
            &mut child,
            &mut process_tree,
            Duration::from_secs(5),
            &stdout_capture,
            &stderr_capture,
            4096,
        )
        .expect("file-backed wait");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "file-backed wait hung: {:?}",
            started.elapsed()
        );
        assert!(status.success(), "{status:?}");
        let decoded = decode_process_text(stdout, "stdout").unwrap();
        assert!(decoded.contains("ready"), "{decoded:?}");
        drop(keepalive);
    }

    fn spawn_exited_parent_with_appending_descendant(
        keepalive: &Path,
        stdout: std::fs::File,
        stderr: std::fs::File,
    ) -> Child {
        #[cfg(unix)]
        {
            Command::new("sh")
                .arg("-c")
                .arg("printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; while [ -f \"$1\" ]; do printf xxxxxxxx; done &")
                .arg("holder")
                .arg(keepalive)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn unix appending descendant")
        }
        #[cfg(windows)]
        {
            let path = keepalive.display().to_string().replace('\'', "''");
            let script = format!(
                "Write-Output ('x' * 64); Start-Process -NoNewWindow -FilePath \"$env:SystemRoot\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -ArgumentList @('-NoProfile','-Command',\"while (Test-Path -LiteralPath '{path}') {{ Write-Output ('x' * 8); Start-Sleep -Milliseconds 50 }}\")"
            );
            Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &script,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn windows appending descendant")
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (keepalive, stdout, stderr);
            panic!("unsupported platform for appending-descendant regression");
        }
    }

    #[test]
    fn file_backed_wait_returns_when_exited_child_leaves_an_appending_writer() {
        let stdout_capture = tempfile::NamedTempFile::new().unwrap();
        let stderr_capture = tempfile::NamedTempFile::new().unwrap();
        let keepalive = tempfile::NamedTempFile::new().unwrap();
        let stdout = stdout_capture.as_file().try_clone().unwrap();
        let stderr = stderr_capture.as_file().try_clone().unwrap();
        let mut child =
            spawn_exited_parent_with_appending_descendant(keepalive.path(), stdout, stderr);
        let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
        let started = Instant::now();
        let error = wait_bounded_file_backed_child(
            &mut child,
            &mut process_tree,
            Duration::from_secs(5),
            &stdout_capture,
            &stderr_capture,
            32,
        )
        .expect_err("appending writer past the cap should fail closed");
        assert!(
            error.contains("stdout capture exceeded 32 bytes"),
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "bounded wait/read hung on open writer: {:?}",
            started.elapsed()
        );
        assert!(child.try_wait().unwrap().is_some());
        drop(keepalive);
    }

    fn spawn_slow_oversized_writer(stdout: std::fs::File, stderr: std::fs::File) -> Child {
        #[cfg(unix)]
        {
            Command::new("yes")
                .arg("xxxxxxxx")
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn unix oversized writer")
        }
        #[cfg(windows)]
        {
            Command::new("powershell.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "while ($true) { Write-Output 'xxxxxxxx'; Start-Sleep -Milliseconds 50 }",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .expect("spawn windows oversized writer")
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (stdout, stderr);
            panic!("unsupported platform for oversized-output regression");
        }
    }

    #[test]
    fn file_backed_wait_terminates_early_when_capture_exceeds_cap() {
        let stdout_capture = tempfile::NamedTempFile::new().unwrap();
        let stderr_capture = tempfile::NamedTempFile::new().unwrap();
        let stdout = stdout_capture.as_file().try_clone().unwrap();
        let stderr = stderr_capture.as_file().try_clone().unwrap();
        let mut child = spawn_slow_oversized_writer(stdout, stderr);
        let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
        let started = Instant::now();
        let error = wait_bounded_file_backed_child(
            &mut child,
            &mut process_tree,
            Duration::from_secs(5),
            &stdout_capture,
            &stderr_capture,
            32,
        )
        .expect_err("oversized capture should fail closed");
        assert!(
            error.contains("stdout capture exceeded 32 bytes"),
            "{error}"
        );
        assert!(
            !error.contains("process-tree cleanup failed"),
            "successful terminate must not be reported as cleanup failure: {error}"
        );
        assert!(
            !error.contains("reap failed"),
            "successful reap must not be reported as reap failure: {error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "oversized capture waited too long: {:?}",
            started.elapsed()
        );
        assert!(child.try_wait().unwrap().is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_status_is_explicitly_unsupported() {
        let status = integration_status(&HerdrManager::new()).unwrap();
        assert!(!status.platform_supported);
        assert!(!status.active);
        assert!(status.reason.unwrap().contains("only on Windows"));
    }
}
