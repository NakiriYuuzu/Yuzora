//! Desktop-owned Companion state under `<app_data>/companion/`: the paired
//! devices the gateway reports, and the settings that build its init frame.
//! No secrets live here. Files are written atomically and owner-only; a
//! corrupt file is backed up and kept, and Companion stays off until resolved.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const DEVICES_FILE: &str = "devices.json";
const SETTINGS_FILE: &str = "settings.json";
const VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
struct DevicesFile {
    version: u32,
    /// Stored exactly as the gateway emitted them: init re-parses each device
    /// with `deny_unknown_fields`, so nothing may be added or normalised.
    devices: Vec<Value>,
}

pub struct Devices {
    path: PathBuf,
    devices: Vec<Value>,
}
impl Devices {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(DEVICES_FILE);
        let devices = match read_versioned::<DevicesFile>(&path, |file| file.version)? {
            Some(file) => file.devices,
            None => Vec::new(),
        };
        if devices.iter().any(|device| device_id(device).is_none()) {
            return Err(corrupt(&path));
        }
        Ok(Self { path, devices })
    }
    pub fn all(&self) -> &[Value] {
        &self.devices
    }
    /// `device.registered` and `device.updated` carry the full device.
    pub fn upsert(&mut self, device: Value) -> Result<(), String> {
        let id = device_id(&device)
            .ok_or("companion-device-invalid")?
            .to_owned();
        let mut next = self.devices.clone();
        match next.iter_mut().find(|d| device_id(d) == Some(&id)) {
            Some(slot) => *slot = device,
            None => next.push(device),
        }
        self.commit(next)
    }
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let next = self
            .devices
            .iter()
            .filter(|d| device_id(d) != Some(id))
            .cloned()
            .collect();
        self.commit(next)
    }
    /// Applies a desktop device command (`device.update`, `device.unlock`,
    /// `device.revoke`) to the stored copy: edits made while the gateway is
    /// off, and commands whose gateway notification was deferred.
    pub fn apply(&mut self, frame: &Value) -> Result<(), String> {
        let id = frame["deviceId"]
            .as_str()
            .ok_or("companion-device-invalid")?;
        if frame["type"] == "device.revoke" {
            return self.remove(id);
        }
        let mut device = self
            .devices
            .iter()
            .find(|d| device_id(d) == Some(id))
            .cloned()
            .ok_or("companion-device-unknown")?;
        match frame["type"].as_str() {
            Some("device.update") => {
                let label = frame.get("label").filter(|v| !v.is_null());
                let grants = frame.get("grants").filter(|v| !v.is_null());
                // The gateway's rules: something must change, and a grant
                // change may not tighten and widen at once.
                if label.is_none() && grants.is_none() {
                    return Err("companion-device-invalid".into());
                }
                if grants.is_some_and(|grants| mixed(&device["grants"], grants)) {
                    return Err("companion-grants-mixed".into());
                }
                for (field, value) in [("label", label), ("grants", grants)] {
                    if let Some(value) = value {
                        device[field] = value.clone();
                    }
                }
            }
            Some("device.unlock") => device["locked"] = Value::Bool(false),
            _ => return Err("companion-device-invalid".into()),
        }
        self.upsert(device)
    }
    /// Memory follows disk only after the write succeeds, so a failed revoke
    /// can never look applied.
    fn commit(&mut self, devices: Vec<Value>) -> Result<(), String> {
        write_json(
            &self.path,
            &DevicesFile {
                version: VERSION,
                devices: devices.clone(),
            },
        )?;
        self.devices = devices;
        Ok(())
    }
}

/// No control grant sorts before any; a later `until` is wider.
fn mixed(old: &Value, new: &Value) -> bool {
    let until = |grants: &Value| {
        grants["control"]["until"]
            .as_str()
            .and_then(|until| chrono::DateTime::parse_from_rfc3339(until).ok())
    };
    let full = |grants: &Value| grants["full"].as_bool().unwrap_or(false);
    let (old_until, new_until) = (until(old), until(new));
    let tightening = new_until < old_until || (!full(new) && full(old));
    let widening = new_until > old_until || (full(new) && !full(old));
    tightening && widening
}

pub fn device_id(device: &Value) -> Option<&str> {
    device.get("deviceId")?.as_str().filter(|id| !id.is_empty())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum EnableFor {
    #[serde(rename = "1h")]
    OneHour,
    #[default]
    #[serde(rename = "8h")]
    EightHours,
    /// Until Yuzora exits; the gateway still needs a concrete deadline.
    #[serde(rename = "untilExit")]
    UntilExit,
}
impl EnableFor {
    pub fn as_std(self) -> std::time::Duration {
        let hours = match self {
            Self::OneHour => 1,
            Self::EightHours => 8,
            Self::UntilExit => 24 * 365,
        };
        std::time::Duration::from_secs(hours * 3600)
    }
}

/// The Serve handler this desktop added. Kept apart from the current ports:
/// removal must target what was applied, even after the settings change.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedServe {
    pub https_port: u16,
    pub gateway_port: u16,
    pub host: String,
    pub login: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub version: u32,
    pub https_port: u16,
    pub gateway_port: u16,
    pub duration: EnableFor,
    pub session_allowlist: Vec<String>,
    pub full_session_enabled: bool,
    pub applied: Option<AppliedServe>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            https_port: 8443,
            gateway_port: 47110,
            duration: EnableFor::default(),
            session_allowlist: Vec::new(),
            full_session_enabled: false,
            applied: None,
        }
    }
}
impl Settings {
    pub fn load(dir: &Path) -> Result<Self, String> {
        Ok(read_versioned::<Self>(&dir.join(SETTINGS_FILE), |s| s.version)?.unwrap_or_default())
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        write_json(&dir.join(SETTINGS_FILE), self)
    }
}

fn read_versioned<T: for<'de> Deserialize<'de>>(
    path: &Path,
    version: impl Fn(&T) -> u32,
) -> Result<Option<T>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("companion-store-unreadable: {e}")),
    };
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) if version(&value) == VERSION => Ok(Some(value)),
        _ => Err(corrupt(path)),
    }
}

/// Keeps the original in place so nothing silently rebuilds it.
fn corrupt(path: &Path) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".corrupt-{stamp}"));
    let _ = std::fs::copy(path, backup);
    "companion-store-corrupt".to_string()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("companion-store-write-failed: {e}");
    let dir = path.parent().ok_or("companion-store-write-failed")?;
    create_private_dir(dir).map_err(failed)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&tmp).map_err(failed)?;
    file.write_all(&bytes).map_err(failed)?;
    file.sync_all().map_err(failed)?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(failed)
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests;
