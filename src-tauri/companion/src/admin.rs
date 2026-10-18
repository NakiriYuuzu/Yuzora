//! Versioned desktop-only channel. Never log frames, serde errors, or init values.
use crate::{
    clock::Timestamp,
    devices::{Device, Grants},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{timeout, Duration},
};
use yuzora_host::protocol::MAX_FRAME_BYTES;
use zeroize::Zeroizing;

pub struct Command {
    pub v: u32,
    pub body: CommandBody,
}

pub enum CommandBody {
    Init(Box<Init>),
    Shutdown,
    Pairing(PairingCommand),
    Device(DeviceCommand),
    Unknown,
}

// Unknown values are skipped, not buffered as serde Content. Version validation
// precedes command-body validation, including future fields and command shapes.
#[derive(Deserialize)]
struct Envelope {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShutdownFrame {
    #[serde(rename = "v")]
    _v: u32,
    #[serde(rename = "type")]
    _kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Init {
    #[serde(rename = "v")]
    _v: u32,
    #[serde(rename = "type")]
    _kind: String,
    pub herdr_binary: PathBuf,
    /// Future bridge must check this AND the running-session list; never auto-start.
    pub session_allowlist: Vec<String>,
    pub web_root: PathBuf,
    pub listeners: Listeners,
    #[serde(deserialize_with = "crate::devices::deserialize_devices")]
    pub devices: Vec<Device>,
    pub policy: Policy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listeners {
    pub loopback: Loopback,
    pub lan: Option<Lan>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Loopback {
    #[serde(default = "default_port")]
    pub port: u16,
    pub public_host: String,
    pub required_tailscale_login: String,
}

fn default_port() -> u16 {
    47110
}

// No Debug/Serialize: PEM material must never be printed or persisted.
pub struct SecretPem(pub Zeroizing<String>);
impl<'de> Deserialize<'de> for SecretPem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| Self(Zeroizing::new(s)))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lan {
    pub binds: Vec<std::net::IpAddr>,
    pub port: u16,
    pub public_host: String,
    pub cert_pem: SecretPem,
    pub key_pem: SecretPem,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub enable_until: Timestamp,
    pub view_idle_min: u32,
    pub view_absolute_hours: u32,
    pub elevation_max_min: u32,
    pub full_session_enabled: bool,
    pub max_devices: u32,
    #[serde(default)]
    pub passkey_binding: PasskeyBinding,
}
impl Policy {
    pub fn valid(&self) -> bool {
        (1..=30).contains(&self.view_idle_min)
            && (1..=12).contains(&self.view_absolute_hours)
            && (1..=15).contains(&self.elevation_max_min)
            && (1..=64).contains(&self.max_devices)
    }
}
/// Which passkeys may authorize each scope; every login and elevation also
/// requires user verification (Face ID, device passcode). O1: all scopes need
/// a credential that cannot sync. O2 (default): synced passkeys may view and
/// control, full needs one that cannot sync. O3: any passkey, every scope.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PasskeyBinding {
    O1,
    #[default]
    O2,
    O3,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PairingCommand {
    #[serde(rename = "pairing.create")]
    Create {
        v: u32,
        path: String,
        #[serde(rename = "addToDevice")]
        add_to_device: Option<String>,
    },
    #[serde(rename = "pairing.approve")]
    Approve {
        v: u32,
        #[serde(rename = "pairingId")]
        pairing_id: String,
    },
    #[serde(rename = "pairing.reject")]
    Reject {
        v: u32,
        #[serde(rename = "pairingId")]
        pairing_id: String,
    },
    #[serde(rename = "pairing.cancel")]
    Cancel {
        v: u32,
        #[serde(rename = "pairingId")]
        pairing_id: String,
    },
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum DeviceCommand {
    #[serde(rename = "device.update")]
    Update {
        v: u32,
        #[serde(rename = "deviceId")]
        device_id: String,
        label: Option<String>,
        grants: Option<Grants>,
    },
    #[serde(rename = "device.revoke")]
    Revoke {
        v: u32,
        #[serde(rename = "deviceId")]
        device_id: String,
    },
    #[serde(rename = "device.unlock")]
    Unlock {
        v: u32,
        #[serde(rename = "deviceId")]
        device_id: String,
    },
}

/// A deferred result means the security decision is already committed, not failed.
#[derive(Debug, PartialEq, Eq)]
pub enum AdminResult {
    Applied,
    NotificationDeferred {
        command: &'static str,
        target: String,
    },
}
impl AdminResult {
    pub fn deferred_event(self) -> Option<Event> {
        match self {
            Self::Applied => None,
            Self::NotificationDeferred { command, target } => Some(Event {
                v: 1,
                body: EventBody::CommandDeferred {
                    command,
                    target,
                    committed: true,
                },
            }),
        }
    }
}

/// Names reserved for later typed payloads; none are emitted before auth exists.
pub const RESERVED_EVENTS: &[&str] = &["push.request"];

/// These are distinct scopes, not an ordered privilege level. The gateway must
/// enforce each method's scope itself: Herdr silently ignores observer writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    View,
    Control,
    Full,
}

#[derive(Clone, Serialize)]
pub struct Event {
    v: u32,
    #[serde(flatten)]
    body: EventBody,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum EventBody {
    #[serde(rename = "audit", rename_all = "camelCase")]
    Audit {
        device_id: String,
        peer_id: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        outcome: Option<String>,
        scope: Scope,
        action: String,
        session: String,
        target: Option<String>,
        summary: serde_json::Value,
        at: Timestamp,
    },
    #[serde(rename = "audit.dropped", rename_all = "camelCase")]
    AuditDropped {
        lost: u64,
        lost_tightening: u64,
        rejected_omitted: u64,
        since: Timestamp,
    },
    #[serde(rename = "control.state", rename_all = "camelCase")]
    ControlState {
        device_id: String,
        peer_id: u64,
        stream: String,
        seq: u64,
        session: String,
        target: String,
        takeover: bool,
        open: bool,
    },
    #[serde(rename = "full.state", rename_all = "camelCase")]
    FullState {
        device_id: String,
        peer_id: u64,
        stream: String,
        seq: u64,
        session: String,
        open: bool,
    },
    #[serde(rename = "command.deferred")]
    CommandDeferred {
        command: &'static str,
        target: String,
        committed: bool,
    },
    #[serde(rename = "session.opened", rename_all = "camelCase")]
    SessionOpened {
        peer_id: u64,
        device_id: String,
    },
    #[serde(rename = "session.closed", rename_all = "camelCase")]
    SessionClosed {
        peer_id: u64,
        device_id: String,
        reason: &'static str,
    },
    Ready {
        ports: Ports,
    },
    #[serde(rename = "pairing.created", rename_all = "camelCase")]
    PairingCreated {
        pairing_id: String,
        url: String,
        expires_at: Timestamp,
    },
    #[serde(rename = "pairing.claimed", rename_all = "camelCase")]
    PairingClaimed {
        pairing_id: String,
        device_label: String,
        sas: String,
        via: &'static str,
        tailscale_login: String,
        user_agent: String,
    },
    #[serde(rename = "pairing.state", rename_all = "camelCase")]
    PairingState {
        pairing_id: String,
        state: &'static str,
        reason: &'static str,
    },
    #[serde(rename = "device.registered")]
    DeviceRegistered {
        device: Device,
    },
    #[serde(rename = "auth.outcome", rename_all = "camelCase")]
    AuthOutcome {
        ceremony: &'static str,
        outcome: &'static str,
        device_id: Option<String>,
    },
    #[serde(rename = "device.updated", rename_all = "camelCase")]
    DeviceUpdated {
        device: Device,
    },
    #[serde(rename = "device.revoked", rename_all = "camelCase")]
    DeviceRevoked {
        device_id: String,
    },
    Error {
        code: &'static str,
        message: &'static str,
    },
}
#[derive(Clone, Serialize)]
struct Ports {
    loopback: u16,
    lan: Option<u16>,
}
impl Event {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn audit(
        device_id: String,
        peer_id: u64,
        scope: Scope,
        action: String,
        session: String,
        target: Option<String>,
        summary: serde_json::Value,
        at: Timestamp,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::Audit {
                device_id,
                peer_id,
                outcome: summary
                    .get("outcome")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                reason: summary
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                scope,
                action,
                session,
                target,
                summary,
                at,
            },
        }
    }
    pub(crate) fn audit_dropped(
        lost: u64,
        lost_tightening: u64,
        rejected_omitted: u64,
        since: Timestamp,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::AuditDropped {
                lost,
                lost_tightening,
                rejected_omitted,
                since,
            },
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn control_state(
        device_id: String,
        peer_id: u64,
        stream: String,
        seq: u64,
        session: String,
        target: String,
        takeover: bool,
        open: bool,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::ControlState {
                device_id,
                peer_id,
                stream,
                seq,
                session,
                target,
                takeover,
                open,
            },
        }
    }
    pub(crate) fn full_state(
        device_id: String,
        peer_id: u64,
        stream: String,
        seq: u64,
        session: String,
        open: bool,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::FullState {
                device_id,
                peer_id,
                stream,
                seq,
                session,
                open,
            },
        }
    }
    pub(crate) fn session_opened(peer_id: u64, device_id: String) -> Self {
        Self {
            v: 1,
            body: EventBody::SessionOpened { peer_id, device_id },
        }
    }
    pub(crate) fn session_closed(peer_id: u64, device_id: String, reason: &'static str) -> Self {
        Self {
            v: 1,
            body: EventBody::SessionClosed {
                peer_id,
                device_id,
                reason,
            },
        }
    }

    pub fn pairing_created(pairing_id: String, url: String, expires_at: Timestamp) -> Self {
        Self {
            v: 1,
            body: EventBody::PairingCreated {
                pairing_id,
                url,
                expires_at,
            },
        }
    }
    pub fn pairing_claimed(
        pairing_id: String,
        device_label: String,
        sas: String,
        tailscale_login: String,
        user_agent: String,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::PairingClaimed {
                pairing_id,
                device_label,
                sas,
                via: "tailscale",
                tailscale_login,
                user_agent,
            },
        }
    }
    pub fn pairing_state(pairing_id: String, state: &'static str, reason: &'static str) -> Self {
        Self {
            v: 1,
            body: EventBody::PairingState {
                pairing_id,
                state,
                reason,
            },
        }
    }
    pub fn device_registered(device: Device) -> Self {
        Self {
            v: 1,
            body: EventBody::DeviceRegistered { device },
        }
    }
    pub fn auth_outcome(
        ceremony: &'static str,
        outcome: &'static str,
        device_id: Option<String>,
    ) -> Self {
        Self {
            v: 1,
            body: EventBody::AuthOutcome {
                ceremony,
                outcome,
                device_id,
            },
        }
    }
    pub fn device_updated(device: Device) -> Self {
        Self {
            v: 1,
            body: EventBody::DeviceUpdated { device },
        }
    }
    pub fn device_revoked(device_id: String) -> Self {
        Self {
            v: 1,
            body: EventBody::DeviceRevoked { device_id },
        }
    }
    pub fn ready(port: u16) -> Self {
        Self {
            v: 1,
            body: EventBody::Ready {
                ports: Ports {
                    loopback: port,
                    lan: None,
                },
            },
        }
    }
    pub fn error(code: &'static str) -> Self {
        Self {
            v: 1,
            body: EventBody::Error {
                code,
                message: "Companion request rejected",
            },
        }
    }
}

pub async fn read<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Command>, &'static str> {
    let Some(frame) = read_frame_zeroizing(reader).await? else {
        return Ok(None);
    };
    let envelope: Envelope = serde_json::from_slice(&frame).map_err(|_| "invalid-command")?;
    if envelope.v != 1 {
        return Err("unsupported-version");
    }
    let body = match envelope.kind.as_str() {
        "init" => CommandBody::Init(Box::new(
            serde_json::from_slice(&frame).map_err(|_| "invalid-command")?,
        )),
        "pairing.create" | "pairing.approve" | "pairing.reject" | "pairing.cancel" => {
            CommandBody::Pairing(serde_json::from_slice(&frame).map_err(|_| "invalid-command")?)
        }
        "device.update" | "device.revoke" | "device.unlock" => {
            CommandBody::Device(serde_json::from_slice(&frame).map_err(|_| "invalid-command")?)
        }
        "shutdown" => {
            let _: ShutdownFrame = serde_json::from_slice(&frame).map_err(|_| "invalid-command")?;
            CommandBody::Shutdown
        }
        _ => CommandBody::Unknown,
    };
    Ok(Some(Command {
        v: envelope.v,
        body,
    }))
}

