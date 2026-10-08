//! App-level runtime switches that must be enforced on the Rust side. The WSL
//! runtime is opt-in: anything unreadable fails closed to "disabled".
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::host_service::HostTarget;

const RUNTIME_PREFERENCES_FILE: &str = "runtime-preferences.json";
pub const PREFERENCES_UNWRITABLE_ERROR: &str = "runtime-preferences-unwritable";
pub const WSL_DISABLED_ERROR: &str = "wsl-runtime-disabled-open-settings";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreferences {
    #[serde(default)]
    pub wsl_enabled: bool,
}

#[derive(Clone, Default)]
pub struct RuntimePreferencesState(pub Arc<Mutex<RuntimePreferences>>);

impl RuntimePreferencesState {
    pub fn wsl_enabled(&self) -> bool {
        self.0.lock().unwrap().wsl_enabled
    }
}

fn preferences_path(dir: &Path) -> PathBuf {
    dir.join(RUNTIME_PREFERENCES_FILE)
}

/// Missing or corrupt files mean the default (WSL off); never an error.
pub fn load_from(dir: &Path) -> RuntimePreferences {
    std::fs::read_to_string(preferences_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Startup never fails on an unresolvable data dir: fall back to the default
/// (WSL off) and log, so the app still launches.
pub fn load_for_startup<E: std::fmt::Display>(dir: Result<PathBuf, E>) -> RuntimePreferences {
    match dir {
        Ok(dir) => load_from(&dir),
        Err(error) => {
            eprintln!("runtime preferences: app data dir unavailable, using defaults: {error}");
            RuntimePreferences::default()
        }
    }
}

pub fn save_to(dir: &Path, preferences: &RuntimePreferences) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create preferences dir: {e}"))?;
    let body = serde_json::to_string_pretty(preferences).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("failed to create temporary preferences file: {e}"))?;
    temp.write_all(body.as_bytes())
        .and_then(|_| temp.flush())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| format!("failed to flush preferences: {e}"))?;
    temp.persist(preferences_path(dir))
        .map_err(|e| format!("failed to atomically replace preferences: {}", e.error))?;
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("failed to sync preferences directory: {e}"))?;
    Ok(())
}

/// Gate for every entry point that reaches a WSL distribution.
pub fn require_wsl_enabled(
    state: &RuntimePreferencesState,
    target: &HostTarget,
) -> Result<(), String> {
    require_wsl_flag(
        state.wsl_enabled(),
        matches!(target, HostTarget::Wsl { .. }),
    )
}

pub fn require_wsl_flag(enabled: bool, is_wsl: bool) -> Result<(), String> {
    if is_wsl && !enabled {
        return Err(WSL_DISABLED_ERROR.into());
    }
    Ok(())
}

#[tauri::command]
pub fn runtime_preferences_get(
    state: tauri::State<'_, RuntimePreferencesState>,
) -> RuntimePreferences {
    *state.0.lock().unwrap()
}

#[tauri::command]
pub fn runtime_preferences_set(
    app: tauri::AppHandle,
    state: tauri::State<'_, RuntimePreferencesState>,
    hosts: tauri::State<'_, crate::host_service::HostState>,
    wsl_enabled: bool,
) -> Result<RuntimePreferences, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("{PREFERENCES_UNWRITABLE_ERROR}: {e}"))?;
    let next = RuntimePreferences { wsl_enabled };
    {
        let mut current = state.0.lock().unwrap();
        save_to(&dir, &next).map_err(|e| format!("{PREFERENCES_UNWRITABLE_ERROR}: {e}"))?;
        *current = next;
    }
    // Lookups already refuse WSL; also end streams opened before the switch.
    // The preferences lock is released first: admitting a connection takes
    // the connection lock before it reads the preferences.
    if !wsl_enabled {
        hosts.0.disconnect_wsl();
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_runtime_preferences_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_from(dir.path()),
            RuntimePreferences { wsl_enabled: false }
        );
        save_to(dir.path(), &RuntimePreferences { wsl_enabled: true }).unwrap();
        assert!(load_from(dir.path()).wsl_enabled);
        let raw = std::fs::read_to_string(dir.path().join("runtime-preferences.json")).unwrap();
        assert!(raw.contains("\"wslEnabled\": true"), "{raw}");
        save_to(dir.path(), &RuntimePreferences { wsl_enabled: false }).unwrap();
        assert!(!load_from(dir.path()).wsl_enabled);
    }

    #[test]
    fn herdr_runtime_preferences_startup_falls_back_when_dir_unresolvable() {
        let failing: Result<PathBuf, &str> = Err("no app data dir");
        assert_eq!(load_for_startup(failing), RuntimePreferences::default());
        let dir = tempfile::tempdir().unwrap();
        save_to(dir.path(), &RuntimePreferences { wsl_enabled: true }).unwrap();
        let ok: Result<PathBuf, &str> = Ok(dir.path().to_path_buf());
        assert!(load_for_startup(ok).wsl_enabled);
    }

    #[test]
    fn herdr_runtime_preferences_unwritable_dir_reports_error_code() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(save_to(&blocker.join("sub"), &RuntimePreferences::default()).is_err());
        assert_eq!(
            PREFERENCES_UNWRITABLE_ERROR,
            "runtime-preferences-unwritable"
        );
    }

    #[test]
    fn herdr_runtime_preferences_missing_or_corrupt_file_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runtime-preferences.json");
        assert!(!load_from(dir.path()).wsl_enabled);
        for corrupt in [
            "",
            "{",
            "[]",
            "\"true\"",
            r#"{"wslEnabled":"yes"}"#,
            "\u{0}\u{1}",
        ] {
            std::fs::write(&path, corrupt).unwrap();
            assert!(!load_from(dir.path()).wsl_enabled, "{corrupt:?}");
        }
        std::fs::write(&path, "{}").unwrap();
        assert!(!load_from(dir.path()).wsl_enabled);
    }

    #[test]
    fn herdr_runtime_preferences_require_wsl_enabled_table() {
        let wsl = HostTarget::Wsl {
            distro: "Ubuntu".into(),
        };
        let state = |enabled| {
            RuntimePreferencesState(Arc::new(Mutex::new(RuntimePreferences {
                wsl_enabled: enabled,
            })))
        };
        assert_eq!(
            require_wsl_enabled(&state(false), &wsl),
            Err("wsl-runtime-disabled-open-settings".to_string())
        );
        assert_eq!(require_wsl_enabled(&state(true), &wsl), Ok(()));
        for target in [
            HostTarget::Local,
            HostTarget::Ssh {
                session_id: "s".into(),
            },
        ] {
            assert_eq!(require_wsl_enabled(&state(false), &target), Ok(()));
            assert_eq!(require_wsl_enabled(&state(true), &target), Ok(()));
        }
        assert!(require_wsl_flag(false, true).is_err());
        assert!(require_wsl_flag(false, false).is_ok());
    }
}
