use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::herdr_service::{
    probe_binary_identity, HerdrBinarySource, HerdrBinarySourceInfo, HerdrBinarySourceSetResult,
    HerdrCapabilities, HerdrManager, HerdrNamedSession, HerdrScrollDirection, HerdrSnapshotResult,
    HerdrTerminalMode, HerdrTerminalOpenResult, OnSubscriptionEvent, OnTerminalEvent,
};

use super::{HerdrRuntimeProvider, HerdrRuntimeTarget};

const BINARY_SOURCE_CONFIG_FILE: &str = "herdr-config-v1.json";

/// Native binary-source lifecycle is runtime-owned. The facade retains public
/// DTOs, but this state is the only owner of source preference, resolution,
/// managed-resource fallback, and restart-required semantics.
pub struct NativeBinarySourceState {
    binary_override: Mutex<Option<PathBuf>>,
    managed_binary_override: Mutex<Option<PathBuf>>,
    config_dir: Mutex<Option<PathBuf>>,
    resource_dir: Mutex<Option<PathBuf>>,
    configured_source: Mutex<HerdrBinarySource>,
    active_source: Mutex<HerdrBinarySource>,
    configuration_error: Mutex<Option<String>>,
    write_lock: Mutex<()>,
}

impl Default for NativeBinarySourceState {
    fn default() -> Self {
        Self {
            binary_override: Mutex::new(None),
            managed_binary_override: Mutex::new(None),
            config_dir: Mutex::new(None),
            resource_dir: Mutex::new(None),
            configured_source: Mutex::new(HerdrBinarySource::Global),
            active_source: Mutex::new(HerdrBinarySource::Global),
            configuration_error: Mutex::new(None),
            write_lock: Mutex::new(()),
        }
    }
}

impl NativeBinarySourceState {
    pub fn configure_paths(&self, config_dir: PathBuf, resource_dir: Option<PathBuf>) {
        *self.config_dir.lock().unwrap() = Some(config_dir.clone());
        *self.resource_dir.lock().unwrap() = resource_dir;
        let loaded = load_preference(&config_dir);
        *self.configured_source.lock().unwrap() = loaded.source;
        *self.active_source.lock().unwrap() = loaded.source;
        *self.configuration_error.lock().unwrap() = loaded.error;
    }

    pub fn info(&self) -> HerdrBinarySourceInfo {
        let configured = *self.configured_source.lock().unwrap();
        let active = *self.active_source.lock().unwrap();
        let (active_path, resolved, active_reason) = self.resolve_selection(active);
        let (configured_path, _configured_resolved, configured_reason) = if configured == active {
            (active_path.clone(), resolved, active_reason.clone())
        } else {
            self.resolve_selection(configured)
        };
        let (version, protocol) = active_path
            .as_deref()
            .map(probe_binary_identity)
            .unwrap_or((None, None));
        let (configured_version, configured_protocol) = if configured == active {
            (version.clone(), protocol)
        } else {
            configured_path
                .as_deref()
                .map(probe_binary_identity)
                .unwrap_or((None, None))
        };
        HerdrBinarySourceInfo {
            configured,
            active,
            resolved,
            available: active_path.is_some(),
            path: active_path.map(|path| path.to_string_lossy().into_owned()),
            reason: active_reason,
            version,
            protocol,
            configured_available: configured_path.is_some(),
            configured_path: configured_path.map(|path| path.to_string_lossy().into_owned()),
            configured_reason,
            configured_version,
            configured_protocol,
            configuration_error: self.configuration_error.lock().unwrap().clone(),
            restart_required: configured != active,
        }
    }

    pub fn configured_source(&self) -> HerdrBinarySource {
        *self.configured_source.lock().unwrap()
    }