/// Shares host wire's bound (exported by protocol, not wire). All owned frame
/// allocations, including replaced buffers and error/cancellation paths, zeroize.
/// This does NOT erase serde_json's unescaped-string scratch, tokio stdin's
/// blocking buffer, or the caller's BufReader buffers. No whole-process memory
/// erasure is promised; PEMs must never be logged or persisted.
pub async fn read_frame_zeroizing<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Zeroizing<Vec<u8>>>, &'static str> {
    let mut frame = Zeroizing::new(Vec::new());
    loop {
        let available = reader.fill_buf().await.map_err(|_| "invalid-frame")?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("invalid-frame")
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(available.len());
        let required = frame
            .len()
            .checked_add(count)
            .filter(|len| *len <= MAX_FRAME_BYTES)
            .ok_or("invalid-frame")?;
        if required > frame.capacity() {
            let capacity = required
                .max(frame.capacity().saturating_mul(2))
                .min(MAX_FRAME_BYTES);
            let mut replacement = Zeroizing::new(Vec::with_capacity(capacity));
            replacement.extend_from_slice(&frame);
            // Never let Vec reallocate a secret-bearing allocation. Assignment
            // drops/zeroizes the old owner before its allocation is freed.
            frame = replacement;
        }
        let completed = available[count - 1] == b'\n';
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if completed {
            return Ok(Some(frame));
        }
    }
}

