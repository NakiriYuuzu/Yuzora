use crate::clock::Timestamp;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

pub fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value.trim() == value
        && value.chars().all(|c| !c.is_control() && !label_format(c))
}
// Unicode Cf ranges (including bidi/zero-width controls), plus line/paragraph
// separators. Keep explicit: labels must not hide or reorder approval text.
fn label_format(c: char) -> bool {
    matches!(c as u32,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x0890..=0x0891 |
        0x08e2 | 0x180e | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2064 |
        0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd |
        0x13430..=0x1343f | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a |
        0xe0001 | 0xe0020..=0xe007f)
}
pub fn decode<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != (N * 8).div_ceil(6) {
        return None;
    }
    URL_SAFE_NO_PAD.decode(value).ok()?.try_into().ok()
}
pub fn valid_rp_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.parse::<std::net::IpAddr>().is_err()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlGrant {
    pub until: Timestamp,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grants {
    pub control: Option<ControlGrant>,
    pub full: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    Usb,
    Nfc,
    Ble,
    Internal,
    Hybrid,
    SmartCard,
}

/// Version-pinned WebAuthn persistence. Parsing validates key state, not a ceremony.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRecord {
    pub id: String,
    pub rp_id: String,
    pub public_key_state: String,
    pub state_format: String,
    pub sign_count: u32,
    pub backup_eligible: bool,
    pub backup_state: bool,
    pub transports: Vec<Transport>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialRecordFields {
    id: String,
    rp_id: String,
    public_key_state: String,
    state_format: String,
    sign_count: u32,
    backup_eligible: bool,
    backup_state: bool,
    transports: Vec<Transport>,
}
impl<'de> Deserialize<'de> for CredentialRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let fields = CredentialRecordFields::deserialize(d)?;
        let value = Self {
            id: fields.id,
            rp_id: fields.rp_id,
            public_key_state: fields.public_key_state,
            state_format: fields.state_format,
            sign_count: fields.sign_count,
            backup_eligible: fields.backup_eligible,
            backup_state: fields.backup_state,
            transports: fields.transports,
        };
        if !value.valid() {
            return Err(D::Error::custom("Invalid CredentialRecord"));
        }
        Ok(value)
    }
}

impl CredentialRecord {
    pub fn valid(&self) -> bool {
        fn binary(value: &str, max: usize) -> bool {
            !value.is_empty()
                && value.len() <= max.div_ceil(3) * 4
                && URL_SAFE_NO_PAD
                    .decode(value)
                    .is_ok_and(|v| !v.is_empty() && v.len() <= max)
        }
        binary(&self.id, 1024)
            && self.state_format == crate::webauthn::STATE_FORMAT
            && crate::webauthn::key_state(&self.public_key_state).is_some()
            && valid_rp_id(&self.rp_id)
            && (!self.backup_state || self.backup_eligible)
            && self.transports.len() <= 6
            && self
                .transports
                .iter()
                .map(std::mem::discriminant)
                .collect::<HashSet<_>>()
                .len()
                == self.transports.len()
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    pub device_id: String,
    pub label: String,
    pub credentials: Vec<CredentialRecord>,
    pub grants: Grants,
    pub locked: bool,
    pub created_at: Timestamp,
    pub last_seen: Timestamp,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceFields {
    device_id: String,
    label: String,
    credentials: Vec<CredentialRecord>,
    grants: Grants,
    locked: bool,
    created_at: Timestamp,
    last_seen: Timestamp,
}
impl<'de> Deserialize<'de> for Device {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let fields = DeviceFields::deserialize(d)?;
        let value = Self {
            device_id: fields.device_id,
            label: fields.label,
            credentials: fields.credentials,
            grants: fields.grants,
            locked: fields.locked,
            created_at: fields.created_at,
            last_seen: fields.last_seen,
        };
        if !value.valid() {
            return Err(D::Error::custom("Invalid Device"));
        }
        Ok(value)
    }
}

impl Device {
    pub fn valid(&self) -> bool {
        decode::<16>(&self.device_id).is_some()
            && valid_label(&self.label)
            && !self.credentials.is_empty()
            && self.credentials.len() <= 8
            && self.credentials.iter().all(CredentialRecord::valid)
            && self
                .credentials
                .iter()
                .map(|c| (&c.rp_id, &c.id))
                .collect::<HashSet<_>>()
                .len()
                == self.credentials.len()
            && self.last_seen >= self.created_at
    }
}

pub fn deserialize_devices<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Device>, D::Error> {
    let devices = Vec::<Device>::deserialize(d)?;
    if devices.len() > 64
        || !devices.iter().all(Device::valid)
        || devices
            .iter()
            .map(|v| &v.device_id)
            .collect::<HashSet<_>>()
            .len()
            != devices.len()
        || devices
            .iter()
            .flat_map(|v| v.credentials.iter().map(|c| (&c.rp_id, &c.id)))
            .collect::<HashSet<_>>()
            .len()
            != devices.iter().map(|v| v.credentials.len()).sum::<usize>()
    {
        return Err(D::Error::custom("Invalid devices"));
    }
    Ok(devices)
}
