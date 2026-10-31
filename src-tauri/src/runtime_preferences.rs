//! App-level runtime switches that must be enforced on the Rust side. The WSL
//! runtime and the experimental herdr-core runtime are opt-in: anything unreadable fails
//! closed to "disabled".
//!
//! Every setter reads the file again, changes its own field and writes the file back, so
//! flipping one switch never resets another, and an installed Yuzora and a debug build that
//! share this file (same app identifier, same app data dir) do not undo each other's fields.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::host_service::HostTarget;

const RUNTIME_PREFERENCES_FILE: &str = "runtime-preferences.json";
pub const PREFERENCES_UNWRITABLE_ERROR: &str = "runtime-preferences-unwritable";
pub const WSL_DISABLED_ERROR: &str = "wsl-runtime-disabled-open-settings";

/// Debug builds use their own herdr-core switch and session; see [`RuntimePreferences`].
pub const DEV_BUILD: bool = cfg!(debug_assertions);
/// The herdr-core session every debug build uses, whatever the file says.
pub const DEV_SESSION: &str = "yuzora-dev";

/// The herdr-core session an installed build connects to: Yuzora's own, or the user's
/// `default`. Anything else in the file is `Yuzora`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrCoreSession {
    #[default]
    Yuzora,
    Default,
}

impl HerdrCoreSession {
    pub fn name(self) -> &'static str {
        match self {
            HerdrCoreSession::Yuzora => "yuzora",
            HerdrCoreSession::Default => "default",
        }
    }

    pub fn parse(name: &str) -> Option<HerdrCoreSession> {
        match name {
            "yuzora" => Some(HerdrCoreSession::Yuzora),
            "default" => Some(HerdrCoreSession::Default),
            _ => None,
        }
    }
}

/// A herdr-core field that is not the type it should be reads as its safe value instead of
/// failing the whole file (which would also reset the WSL switch).
fn lenient_bool<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    use serde::Deserialize as _;
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_bool()
        .unwrap_or(false))
}

fn lenient_session<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HerdrCoreSession, D::Error> {
    use serde::Deserialize as _;
    Ok(serde_json::Value::deserialize(deserializer)?
        .as_str()
        .and_then(HerdrCoreSession::parse)
        .unwrap_or_default())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePreferences {
    #[serde(default)]
    pub wsl_enabled: bool,
    /// Experimental herdr-core runtime, installed builds.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub herdr_core_enabled: bool,
    /// The same switch for debug builds, so a dev run never turns the installed app's
    /// herdr-core on (or the other way round).
    #[serde(default, deserialize_with = "lenient_bool")]
    pub herdr_core_enabled_dev: bool,
    /// Ignored by debug builds, which always use [`DEV_SESSION`] and never write this field:
    /// the installed app reads it from the same file.
    #[serde(default, deserialize_with = "lenient_session")]
    pub herdr_core_session: HerdrCoreSession,
}

impl RuntimePreferences {
    /// `dev`: the switch of a debug build instead of the installed one.
    pub fn herdr_core_enabled_for(&self, dev: bool) -> bool {
        if dev {
            self.herdr_core_enabled_dev
        } else {
            self.herdr_core_enabled
        }
    }

    pub fn herdr_core_session_for(&self, dev: bool) -> &'static str {
        if dev {
            DEV_SESSION
        } else {
            self.herdr_core_session.name()
        }
    }
}

#[derive(Clone, Default)]
pub struct RuntimePreferencesState(pub Arc<Mutex<RuntimePreferences>>);

impl RuntimePreferencesState {
    pub fn wsl_enabled(&self) -> bool {
        self.0.lock().unwrap().wsl_enabled
    }