const EVENT_QUEUE_CAPACITY: usize = 64;
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
// Ordinary write audits/open announcements cannot consume the final 16 slots.
const AUDIT_SLOTS: usize = EVENT_QUEUE_CAPACITY - 16;
// Optional input buckets cannot starve mandatory pre-Host announcements.
const BUCKET_RESERVATIONS: usize = 12;
const ANNOUNCEMENT_RESERVATIONS: usize = 4;
struct QueuedEvent {
    event: Event,
    written: Option<oneshot::Sender<()>>,
    _audit: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[derive(Clone)]
pub struct EventSender {
    sender: mpsc::Sender<QueuedEvent>,
    audit_slots: std::sync::Arc<tokio::sync::Semaphore>,
    bucket_reservations: std::sync::Arc<tokio::sync::Semaphore>,
    announcement_reservations: std::sync::Arc<tokio::sync::Semaphore>,
}
pub(crate) struct EventReservation {
    permit: mpsc::OwnedPermit<QueuedEvent>,
    audit: tokio::sync::OwnedSemaphorePermit,
    _reservation: tokio::sync::OwnedSemaphorePermit,
}
impl EventReservation {
    pub(crate) fn emit(self, event: Event) {
        self.permit.send(QueuedEvent {
            event,
            written: None,
            _audit: Some(self.audit),
        });
    }
}
impl EventSender {
    pub(crate) fn reserve_bucket(&self) -> Option<EventReservation> {
        self.reserve(&self.bucket_reservations)
    }
    pub(crate) fn reserve_announcement(&self) -> Option<EventReservation> {
        self.reserve(&self.announcement_reservations)
    }
    fn reserve(
        &self,
        reservations: &std::sync::Arc<tokio::sync::Semaphore>,
    ) -> Option<EventReservation> {
        let reservation = reservations.clone().try_acquire_owned().ok()?;
        let audit = self.audit_slots.clone().try_acquire_owned().ok()?;
        let permit = self.sender.clone().try_reserve_owned().ok()?;
        Some(EventReservation {
            permit,
            audit,
            _reservation: reservation,
        })
    }
    pub(crate) fn available(&self) -> bool {
        !self.sender.is_closed()
            && self.sender.capacity() > 0
            && self.audit_slots.available_permits() > 0
    }
    /// HTTP handlers never wait on stdout. True means enqueued, NOT written;
    /// a full or closed queue returns false without submitting the event.
    pub fn try_emit(&self, event: Event) -> bool {
        let ordinary = match &event.body {
            EventBody::Audit { action, .. } => !matches!(
                action.as_str(),
                "terminal.release" | "full.close" | "expiry" | "revoke"
            ),
            EventBody::ControlState { open, .. } | EventBody::FullState { open, .. } => *open,
            EventBody::AuditDropped { .. } => true,
            _ => false,
        };
        let audit = if ordinary {
            let Ok(permit) = self.audit_slots.clone().try_acquire_owned() else {
                return false;
            };
            Some(permit)
        } else {
            None
        };
        self.sender
            .try_send(QueuedEvent {
                event,
                written: None,
                _audit: audit,
            })
            .is_ok()
    }