    /// Persists a preference without changing the current process's active
    /// executable. The next manager construction/configuration observes it.
    pub fn set_configured_source(
        &self,
        source: HerdrBinarySource,
    ) -> Result<HerdrBinarySourceSetResult, String> {
        let config_dir = self
            .config_dir
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "herdr config directory is not configured".to_string())?;
        let _write_guard = self.write_lock.lock().unwrap();
        save_preference(&config_dir, source)?;
        *self.configured_source.lock().unwrap() = source;
        *self.configuration_error.lock().unwrap() = None;
        let active = *self.active_source.lock().unwrap();
        Ok(HerdrBinarySourceSetResult {
            configured: source,
            restart_required: source != active,
        })
    }

    pub fn resolve_active_binary(&self) -> Option<PathBuf> {
        let active = *self.active_source.lock().unwrap();
        self.resolve_selection(active).0
    }

    /// Global is automatic: first PATH, then a bundled managed binary. An
    /// explicit managed source is strict and never falls back to PATH.
    pub fn resolve_selection(
        &self,
        source: HerdrBinarySource,
    ) -> (Option<PathBuf>, Option<HerdrBinarySource>, Option<String>) {
        let has_explicit_override = self.binary_override.lock().unwrap().is_some();
        let primary = self.resolve_for_source(source);
        let managed =
            if source == HerdrBinarySource::Global && primary.0.is_none() && !has_explicit_override
            {
                self.resolve_for_source(HerdrBinarySource::Default)
            } else {
                (None, None)
            };
        select_binary_resolution(source, has_explicit_override, primary, managed)
    }

    pub fn resolve_for_source(
        &self,
        source: HerdrBinarySource,
    ) -> (Option<PathBuf>, Option<String>) {
        if let Some(path) = self.binary_override.lock().unwrap().clone() {
            if is_executable(&path) {
                return (Some(path), None);
            }
            return (
                None,
                Some(format!(
                    "herdr binary override is not executable: {}",
                    path.display()
                )),
            );
        }
        match source {
            HerdrBinarySource::Global => match which_binary("herdr").map(PathBuf::from) {
                Some(path) => (Some(path), None),
                None => (None, Some("Herdr was not found on PATH".into())),
            },
            HerdrBinarySource::Default => {
                if let Some(path) = self.managed_binary_override.lock().unwrap().clone() {
                    if is_executable(&path) {
                        return (Some(path), None);
                    }
                    return (
                        None,
                        Some("Yuzora-managed Herdr override is not an executable file".into()),
                    );
                }
                let Some(resource_dir) = self.resource_dir.lock().unwrap().clone() else {
                    return (
                        None,
                        Some("This build does not include a managed Herdr binary".into()),
                    );
                };
                let candidate = managed_binary_path(&resource_dir);
                if is_executable(&candidate) {
                    (Some(candidate), None)
                } else {
                    (
                        None,
                        Some(format!(
                            "Yuzora-managed Herdr binary is unavailable at {}",
                            candidate.display()
                        )),
                    )
                }
            }
        }
    }

    #[cfg(test)]
    pub fn set_binary_override_for_test(&self, path: Option<PathBuf>) {
        *self.binary_override.lock().unwrap() = path;
    }

    #[cfg(test)]
    pub fn set_managed_binary_override_for_test(&self, path: Option<PathBuf>) {
        *self.managed_binary_override.lock().unwrap() = path;
    }

    #[cfg(test)]
    pub fn set_config_dir_for_test(&self, dir: PathBuf) {
        self.configure_paths(dir, None);
    }

    #[cfg(test)]
    pub fn active_source_for_test(&self) -> HerdrBinarySource {
        *self.active_source.lock().unwrap()
    }

    #[cfg(test)]
    pub fn set_sources_for_test(&self, source: HerdrBinarySource) {
        *self.active_source.lock().unwrap() = source;
        *self.configured_source.lock().unwrap() = source;
    }
}

/// Adapter for the host-local Herdr binary, local socket, and official terminal
/// connector. It is the Phase 1 reference implementation of runtime ownership.
pub struct NativeHerdrRuntimeProvider {
    binary_source: NativeBinarySourceState,
}

impl Default for NativeHerdrRuntimeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeHerdrRuntimeProvider {
    pub fn new() -> Self {
        Self {
            binary_source: NativeBinarySourceState::default(),
        }
    }

    pub fn binary_source(&self) -> &NativeBinarySourceState {
        &self.binary_source
    }
}

impl HerdrRuntimeProvider for NativeHerdrRuntimeProvider {
    fn target(&self) -> HerdrRuntimeTarget {
        HerdrRuntimeTarget::Native
    }

    fn validate(&self, target: &HerdrRuntimeTarget) -> Result<(), String> {
        if matches!(target, HerdrRuntimeTarget::Native) {
            Ok(())
        } else {
            Err(format!(
                "native Herdr provider cannot serve runtime target {target}"
            ))
        }
    }

    fn list_sessions(
        &self,
        _runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
    ) -> Result<Vec<HerdrNamedSession>, String> {
        manager.list_sessions()
    }