    /// The herdr-core switch and session this build acts on.
    pub fn herdr_core(&self) -> (bool, &'static str) {
        let preferences = self.0.lock().unwrap();
        (
            preferences.herdr_core_enabled_for(DEV_BUILD),
            preferences.herdr_core_session_for(DEV_BUILD),
        )
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

/// What a setter starts from: the file as it is now. Missing or corrupt means the defaults
/// (as at startup); a file that exists but cannot be read is an error, so a setter never
/// writes defaults over settings it could not see.
fn read_for_update(dir: &Path) -> Result<RuntimePreferences, String> {
    match std::fs::read_to_string(preferences_path(dir)) {
        Ok(raw) => Ok(serde_json::from_str(&raw).unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RuntimePreferences::default()),
        Err(e) => Err(format!("failed to read preferences: {e}")),
    }
}

/// Read, change one field, write back. `apply` runs on the file's contents and on this
/// process's copy, so the copy only ever changes the field a setter changed: another
/// process's edit to a different field does not leak into this process's gates.
pub fn update(
    dir: &Path,
    state: &RuntimePreferencesState,
    apply: impl Fn(&mut RuntimePreferences),
) -> Result<RuntimePreferences, String> {
    let mut current = state.0.lock().unwrap();
    let mut next = read_for_update(dir)?;
    apply(&mut next);
    save_to(dir, &next)?;
    apply(&mut current);
    Ok(*current)
}

/// `dev`: change the debug builds' switch instead of the installed one.
pub fn set_herdr_core_enabled(
    dir: &Path,
    state: &RuntimePreferencesState,
    enabled: bool,
    dev: bool,
) -> Result<RuntimePreferences, String> {
    update(dir, state, |preferences| {
        if dev {
            preferences.herdr_core_enabled_dev = enabled;
        } else {
            preferences.herdr_core_enabled = enabled;
        }
    })
}

/// Why [`set_herdr_core_session`] changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetSessionError {
    /// A debug build's session is [`DEV_SESSION`] and the file's session field belongs to the
    /// installed app: letting a dev run write it could point the installed app at the user's
    /// `default` session. Nothing is read or written.
    DevSessionFixed,
    Unwritable(String),
}

/// `dev`: this is a debug build, which may not change the session.
pub fn set_herdr_core_session(
    dir: &Path,
    state: &RuntimePreferencesState,
    session: HerdrCoreSession,
    dev: bool,
) -> Result<RuntimePreferences, SetSessionError> {
    if dev {
        return Err(SetSessionError::DevSessionFixed);
    }
    update(dir, state, |preferences| {
        preferences.herdr_core_session = session;
    })
    .map_err(SetSessionError::Unwritable)
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
    let next = update(&dir, &state, |preferences| {
        preferences.wsl_enabled = wsl_enabled;
    })
    .map_err(|e| format!("{PREFERENCES_UNWRITABLE_ERROR}: {e}"))?;
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
            RuntimePreferences {
                wsl_enabled: false,
                ..Default::default()
            }
        );
        save_to(
            dir.path(),
            &RuntimePreferences {
                wsl_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(load_from(dir.path()).wsl_enabled);
        let raw = std::fs::read_to_string(dir.path().join("runtime-preferences.json")).unwrap();
        assert!(raw.contains("\"wslEnabled\": true"), "{raw}");
        save_to(
            dir.path(),
            &RuntimePreferences {
                wsl_enabled: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!load_from(dir.path()).wsl_enabled);
    }

    #[test]
    fn herdr_runtime_preferences_startup_falls_back_when_dir_unresolvable() {
        let failing: Result<PathBuf, &str> = Err("no app data dir");
        assert_eq!(load_for_startup(failing), RuntimePreferences::default());
        let dir = tempfile::tempdir().unwrap();
        save_to(
            dir.path(),
            &RuntimePreferences {
                wsl_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
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
                ..Default::default()
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

    fn state_of(preferences: RuntimePreferences) -> RuntimePreferencesState {
        RuntimePreferencesState(Arc::new(Mutex::new(preferences)))
    }

    fn write_raw(dir: &Path, raw: &str) {
        std::fs::write(dir.join("runtime-preferences.json"), raw).unwrap();
    }

    #[test]
    fn herdr_core_preferences_default_to_off_and_the_yuzora_session() {
        let dir = tempfile::tempdir().unwrap();
        for preferences in [
            load_from(dir.path()),
            RuntimePreferences::default(),
            serde_json::from_str("{}").unwrap(),
        ] {
            assert!(!preferences.herdr_core_enabled);
            assert!(!preferences.herdr_core_enabled_dev);
            assert_eq!(preferences.herdr_core_session, HerdrCoreSession::Yuzora);
            assert!(!preferences.herdr_core_enabled_for(true));
            assert!(!preferences.herdr_core_enabled_for(false));
            assert_eq!(preferences.herdr_core_session_for(false), "yuzora");
        }
    }

    #[test]
    fn herdr_core_preferences_unreadable_files_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        for corrupt in [
            "",
            "{",
            "[]",
            "\"true\"",
            "\u{0}\u{1}",
            r#"{"herdrCoreEnabled":true,"wslEnabled":"yes"}"#,
        ] {
            write_raw(dir.path(), corrupt);
            let preferences = load_from(dir.path());
            assert!(!preferences.herdr_core_enabled, "{corrupt:?}");
            assert!(!preferences.herdr_core_enabled_dev, "{corrupt:?}");
            assert_eq!(preferences.herdr_core_session, HerdrCoreSession::Yuzora);
        }
        // A path that is a directory cannot be read as a file either.
        std::fs::remove_file(dir.path().join("runtime-preferences.json")).unwrap();
        std::fs::create_dir(dir.path().join("runtime-preferences.json")).unwrap();
        assert!(!load_from(dir.path()).herdr_core_enabled);
    }

    #[test]
    fn herdr_core_preferences_wrong_types_read_as_the_safe_value_without_losing_wsl() {
        let dir = tempfile::tempdir().unwrap();
        for (raw, session) in [
            (r#""default""#, HerdrCoreSession::Default),
            (r#""yuzora""#, HerdrCoreSession::Yuzora),
            (r#""Default""#, HerdrCoreSession::Yuzora),
            (r#""yuzora-dev""#, HerdrCoreSession::Yuzora),
            (r#""../x""#, HerdrCoreSession::Yuzora),
            ("7", HerdrCoreSession::Yuzora),
            ("null", HerdrCoreSession::Yuzora),
            (r#"{"a":1}"#, HerdrCoreSession::Yuzora),
        ] {
            write_raw(
                dir.path(),
                &format!(
                    r#"{{"wslEnabled":true,"herdrCoreEnabled":"yes","herdrCoreEnabledDev":1,"herdrCoreSession":{raw}}}"#
                ),
            );
            let preferences = load_from(dir.path());
            assert!(preferences.wsl_enabled, "{raw}");
            assert!(!preferences.herdr_core_enabled, "{raw}");
            assert!(!preferences.herdr_core_enabled_dev, "{raw}");
            assert_eq!(preferences.herdr_core_session, session, "{raw}");
        }
    }

    #[test]
    fn herdr_core_preferences_use_camel_case_names_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        save_to(
            dir.path(),
            &RuntimePreferences {
                herdr_core_enabled: true,
                herdr_core_enabled_dev: true,
                herdr_core_session: HerdrCoreSession::Default,
                ..Default::default()
            },
        )
        .unwrap();
        let raw = std::fs::read_to_string(dir.path().join("runtime-preferences.json")).unwrap();
        for expected in [
            "\"herdrCoreEnabled\": true",
            "\"herdrCoreEnabledDev\": true",
            "\"herdrCoreSession\": \"default\"",
            "\"wslEnabled\": false",
        ] {
            assert!(raw.contains(expected), "{expected} in {raw}");
        }
    }

    #[test]
    fn herdr_core_setters_read_modify_write_without_clobbering_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_of(RuntimePreferences::default());

        update(dir.path(), &state, |p| p.wsl_enabled = true).unwrap();
        set_herdr_core_enabled(dir.path(), &state, true, false).unwrap();
        set_herdr_core_session(dir.path(), &state, HerdrCoreSession::Default, false).unwrap();
        // Toggling WSL again must not rebuild the struct and reset the herdr-core fields.
        let after_wsl = update(dir.path(), &state, |p| p.wsl_enabled = false).unwrap();
        assert!(!after_wsl.wsl_enabled);
        assert!(after_wsl.herdr_core_enabled);
        assert_eq!(after_wsl.herdr_core_session, HerdrCoreSession::Default);
        let on_disk = load_from(dir.path());
        assert_eq!(on_disk, after_wsl);

        // ... and the other way round.
        update(dir.path(), &state, |p| p.wsl_enabled = true).unwrap();
        let after_core = set_herdr_core_enabled(dir.path(), &state, false, false).unwrap();
        assert!(after_core.wsl_enabled);
        assert!(!after_core.herdr_core_enabled);
        assert_eq!(load_from(dir.path()), after_core);
    }

    #[test]
    fn herdr_core_debug_builds_keep_their_own_switch() {
        let dir = tempfile::tempdir().unwrap();
        // The installed app and a dev run each have a process-local copy of the one file.
        let installed = state_of(RuntimePreferences::default());
        let dev = state_of(RuntimePreferences::default());

        set_herdr_core_enabled(dir.path(), &dev, true, true).unwrap();
        let on_disk = load_from(dir.path());
        assert!(on_disk.herdr_core_enabled_dev);
        assert!(!on_disk.herdr_core_enabled);
        assert!(on_disk.herdr_core_enabled_for(true));
        assert!(!on_disk.herdr_core_enabled_for(false));

        // The installed app writes its own field, from a copy that never saw the dev switch:
        // the file keeps both, and neither process's gate moved.
        let installed_view = set_herdr_core_enabled(dir.path(), &installed, true, false).unwrap();
        assert!(installed_view.herdr_core_enabled);
        assert!(!installed_view.herdr_core_enabled_dev);
        let on_disk = load_from(dir.path());
        assert!(on_disk.herdr_core_enabled && on_disk.herdr_core_enabled_dev);
        assert!(dev.0.lock().unwrap().herdr_core_enabled_dev);
        assert!(!dev.0.lock().unwrap().herdr_core_enabled);

        set_herdr_core_enabled(dir.path(), &dev, false, true).unwrap();
        let on_disk = load_from(dir.path());
        assert!(on_disk.herdr_core_enabled && !on_disk.herdr_core_enabled_dev);
    }

    #[test]
    fn herdr_core_a_debug_build_cannot_change_the_session_and_leaves_the_file_alone() {
        // Dev and installed builds share this file. Whatever state it is in, a dev setter
        // neither creates nor rewrites it.
        let installed = RuntimePreferences {
            wsl_enabled: true,
            herdr_core_enabled: true,
            herdr_core_session: HerdrCoreSession::Yuzora,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("runtime-preferences.json");
        let state = state_of(installed);
        assert_eq!(
            set_herdr_core_session(dir.path(), &state, HerdrCoreSession::Default, true),
            Err(SetSessionError::DevSessionFixed)
        );
        assert!(!file.exists(), "a dev run must not create the file either");

        // An existing file is byte for byte what it was, and so is the process's copy.
        write_raw(
            dir.path(),
            r#"{"wslEnabled":true,"herdrCoreSession":"yuzora"}"#,
        );
        let before = std::fs::read(&file).unwrap();
        for session in [HerdrCoreSession::Default, HerdrCoreSession::Yuzora] {
            assert_eq!(
                set_herdr_core_session(dir.path(), &state, session, true),
                Err(SetSessionError::DevSessionFixed)
            );
        }
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(*state.0.lock().unwrap(), installed);

        // The installed build's setter still works on the same file.
        let next =
            set_herdr_core_session(dir.path(), &state, HerdrCoreSession::Default, false).unwrap();
        assert_eq!(next.herdr_core_session, HerdrCoreSession::Default);
        assert_eq!(
            load_from(dir.path()).herdr_core_session,
            HerdrCoreSession::Default
        );
    }

    #[test]
    fn herdr_core_debug_builds_always_use_the_dev_session() {
        let preferences = RuntimePreferences {
            herdr_core_session: HerdrCoreSession::Default,
            ..Default::default()
        };
        assert_eq!(preferences.herdr_core_session_for(false), "default");
        assert_eq!(preferences.herdr_core_session_for(true), "yuzora-dev");
        assert_eq!(DEV_SESSION, "yuzora-dev");
        assert_eq!(
            HerdrCoreSession::parse("yuzora"),
            Some(HerdrCoreSession::Yuzora)
        );
        assert_eq!(
            HerdrCoreSession::parse("default"),
            Some(HerdrCoreSession::Default)
        );
        for rejected in ["", "yuzora-dev", "Yuzora", " default", "other"] {
            assert_eq!(HerdrCoreSession::parse(rejected), None, "{rejected:?}");
        }
        // This build's view, as the service applies it.
        let (enabled, session) = state_of(preferences).herdr_core();
        assert!(!enabled);
        assert_eq!(session, if DEV_BUILD { "yuzora-dev" } else { "default" });
    }

    #[test]
    fn herdr_core_update_does_not_write_over_a_file_it_cannot_read() {
        let dir = tempfile::tempdir().unwrap();
        // The path exists but cannot be read as a file. `load_from` reads that as the
        // defaults, but a setter must not replace what it could not see.
        std::fs::create_dir(dir.path().join("runtime-preferences.json")).unwrap();
        let state = state_of(RuntimePreferences::default());
        assert!(set_herdr_core_enabled(dir.path(), &state, true, false).is_err());
        assert!(!state.0.lock().unwrap().herdr_core_enabled);

        // A missing or corrupt file is simply the defaults to start from.
        let dir = tempfile::tempdir().unwrap();
        write_raw(dir.path(), "{");
        let next = set_herdr_core_enabled(dir.path(), &state, true, false).unwrap();
        assert!(next.herdr_core_enabled);
        assert!(load_from(dir.path()).herdr_core_enabled);
    }
}