    /// Lifecycle events in main only: wait for enqueue and the writer's flush
    /// ACK. Only the writer times out IO; queue delay is not a broken stdout.
    pub async fn emit(&self, event: Event) -> bool {
        let (written, ack) = oneshot::channel();
        if self
            .sender
            .send(QueuedEvent {
                event,
                written: Some(written),
                _audit: None,
            })
            .await
            .is_err()
        {
            return false;
        }
        ack.await.is_ok()
    }

    /// The writer owns the receiver; its termination wakes gateway supervision.
    pub async fn closed(&self) {
        self.sender.closed().await;
    }
}

/// The only stdout owner. FIFO queue order (and each producer's order) is
/// preserved; concurrent producers never write NDJSON bytes themselves.
pub fn spawn_writer<W: AsyncWrite + Unpin + Send + 'static>(
    mut writer: W,
) -> (EventSender, JoinHandle<std::io::Result<()>>) {
    let (sender, mut events) = mpsc::channel::<QueuedEvent>(EVENT_QUEUE_CAPACITY);
    let task = tokio::spawn(async move {
        while let Some(queued) = events.recv().await {
            timeout(WRITE_TIMEOUT, send(&mut writer, queued.event))
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "Admin write timed out")
                })??;
            if let Some(written) = queued.written {
                let _ = written.send(());
            }
        }
        Ok(())
    });
    (
        EventSender {
            sender,
            audit_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(AUDIT_SLOTS)),
            bucket_reservations: std::sync::Arc::new(tokio::sync::Semaphore::new(
                BUCKET_RESERVATIONS,
            )),
            announcement_reservations: std::sync::Arc::new(tokio::sync::Semaphore::new(
                ANNOUNCEMENT_RESERVATIONS,
            )),
        },
        task,
    )
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, event: Event) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(&event)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}