    fn capabilities(
        &self,
        _runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> HerdrCapabilities {
        manager.capabilities_for_session(session_name)
    }

    fn snapshot(
        &self,
        _runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> Result<HerdrSnapshotResult, String> {
        manager.snapshot(session_name)
    }

    fn request(
        &self,
        _runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        manager.call_checked_api_for_method(session_name, method, params)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_terminal(
        &self,
        _runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        target: String,
        mode: HerdrTerminalMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        session_name: Option<String>,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String> {
        manager.open_terminal(target, mode, takeover, cols, rows, session_name, on_event)
    }

    fn terminal_input(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        manager.terminal_input_for_runtime(runtime_target, session_id, text, bytes_base64)
    }

    fn terminal_resize(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        manager.terminal_resize_for_runtime(runtime_target, session_id, cols, rows)
    }

    fn terminal_scroll(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        direction: HerdrScrollDirection,
        lines: u32,
    ) -> Result<(), String> {
        manager.terminal_scroll_for_runtime(runtime_target, session_id, direction, lines)
    }

    fn terminal_release(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
    ) -> Result<(), String> {
        manager.terminal_release_for_runtime(runtime_target, session_id)
    }

    fn subscribe(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        session_name: Option<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String> {
        manager.events_subscribe_for_runtime(runtime_target, session_name, on_event)
    }

    fn release_subscription(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        subscription_id: &str,
    ) -> Result<(), String> {
        manager.events_release_for_runtime(runtime_target, subscription_id)
    }
}

pub(crate) fn select_binary_resolution(
    source: HerdrBinarySource,
    has_explicit_override: bool,
    primary: (Option<PathBuf>, Option<String>),
    managed: (Option<PathBuf>, Option<String>),
) -> (Option<PathBuf>, Option<HerdrBinarySource>, Option<String>) {
    let (primary_path, primary_reason) = primary;
    if primary_path.is_some() {
        return (primary_path, Some(source), primary_reason);
    }
    if source != HerdrBinarySource::Global || has_explicit_override {
        return (None, None, primary_reason);
    }
    let (managed_path, managed_reason) = managed;
    if managed_path.is_some() {
        return (
            managed_path,
            Some(HerdrBinarySource::Default),
            Some("Herdr was not found on PATH; using Yuzora-managed Herdr".into()),
        );
    }
    let primary_reason =
        primary_reason.unwrap_or_else(|| "Herdr was not found on PATH".to_string());
    let managed_reason =
        managed_reason.unwrap_or_else(|| "Yuzora-managed Herdr is unavailable".to_string());
    (
        None,
        None,
        Some(format!(
            "{primary_reason}; Yuzora-managed fallback is also unavailable: {managed_reason}"
        )),
    )
}

fn managed_binary_path(resource_dir: &Path) -> PathBuf {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let file_name = if cfg!(windows) { "herdr.exe" } else { "herdr" };
    resource_dir
        .join("herdr")
        .join(format!("{os}-{arch}"))
        .join(file_name)
}

fn preference_path(config_dir: &Path) -> PathBuf {
    config_dir.join(BINARY_SOURCE_CONFIG_FILE)
}

struct PreferenceLoad {
    source: HerdrBinarySource,
    error: Option<String>,
}

fn load_preference(config_dir: &Path) -> PreferenceLoad {
    let path = preference_path(config_dir);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PreferenceLoad {
                source: HerdrBinarySource::Global,
                error: None,
            };
        }
        Err(error) => {
            return PreferenceLoad {
                source: HerdrBinarySource::Global,
                error: Some(format!(
                    "failed to read Herdr binary-source preference at {}: {error}",
                    path.display()
                )),
            };
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => value,
        Err(error) => {
            return PreferenceLoad {
                source: HerdrBinarySource::Global,
                error: Some(format!(
                    "invalid Herdr binary-source preference at {}: {error}",
                    path.display()
                )),
            };
        }
    };
    match value
        .get("binarySource")
        .and_then(serde_json::Value::as_str)
    {
        Some("global") => PreferenceLoad {
            source: HerdrBinarySource::Global,
            error: None,
        },
        Some("default") => PreferenceLoad {
            source: HerdrBinarySource::Default,
            error: None,
        },
        other => PreferenceLoad {
            source: HerdrBinarySource::Global,
            error: Some(format!(
                "unknown Herdr binarySource {:?} in {}",
                other,
                path.display()
            )),
        },
    }
}

fn save_preference(config_dir: &Path, source: HerdrBinarySource) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|error| format!("failed to create herdr config dir: {error}"))?;
    let path = preference_path(config_dir);
    let value = serde_json::json!({
        "binarySource": match source {
            HerdrBinarySource::Global => "global",
            HerdrBinarySource::Default => "default",
        }
    });
    let body = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(config_dir)
        .map_err(|error| format!("failed to create temporary herdr config: {error}"))?;
    temp.write_all(body.as_bytes())
        .and_then(|_| temp.flush())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|error| format!("failed to flush herdr config: {error}"))?;
    temp.persist(&path)
        .map_err(|error| format!("failed to atomically replace herdr config: {}", error.error))?;
    #[cfg(unix)]
    fs::File::open(config_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("failed to sync herdr config directory: {error}"))?;
    Ok(())
}

fn which_binary(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(command);
        if is_executable(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{command}.exe"));
            if is_executable(&candidate) {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
pub(crate) fn binary_source_config_path_for_test(config_dir: &Path) -> PathBuf {
    preference_path(config_dir)
}
