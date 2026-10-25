use crate::{
    admin::{AdminResult, Event, EventSender, PairingCommand},
    clock::{Clock, Timestamp},
    devices::{decode, valid_label},
    http::try_emit_event,
    secrets,
};
use axum::http::StatusCode;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{watch, Semaphore};
use zeroize::Zeroizing;

type Failure = (StatusCode, &'static str);
const INVALID: Failure = (StatusCode::FORBIDDEN, "Pairing request rejected");
const BUSY: Failure = (StatusCode::TOO_MANY_REQUESTS, "Pairing limit reached");
const TTL: Duration = Duration::from_secs(300);
const TICKET_TTL: Duration = Duration::from_secs(120);
const REGISTRATION_WINDOW: Duration = Duration::from_secs(600);
const MAX_LIVE: usize = 4;
const MAX_RECORDS: usize = 16;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimRequest {
    pub pairing_id: String,
    pub secret: String,
    pub device_label: String,
    pub device_nonce: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusRequest {
    pub pairing_id: String,
    pub claim_token: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResponse {
    pub gateway_nonce: String,
    pub sas: String,
    pub claim_token: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Approved,
    Rejected,
    Expired,
    Registering,
    Registered,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_ticket: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Registration {
    pub pairing_id: String,
    pub device_label: String,
    pub add_to_device: Option<String>,
    pub started_at: Timestamp,
    pub deadline: Timestamp,
}
struct Ticket {
    hash: [u8; 32],
    until: Timestamp,
}
struct Pairing {
    add_to_device: Option<String>,
    secret: Option<Zeroizing<[u8; 32]>>,
    secret_hash: [u8; 32],
    claim_hash: Option<[u8; 32]>,
    label: Option<String>,
    failures: u8,
    state: State,
    until: Timestamp,
    changed: watch::Sender<()>,
    polls: Arc<Semaphore>,
    ticket: Option<Ticket>,
    registration: Option<Registration>,
    notification: Option<&'static str>,
}
impl Pairing {
    fn live(&self) -> bool {
        matches!(
            self.state,
            State::Pending | State::Approved | State::Registering
        )
    }
    fn transition(&mut self, state: State) {
        self.state = state;
        if state != State::Registering {
            self.registration = None;
        }
        if state != State::Approved {
            self.ticket = None;
        }
        if state != State::Pending {
            self.secret = None;
        }
        self.changed.send_replace(());
    }
    fn expire(&mut self, now: Timestamp) {
        if self.live()
            && now
                >= self
                    .registration
                    .as_ref()
                    .map_or(self.until, |r| r.deadline)
        {
            self.transition(State::Expired);
            self.notification = Some("ttl");
        }
    }
    fn notify(&mut self, id: &str, events: &EventSender) -> Result<(), Failure> {
        if let Some(reason) = self.notification {
            let state = if self.state == State::Expired {
                "expired"
            } else {
                "rejected"
            };
            try_emit_event(events, Event::pairing_state(id.into(), state, reason))?;
            self.notification = None;
        }
        Ok(())
    }
    fn fail(&mut self, id: &str, events: &EventSender, second_claim: bool) -> Result<(), Failure> {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= 5 || second_claim {
            self.transition(State::Rejected);
            self.notification = Some(if second_claim {
                "second-claim"
            } else {
                "attempt-limit"
            });
            // Invalidation is fail-closed even if stdout is saturated. Retry the
            // notification on maintenance, but never restore the pairing.
            self.notify(id, events)?;
        }
        Err(INVALID)
    }
}

pub struct Pairings {
    entries: Mutex<HashMap<String, Pairing>>,
    polls: Arc<Semaphore>,
    origin: String,
    login: String,
    clock: Clock,
    events: EventSender,
}
impl Pairings {
    pub fn new(public_host: &str, login: &str, clock: Clock, events: EventSender) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            polls: Arc::new(Semaphore::new(8)),
            origin: format!("https://{public_host}"),
            login: login.into(),
            clock,
            events,
        }
    }
    pub fn maintenance(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            for (id, entry) in entries.iter_mut() {
                entry.expire(self.clock.now());
                let _ = entry.notify(id, &self.events);
            }
        }
    }
    pub fn command(&self, command: PairingCommand) -> Result<AdminResult, Failure> {
        self.maintenance();
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        if let PairingCommand::Create {
            path,
            add_to_device,
            ..
        } = command
        {
            if add_to_device
                .as_ref()
                .is_some_and(|id| decode::<16>(id).is_none())
            {
                return Err(INVALID);
            }
            if path == "lan" {
                return Err((StatusCode::BAD_REQUEST, "LAN pairing not supported"));
            }
            if path != "tailscale" {
                return Err((StatusCode::BAD_REQUEST, "Invalid pairing path"));
            }
            if entries.values().filter(|p| p.live()).count() >= MAX_LIVE {
                return Err(BUSY);
            }
            if entries.len() >= MAX_RECORDS {
                let oldest = entries
                    .iter()
                    .filter(|(_, p)| !p.live() && p.notification.is_none())
                    .min_by_key(|(_, p)| p.until)
                    .map(|(id, _)| id.clone())
                    .ok_or(BUSY)?;
                entries.remove(&oldest);
            }
            let id = secrets::encode(secrets::random::<16>().map_err(entropy)?.as_ref());
            let secret = secrets::random::<32>().map_err(entropy)?;
            let until = self.clock.now().after(TTL);
            let url = format!(
                "{}/pair#p={id}.{}",
                self.origin,
                secrets::encode(secret.as_ref())
            );
            try_emit_event(&self.events, Event::pairing_created(id.clone(), url, until))?;
            entries.insert(
                id,
                Pairing {
                    add_to_device,
                    secret_hash: secrets::hash(secret.as_ref()),
                    secret: Some(secret),
                    claim_hash: None,
                    label: None,
                    failures: 0,
                    state: State::Pending,
                    until,
                    changed: watch::channel(()).0,
                    polls: Arc::new(Semaphore::new(1)),
                    ticket: None,
                    registration: None,
                    notification: None,
                },
            );
            return Ok(AdminResult::Applied);
        }
        let (id, state, reason) = match command {
            PairingCommand::Approve { pairing_id, .. } => (pairing_id, State::Approved, "approved"),
            PairingCommand::Reject { pairing_id, .. } => (pairing_id, State::Rejected, "rejected"),
            PairingCommand::Cancel { pairing_id, .. } => (pairing_id, State::Rejected, "cancelled"),
            PairingCommand::Create { .. } => unreachable!(),
        };
        if decode::<16>(&id).is_none() {
            return Err(INVALID);
        }
        let entry = entries.get_mut(&id).ok_or(INVALID)?;
        Self::decide(entry, &id, state, reason, self.clock.now(), &self.events)
    }
    fn decide(
        entry: &mut Pairing,
        id: &str,
        state: State,
        reason: &'static str,
        now: Timestamp,
        events: &EventSender,
    ) -> Result<AdminResult, Failure> {
        // The maintenance lock was released: recheck this entry at commit time.
        entry.expire(now);
        if !entry.live()
            || (state == State::Approved
                && (entry.state != State::Pending || entry.claim_hash.is_none()))
        {
            return Err(INVALID);
        }
        if state == State::Approved {
            try_emit_event(events, Event::pairing_state(id.into(), "approved", reason))?;
            entry.until = now.after(REGISTRATION_WINDOW);
            entry.transition(state);
        } else {
            entry.transition(state);
            entry.notification = Some(reason);
            if entry.notify(id, events).is_err() {
                return Ok(AdminResult::NotificationDeferred {
                    command: if reason == "cancelled" {
                        "pairing.cancel"
                    } else {
                        "pairing.reject"
                    },
                    target: id.into(),
                });
            }
        }
        Ok(AdminResult::Applied)
    }
    pub fn claim(&self, request: ClaimRequest, user_agent: &str) -> Result<ClaimResponse, Failure> {
        if decode::<16>(&request.pairing_id).is_none()
            || !valid_label(&request.device_label)
            || user_agent.len() > 256
            || !user_agent.bytes().all(|b| (32..=126).contains(&b))
        {
            return Err(INVALID);
        }
        let device_nonce = decode::<32>(&request.device_nonce).ok_or(INVALID)?;
        let supplied = decode::<32>(&request.secret);
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        let entry = entries.get_mut(&request.pairing_id).ok_or(INVALID)?;
        entry.expire(self.clock.now());
        entry.notify(&request.pairing_id, &self.events)?;
        if !entry.live() {
            return Err(INVALID);
        }
        // Every structurally valid attempt traverses the same SHA-256 + subtle
        // comparison, including malformed secret encodings. No string equality.
        let equal = secrets::matches(&entry.secret_hash, &supplied.unwrap_or([0; 32]));
        if entry.claim_hash.is_some() {
            entry.fail(&request.pairing_id, &self.events, true)?;
            unreachable!();
        }
        if !equal || supplied.is_none() {
            entry.fail(&request.pairing_id, &self.events, false)?;
            unreachable!();
        }
        let gateway_nonce = secrets::random::<32>().map_err(entropy)?;
        let token = secrets::random::<32>().map_err(entropy)?;
        let sas = derive_sas(
            entry.secret.as_ref().ok_or(INVALID)?.as_ref(),
            &device_nonce,
            gateway_nonce.as_ref(),
        );
        try_emit_event(
            &self.events,
            Event::pairing_claimed(
                request.pairing_id,
                request.device_label.clone(),
                sas.clone(),
                self.login.clone(),
                user_agent.into(),
            ),
        )?;
        entry.claim_hash = Some(secrets::hash(token.as_ref()));
        entry.label = Some(request.device_label);
        entry.secret = None;
        Ok(ClaimResponse {
            gateway_nonce: secrets::encode(gateway_nonce.as_ref()),
            sas,
            claim_token: secrets::encode(token.as_ref()),
        })
    }
    fn authenticate(
        entry: &mut Pairing,
        request: &StatusRequest,
        events: &EventSender,
    ) -> Result<(), Failure> {
        let token = decode::<32>(&request.claim_token);
        let valid = secrets::matches(
            &entry.claim_hash.unwrap_or([0; 32]),
            &token.unwrap_or([0; 32]),
        );
        if !valid || token.is_none() || entry.claim_hash.is_none() {
            if entry.live() {
                entry.fail(&request.pairing_id, events, false)?;
            }
            return Err(INVALID);
        }
        Ok(())
    }
    pub async fn status(&self, request: StatusRequest) -> Result<StatusResponse, Failure> {
        if decode::<16>(&request.pairing_id).is_none() {
            return Err(INVALID);
        }
        let (mut changed, _pair_permit, _global_permit, until) = {
            let mut entries = self.entries.lock().map_err(|_| INVALID)?;
            let entry = entries.get_mut(&request.pairing_id).ok_or(INVALID)?;
            entry.expire(self.clock.now());
            Self::authenticate(entry, &request, &self.events)?;
            let pair = entry.polls.clone().try_acquire_owned().map_err(|_| BUSY)?;
            let global = self.polls.clone().try_acquire_owned().map_err(|_| BUSY)?;
            (entry.changed.subscribe(), pair, global, entry.until)
        };
        // Subscribe under the same lock as decisions; recheck before waiting.
        let pending = self
            .entries
            .lock()
            .map_err(|_| INVALID)?
            .get(&request.pairing_id)
            .is_some_and(|p| p.state == State::Pending);
        if pending {
            tokio::select! {
                _ = changed.changed() => {},
                _ = self.clock.until(until) => {},
                _ = tokio::time::sleep(Duration::from_secs(25)) => {},
            }
        }
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        let entry = entries.get_mut(&request.pairing_id).ok_or(INVALID)?;
        entry.expire(self.clock.now());
        Self::authenticate(entry, &request, &self.events)?;
        entry.notify(&request.pairing_id, &self.events)?;
        let registration_ticket = if entry.state == State::Approved {
            let ticket = secrets::random::<32>().map_err(entropy)?;
            entry.ticket = Some(Ticket {
                hash: secrets::hash(ticket.as_ref()),
                until: self.clock.now().after(TICKET_TTL).min(entry.until),
            });
            Some(secrets::encode(ticket.as_ref()))
        } else {
            None
        };
        Ok(StatusResponse {
            state: entry.state,
            registration_ticket,
        })
    }
    /// 2B's options handler consumes BEFORE creating a WebAuthn challenge.
    /// Only one current ticket exists; each approved status rotates it atomically.
    pub fn consume_ticket(&self, pairing_id: &str, ticket: &str) -> Result<Registration, Failure> {
        let supplied = decode::<32>(ticket).ok_or(INVALID)?;
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        let entry = entries.get_mut(pairing_id).ok_or(INVALID)?;
        entry.expire(self.clock.now());
        let current = entry.ticket.as_ref().ok_or(INVALID)?;
        let equal = secrets::matches(&current.hash, &supplied);
        if !equal || self.clock.now() >= current.until || entry.state != State::Approved {
            return Err(INVALID);
        }
        let started_at = self.clock.now();
        let registration = Registration {
            pairing_id: pairing_id.into(),
            device_label: entry.label.clone().ok_or(INVALID)?,
            add_to_device: entry.add_to_device.clone(),
            started_at,
            deadline: started_at.after(TICKET_TTL).min(entry.until),
        };
        entry.registration = Some(registration.clone());
        entry.transition(State::Registering);
        Ok(registration)
    }
    /// Options failed before publishing a challenge. Preserve the original
    /// approval window and never resurrect cancelled/expired/completed pairings.
    pub(crate) fn retry_registration(&self, registration: &Registration) -> Result<(), Failure> {
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        let entry = entries.get_mut(&registration.pairing_id).ok_or(INVALID)?;
        entry.expire(self.clock.now());
        if entry.state == State::Registering && entry.registration.as_ref() == Some(registration) {
            entry.transition(State::Approved);
        }
        Ok(())
    }
    /// Holds pairing then identity (inside commit), rechecking cancellation and
    /// expiry after cryptographic verification, before any widening event/commit.
    pub(crate) fn finish<T>(
        &self,
        registration: &Registration,
        commit: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let mut entries = self.entries.lock().map_err(|_| INVALID)?;
        let entry = entries.get_mut(&registration.pairing_id).ok_or(INVALID)?;
        entry.expire(self.clock.now());
        let current = entry.registration.as_ref().ok_or(INVALID)?;
        if entry.state != State::Registering
            || current.started_at != registration.started_at
            || current.deadline != registration.deadline
            || current.device_label != registration.device_label
            || current.add_to_device != registration.add_to_device
        {
            return Err(INVALID);
        }
        let result = commit()?;
        entry.transition(State::Registered);
        Ok(result)
    }
    #[cfg(test)]
    fn complete_registration(&self, registration: &Registration) -> Result<(), Failure> {
        self.finish(registration, || Ok(()))
    }
}
#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;

fn entropy(_: &'static str) -> Failure {
    (StatusCode::SERVICE_UNAVAILABLE, "Pairing unavailable")
}

/// Read digest bytes left-to-right, discard 250..255, map 0..249 to digit b%10.
/// Each decimal digit therefore has exactly 25 preimages (no modulo bias).
/// Extremely rare insufficient output extends with HMAC(message || u32-BE counter).
/// Nonces are the decoded 32-byte values, not their base64url text.
pub fn derive_sas(secret: &[u8], device_nonce: &[u8], gateway_nonce: &[u8]) -> String {
    let mut result = String::with_capacity(6);
    for counter in 0u32.. {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
        mac.update(b"yz-sas-v1");
        mac.update(device_nonce);
        mac.update(gateway_nonce);
        if counter != 0 {
            mac.update(&counter.to_be_bytes());
        }
        for byte in mac.finalize().into_bytes() {
            if byte < 250 {
                result.push(char::from(b'0' + byte % 10));
                if result.len() == 6 {
                    return result;
                }
            }
        }
    }
    unreachable!()
}
