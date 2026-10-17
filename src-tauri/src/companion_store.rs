//! Desktop-owned Companion state under `<app_data>/companion/`: the paired
//! devices the gateway reports, and the settings that build its init frame.
//! No secrets live here. Files are written atomically and owner-only; a
//! corrupt file is backed up and kept, and Companion stays off until resolved.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
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
        if !valid_devices(&devices) {
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

/// The gateway's init rules for stored devices (`companion/src/devices.rs`),
/// except credential key state, which only the gateway parses. Checked on load
/// only: a file that fails them would fail every start, so it is backed up as
/// corrupt instead. Never stricter than the gateway, or a good file is lost.
fn valid_devices(devices: &[Value]) -> bool {
    const FIELDS: [&str; 7] = [
        "deviceId",
        "label",
        "credentials",
        "grants",
        "locked",
        "createdAt",
        "lastSeen",
    ];
    let timestamp = |value: &Value| {
        value
            .as_str()
            .filter(|value| valid_until(value))
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    };
    let mut ids = HashSet::new();
    let mut credentials = HashSet::new();
    devices.len() <= 64
        && devices.iter().all(|device| {
            device
                .as_object()
                .is_some_and(|d| d.len() == FIELDS.len() && FIELDS.iter().all(|f| d.contains_key(*f)))
                && device["deviceId"].as_str().is_some_and(|id| {
                    id.len() == 22
                        && URL_SAFE_NO_PAD.decode(id).is_ok_and(|bytes| bytes.len() == 16)
                        && ids.insert(id.to_owned())
                })
                && device["label"].as_str().is_some_and(|label| valid_label(label).is_ok())
                && valid_grants(&device["grants"]).is_ok()
                && device["locked"].is_boolean()
                && matches!(
                    (timestamp(&device["createdAt"]), timestamp(&device["lastSeen"])),
                    (Some(created), Some(seen)) if seen >= created
                )
                // Credential ids are unique per relying party across all devices.
                && device["credentials"].as_array().is_some_and(|list| {
                    (1..=8).contains(&list.len())
                        && list.iter().all(|c| match (c["rpId"].as_str(), c["id"].as_str()) {
                            (Some(rp), Some(id)) => credentials.insert((rp.to_owned(), id.to_owned())),
                            _ => false,
                        })
                })
        })
}

/// The gateway's own label rule (`companion/src/devices.rs`): a label it
/// rejects would make init fail at the next start.
pub(crate) fn valid_label(label: &str) -> Result<(), String> {
    if !label.is_empty()
        && label.len() <= 80
        && label.trim() == label
        && label.chars().all(|c| !c.is_control() && !label_format(c))
    {
        Ok(())
    } else {
        Err("companion-label-invalid".into())
    }
}

// Mirrors the gateway's explicit Unicode Cf list.
fn label_format(c: char) -> bool {
    matches!(c as u32,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891 |
        0x08e2 | 0x180e | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 |
        0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd |
        0x13430..=0x1343f | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a |
        0xe0001 | 0xe0020..=0xe007f)
}

/// The gateway's timestamp rule (`companion/src/clock.rs`): chrono alone
/// accepts more RFC 3339 variants than the admin protocol.
pub(crate) fn valid_until(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(core) = bytes
        .strip_suffix(b"Z")
        .or_else(|| bytes.strip_suffix(b"+00:00"))
    else {
        return false;
    };
    let shape = core.len() >= 19
        && core[..19].iter().enumerate().all(|(i, byte)| match i {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            _ => byte.is_ascii_digit(),
        })
        && (core.len() == 19
            || ((21..=29).contains(&core.len())
                && core[19] == b'.'
                && core[20..].iter().all(u8::is_ascii_digit)));
    shape
        && chrono::DateTime::parse_from_rfc3339(value)
            .is_ok_and(|parsed| parsed.timestamp_subsec_nanos() < 1_000_000_000)
}

/// `{control: {until} | null, full: bool}`, exactly; init rejects anything else.
pub(crate) fn valid_grants(grants: &Value) -> Result<(), String> {
    let Some(object) = grants.as_object() else {
        return Err("companion-grants-invalid".into());
    };
    let control_ok = match object.get("control") {
        Some(Value::Null) => true,
        Some(Value::Object(control)) => {
            control.len() == 1
                && control
                    .get("until")
                    .and_then(Value::as_str)
                    .is_some_and(valid_until)
        }
        _ => false,
    };
    if object.len() == 2 && control_ok && object.get("full").is_some_and(Value::is_boolean) {
        Ok(())
    } else {
        Err("companion-grants-invalid".into())
    }
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
    crate::ssh_service::retry_transient(crate::ssh_service::windows_scanner_held, || {
        std::fs::rename(&tmp, path)
    })
    .map_err(failed)
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
