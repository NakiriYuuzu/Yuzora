use crate::{
    admin::{AdminResult, DeviceCommand, Event, EventSender, Policy, Scope},
    clock::{Clock, Timestamp},
    devices::{decode, valid_label, Device},
    http::try_emit_event,
};
use axum::http::{HeaderMap, StatusCode};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

pub const COOKIE_NAME: &str = "__Host-yzc";
pub const COOKIE_ATTRIBUTES: &str = "Secure; HttpOnly; SameSite=Strict; Path=/";
pub fn clear_cookie() -> String {
    format!("{COOKIE_NAME}=; {COOKIE_ATTRIBUTES}; Max-Age=0")
}
pub(crate) fn session_cookie(id: &[u8; 32]) -> String {
    format!(
        "{COOKIE_NAME}={}; {COOKIE_ATTRIBUTES}",
        crate::secrets::encode(id)
    )
}
pub fn cookie_id(headers: &HeaderMap) -> Option<[u8; 32]> {
    let mut result = None;
    for header in headers.get_all("cookie") {
        for pair in header.as_bytes().split(|b| *b == b';') {
            let pair = pair.trim_ascii();
            let split = pair.iter().position(|b| *b == b'=');
            let name = split.map_or(pair, |i| &pair[..i]);
            if name == COOKIE_NAME.as_bytes() {
                if result.is_some() {
                    return None;
                }
                let value = std::str::from_utf8(&pair[split? + 1..]).ok()?;
                result = Some(decode(value)?);
            }
        }
    }
    result
}

/// Opaque proof of transport authentication, not an enduring write authorization.
/// Fields are read-only outside this module; never log the session identifier.
pub struct AuthContext {
    session_id: [u8; 32],
    device_id: String,
    rp_id: String,
}
impl AuthContext {
    pub fn session_id(&self) -> &[u8; 32] {
        &self.session_id
    }
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
    pub fn rp_id(&self) -> &str {
        &self.rp_id
    }
}

struct Peer {
    session: [u8; 32],
    device: String,
    invalidated: tokio::sync::watch::Sender<bool>,
}

pub struct Session {
    pub device_id: String,
    pub rp_id: String,
    pub created_at: Timestamp,
    pub last_seen: Timestamp,
    pub control_until: Option<Timestamp>,
    pub full_until: Option<Timestamp>,
    control_at: Option<Timestamp>,
    full_at: Option<Timestamp>,
    binding_hash: [u8; 32],
}
/// Devices and sessions share one lock, so grant updates/revocation cannot race validation.
pub struct Identity {
    devices: HashMap<String, Device>,
    // Latest snapshot per device; revoke supersedes all earlier snapshots.
    notifications: HashMap<String, Option<Device>>,
    sessions: HashMap<[u8; 32], Session>,
    peers: HashMap<u64, Peer>,
    next_peer: u64,
    failures: HashMap<String, u8>,
    dirty_devices: HashSet<String>,
    policy: Policy,
    clock: Clock,
}
impl Identity {
    pub fn new(devices: Vec<Device>, policy: Policy, clock: Clock) -> Result<Self, &'static str> {
        if !policy.valid()
            || devices.len() > policy.max_devices as usize
            || !devices.iter().all(Device::valid)
        {
            return Err("invalid-policy");
        }
        let count = devices.len();
        let devices: HashMap<_, _> = devices
            .into_iter()
            .map(|d| (d.device_id.clone(), d))
            .collect();
        if devices.len() != count {
            return Err("invalid-devices");
        }
        Ok(Self {
            devices,
            sessions: HashMap::new(),
            peers: HashMap::new(),
            next_peer: 0,
            failures: HashMap::new(),
            dirty_devices: HashSet::new(),
            notifications: HashMap::new(),
            policy,
            clock,
        })
    }
    /// Mutations must use commands, not direct table access.
    /// ```compile_fail
    /// fn bypass(identity: &mut yuzora_companion::session::Identity) {
    ///     identity.devices.clear();
    /// }
    /// ```
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.get(id)
    }
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }
    /// Pre-upgrade DoS filter, NOT authentication or a TTL refresh. Binding and
    /// current authorization are still required after upgrade.
    pub(crate) fn has_session_cookie(&self, headers: &HeaderMap) -> bool {
        cookie_id(headers).is_some_and(|id| self.sessions.contains_key(&id))
    }
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
    pub fn session_limit(&self) -> usize {
        self.policy.max_devices as usize * 4
    }
    // Called only inside Gateway::with_authorized; subscription and revalidation
    // share the identity lock so revoke cannot fall between them. The global
    // transport permit is held before registration, including unauthenticated WS.
    pub(crate) fn register_peer(
        &mut self,
        ctx: &AuthContext,
    ) -> Option<(u64, tokio::sync::watch::Receiver<bool>)> {
        if self.peers.len() >= crate::bridge::MAX_PEERS
            || self
                .peers
                .values()
                .filter(|p| p.device == ctx.device_id)
                .count()
                >= 2
        {
            return None;
        }
        let id = self.next_peer;
        self.next_peer = self.next_peer.checked_add(1)?;
        let (invalidated, receiver) = tokio::sync::watch::channel(false);
        self.peers.insert(
            id,
            Peer {
                session: ctx.session_id,
                device: ctx.device_id.clone(),
                invalidated,
            },
        );
        Some((id, receiver))
    }
    pub(crate) fn unregister_peer(&mut self, id: u64) {
        self.peers.remove(&id);
    }
    fn invalidate_session(&mut self, id: &[u8; 32]) {
        self.sessions.remove(id);
        for peer in self.peers.values().filter(|p| p.session == *id) {
            peer.invalidated.send_replace(true);
        }
    }
    fn invalidate_device(&mut self, device: &str) {
        let sessions: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| (s.device_id == device).then_some(*id))
            .collect();
        for id in sessions {
            self.invalidate_session(&id);
        }
    }
    pub fn purge_expired(&mut self) {
        let now = self.clock.now();
        let expired: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(id, s)| (!self.active(s, now)).then_some(*id))
            .collect();
        for id in expired {
            self.invalidate_session(&id);
        }
    }
    fn notification(id: &str, device: &Option<Device>) -> Event {
        match device {
            Some(device) => Event::device_updated(device.clone()),
            None => Event::device_revoked(id.into()),
        }
    }
    pub fn maintenance(&mut self, events: &EventSender) {
        self.purge_expired();
        for id in std::mem::take(&mut self.dirty_devices) {
            self.notify_snapshot(&id, events);
        }
        self.notifications
            .retain(|id, device| !events.try_emit(Self::notification(id, device)));
    }
    /// Cookies are hostname-, not port-scoped. The binding is returned ONLY in
    /// 2B's login body, kept in origin-scoped memory/sessionStorage, never a cookie.
    /// Phase 4 warns about same-host Serve handlers and refuses Funnel on ANY port.
    // The requested transport scope is not an authorization proof. Keep 401
    // (identity/binding) distinct from with_authorized's 403 (scope/elevation).
    pub fn authenticate(
        &mut self,
        headers: &HeaderMap,
        rp_id: &str,
        _requested_scope: Scope,
    ) -> Option<AuthContext> {
        let id = cookie_id(headers)?;
        let mut bindings = headers.get_all("x-yzc-binding").iter();
        let token = decode::<32>(bindings.next()?.to_str().ok()?)?;
        if bindings.next().is_some() {
            return None;
        }
        if !crate::secrets::matches(&self.sessions.get(&id)?.binding_hash, &token)
            || !self.validate(&id, rp_id, Scope::View)
        {
            return None;
        }
        Some(AuthContext {
            session_id: id,
            device_id: self.sessions.get(&id)?.device_id.clone(),
            rp_id: rp_id.into(),
        })
    }
    pub(crate) fn revalidate(&mut self, ctx: &AuthContext, scope: Scope) -> bool {
        self.sessions
            .get(&ctx.session_id)
            .is_some_and(|s| s.device_id == ctx.device_id)
            && self.validate(&ctx.session_id, &ctx.rp_id, scope)
    }
    fn active(&self, session: &Session, now: Timestamp) -> bool {
        now < self.policy.enable_until
            && now
                < session.created_at.after(Duration::from_secs(
                    u64::from(self.policy.view_absolute_hours) * 3600,
                ))
            && now
                < session.last_seen.after(Duration::from_secs(
                    u64::from(self.policy.view_idle_min) * 60,
                ))
            && self.devices.get(&session.device_id).is_some_and(|d| {
                !d.locked && d.credentials.iter().any(|c| c.rp_id == session.rp_id)
            })
    }
    pub(crate) fn validate(&mut self, id: &[u8; 32], rp_id: &str, scope: Scope) -> bool {
        let now = self.clock.now();
        let Some(session) = self.sessions.get(id) else {
            return false;
        };
        if !self.active(session, now) {
            self.invalidate_session(id);
            return false;
        }
        if session.rp_id != rp_id {
            return false;
        }
        let device = &self.devices[&session.device_id];
        let granted = match scope {
            Scope::View => true,
            Scope::Control => device
                .grants
                .control
                .as_ref()
                .is_some_and(|g| now < g.until),
            Scope::Full => self.policy.full_session_enabled && device.grants.full,
        };
        if !granted
            || (scope != Scope::View
                && (match scope {
                    Scope::Control => session.control_until,
                    Scope::Full => session.full_until,
                    Scope::View => None,
                })
                .is_none_or(|until| now >= until))
        {
            return false;
        }
        let session = self.sessions.get_mut(id).expect("validated session");
        session.last_seen = now;
        self.devices
            .get_mut(&session.device_id)
            .expect("validated device")
            .last_seen = now;
        self.dirty_devices.insert(session.device_id.clone());
        true
    }
    /// 2B calls only after successful fresh WebAuthn verification for this scope.
    fn elevate(&mut self, ctx: &AuthContext, scope: Scope) -> Option<Timestamp> {
        if !self.revalidate(ctx, Scope::View) {
            return None;
        }
        let id = ctx.session_id();
        let rp_id = ctx.rp_id();
        let now = self.clock.now();
        let session = self.sessions.get(id)?;
        if session.rp_id != rp_id || !self.active(session, now) {
            return None;
        }
        let device = self.devices.get(&session.device_id)?;
        let cap = match scope {
            Scope::View => return None,
            Scope::Control => device.grants.control.as_ref()?.until,
            Scope::Full if self.policy.full_session_enabled && device.grants.full => {
                now.after(Duration::from_secs(900))
            }
            Scope::Full => return None,
        };
        let until = now
            .after(Duration::from_secs(
                u64::from(self.policy.elevation_max_min) * 60,
            ))
            .min(cap)
            .min(self.policy.enable_until)
            .min(session.created_at.after(Duration::from_secs(
                u64::from(self.policy.view_absolute_hours) * 3600,
            )));
        if until <= now {
            return None;
        }
        let session = self.sessions.get_mut(id)?;
        match scope {
            Scope::Control => {
                session.control_until = Some(until);
                session.control_at = Some(now);
            }
            Scope::Full => {
                session.full_until = Some(until);
                session.full_at = Some(now);
            }
            Scope::View => unreachable!(),
        }
        Some(until)
    }
    /// Called under with_authorized. Freshness gates open/takeover only; an
    /// already-open stream retains its own immutable deadline, never renewed.
    pub(crate) fn write_deadline(
        &self,
        ctx: &AuthContext,
        scope: Scope,
        fresh: bool,
    ) -> Option<Timestamp> {
        let session = self.sessions.get(ctx.session_id())?;
        let (at, until) = match scope {
            Scope::Control => (session.control_at?, session.control_until?),
            Scope::Full => (session.full_at?, session.full_until?),
            Scope::View => return None,
        };
        if fresh && self.clock.now() > at.after(Duration::from_secs(120)) {
            return None;
        }
        Some(until)
    }
    pub(crate) fn freshness_deadline(&self, ctx: &AuthContext, scope: Scope) -> Option<Timestamp> {
        let s = self.sessions.get(ctx.session_id())?;
        let at = match scope {
            Scope::Control => s.control_at,
            Scope::Full => s.full_at,
            Scope::View => None,
        }?;
        Some(at.after(Duration::from_secs(120)))
    }
    pub(crate) fn logout(&mut self, id: &[u8; 32]) {
        self.invalidate_session(id);
    }
    pub fn command(
        &mut self,
        command: DeviceCommand,
        events: &EventSender,
    ) -> Result<AdminResult, (StatusCode, &'static str)> {
        let id = match &command {
            DeviceCommand::Update { device_id, .. }
            | DeviceCommand::Revoke { device_id, .. }
            | DeviceCommand::Unlock { device_id, .. } => device_id,
        };
        let invalid = (StatusCode::BAD_REQUEST, "Invalid device command");
        if decode::<16>(id).is_none() {
            return Err(invalid);
        }
        let Some(mut device) = self.devices.get(id).cloned() else {
            return Err(invalid);
        };
        let mut widening = false;
        let name = match &command {
            DeviceCommand::Update { label, grants, .. } => {
                if label.is_none() && grants.is_none() {
                    return Err(invalid);
                }
                if let Some(label) = label {
                    if !valid_label(label) {
                        return Err(invalid);
                    }
                    device.label = label.clone();
                }
                if let Some(grants) = grants {
                    // None < Some(until), shorter < longer; false < true.
                    // Mixed deltas must be split by desktop: tightening first.
                    let old = device.grants.control.as_ref().map(|g| g.until);
                    let new = grants.control.as_ref().map(|g| g.until);
                    let tightening = new < old || (!grants.full && device.grants.full);
                    widening = new > old || (grants.full && !device.grants.full);
                    if tightening && widening {
                        return Err(invalid);
                    }
                    device.grants = grants.clone();
                }
                "device.update"
            }
            DeviceCommand::Unlock { .. } => {
                device.locked = false;
                widening = true;
                "device.unlock"
            }
            DeviceCommand::Revoke { .. } => "device.revoke",
        };
        let snapshot = if matches!(command, DeviceCommand::Revoke { .. }) {
            None
        } else {
            Some(device)
        };
        // Never enqueue a widening snapshot ahead of a deferred older snapshot.
        if widening {
            if self.notifications.contains_key(id) {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Admin event queue unavailable",
                ));
            }
            try_emit_event(events, Self::notification(id, &snapshot))?;
        }
        self.dirty_devices.remove(id);
        if matches!(
            command,
            DeviceCommand::Unlock { .. } | DeviceCommand::Revoke { .. }
        ) {
            self.failures.remove(id);
        }
        match &snapshot {
            Some(device) => {
                self.devices.insert(id.clone(), device.clone());
            }
            None => {
                self.devices.remove(id);
            }
        }
        // Tightening (including future lockouts) and neutral label updates commit
        // under the identity lock BEFORE enqueue, clearing sessions/elevations.
        self.invalidate_device(id);
        if !widening {
            self.notifications.insert(id.clone(), snapshot);
            if events.try_emit(Self::notification(id, &self.notifications[id])) {
                self.notifications.remove(id);
            } else {
                return Ok(AdminResult::NotificationDeferred {
                    command: name,
                    target: id.clone(),
                });
            }
        }
        Ok(AdminResult::Applied)
    }
    #[cfg(test)]
    fn elevate_for_test(&mut self, id: &[u8; 32], rp_id: &str, scope: Scope) -> Option<Timestamp> {
        let ctx = AuthContext {
            session_id: *id,
            device_id: self.sessions.get(id)?.device_id.clone(),
            rp_id: rp_id.into(),
        };
        self.elevate(&ctx, scope)
    }
    /// Deliberately absent from production until 2B supplies verified credentials.
    #[cfg(test)]
    pub(crate) fn create_for_test(
        &mut self,
        device_id: &str,
        rp_id: &str,
    ) -> Option<([u8; 32], String)> {
        let now = self.clock.now();
        self.purge_expired();
        if self.sessions.len() >= self.session_limit() {
            return None;
        }
        let binding = crate::secrets::random::<32>().unwrap();
        let id = *crate::secrets::random::<32>().unwrap();
        self.sessions.insert(
            id,
            Session {
                device_id: device_id.into(),
                rp_id: rp_id.into(),
                created_at: now,
                last_seen: now,
                control_until: None,
                full_until: None,
                control_at: None,
                full_at: None,
                binding_hash: crate::secrets::hash(binding.as_ref()),
            },
        );
        Some((id, crate::secrets::encode(binding.as_ref())))
    }
}

impl Identity {
    pub(crate) fn registration_target(
        &self,
        id: &str,
    ) -> Result<&Device, crate::webauthn::Failure> {
        self.devices
            .get(id)
            .filter(|d| !d.locked && d.credentials.len() < 8)
            .ok_or(crate::webauthn::REJECTED)
    }
    pub(crate) fn register_verified(
        &mut self,
        pairing: &crate::pairing::Registration,
        verified: crate::webauthn::VerifiedRegistration,
        events: &EventSender,
    ) -> Result<String, crate::webauthn::Failure> {
        use crate::webauthn::REJECTED;
        let (id, record) = verified.into_parts();
        if self.clock.now() >= self.policy.enable_until || self.clock.now() >= pairing.deadline {
            return Err(REJECTED);
        }
        if self
            .devices
            .values()
            .flat_map(|d| &d.credentials)
            .any(|c| c.id == record.id && c.rp_id == record.rp_id)
        {
            return Err(REJECTED);
        }
        let added = pairing.add_to_device.is_some();
        let mut device = if let Some(target) = &pairing.add_to_device {
            if *target != id {
                return Err(REJECTED);
            }
            if self.notifications.contains_key(target) {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Admin event queue unavailable",
                ));
            }
            self.registration_target(target)?.clone()
        } else {
            // Refuse new identities while ANY deferred snapshot/tombstone exists.
            // Thus churn cannot make pending revocations exceed maxDevices.
            if !self.notifications.is_empty() {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Admin event queue unavailable",
                ));
            }
            if self.devices.len() >= self.policy.max_devices as usize
                || self.devices.contains_key(&id)
            {
                return Err(REJECTED);
            }
            let now = self.clock.now();
            Device {
                device_id: id.clone(),
                label: pairing.device_label.clone(),
                credentials: Vec::new(),
                grants: crate::devices::Grants {
                    control: None,
                    full: false,
                },
                locked: false,
                created_at: now,
                last_seen: now,
            }
        };
        device.credentials.push(record);
        if !device.valid() {
            return Err(REJECTED);
        }
        try_emit_event(
            events,
            if added {
                Event::device_updated(device.clone())
            } else {
                Event::device_registered(device.clone())
            },
        )?;
        self.devices.insert(id.clone(), device);
        self.invalidate_device(&id);
        Ok(id)
    }
    fn notify_snapshot(&mut self, id: &str, events: &EventSender) {
        self.dirty_devices.remove(id);
        if let Some(device) = self.devices.get(id) {
            self.notifications.insert(id.into(), Some(device.clone()));
            if events.try_emit(Self::notification(id, &self.notifications[id])) {
                self.notifications.remove(id);
            }
        }
    }
    pub(crate) fn assertion_owner(
        &self,
        response: &serde_json::Value,
        rp_id: &str,
    ) -> Option<String> {
        let id = response.get("rawId")?.as_str()?;
        self.devices
            .values()
            .find(|d| d.credentials.iter().any(|c| c.id == id && c.rp_id == rp_id))
            .map(|d| d.device_id.clone())
    }
    pub(crate) fn assertion_failed(&mut self, id: &str, rollback: bool, events: &EventSender) {
        let Some(device) = self.devices.get_mut(id) else {
            return;
        };
        if device.locked {
            return;
        }
        let count = self.failures.entry(id.into()).or_insert(0);
        *count = count.saturating_add(1);
        if rollback || *count >= 5 {
            device.locked = true;
            self.invalidate_device(id);
            // Tightening first; queue pressure cannot restore authentication.
            self.notify_snapshot(id, events);
        }
    }
    fn assertion_verified(
        &mut self,
        id: &str,
        credential_id: &str,
        rp_id: &str,
        verified: &crate::webauthn::VerifiedAssertion,
        events: &EventSender,
    ) {
        let device = self
            .devices
            .get_mut(id)
            .expect("verified device under lock");
        let record = device
            .credentials
            .iter_mut()
            .find(|c| c.id == credential_id && c.rp_id == rp_id)
            .expect("verified credential under lock");
        let (sign_count, backup_state) = verified.values();
        record.sign_count = sign_count;
        record.backup_state = backup_state;
        device.last_seen = self.clock.now();
        self.failures.remove(id);
        self.notify_snapshot(id, events); // neutral, commit first, coalesce latest
    }
    pub(crate) fn finish_assertion(
        &mut self,
        rp: &crate::webauthn::RelyingParty,
        state: crate::webauthn::AssertionState,
        response: serde_json::Value,
        ctx: Option<(&AuthContext, Scope)>,
        events: &EventSender,
    ) -> Result<AssertionResult, crate::webauthn::Failure> {
        use crate::webauthn::{AssertionFailure, REJECTED};
        let id = if let Some((ctx, _)) = ctx {
            ctx.device_id().to_owned()
        } else {
            let owner = self.assertion_owner(&response, &rp.host).ok_or(REJECTED)?;
            let handle = response["response"]["userHandle"].as_str();
            if handle != Some(owner.as_str()) {
                return Err(REJECTED);
            }
            // Discoverable userHandle selects the device; credential membership
            // must agree. The library independently checks this binding again.
            handle.ok_or(REJECTED)?.to_owned()
        };
        let scope = ctx.map_or(Scope::View, |(_, scope)| scope);
        let verification = (|| {
            let device = self
                .devices
                .get(&id)
                .filter(|d| !d.locked)
                .ok_or(AssertionFailure::Invalid)?;
            let record = device
                .credentials
                .iter()
                .find(|c| {
                    c.rp_id == rp.host
                        && Some(c.id.as_str())
                            == response.get("rawId").and_then(serde_json::Value::as_str)
                })
                .ok_or(AssertionFailure::Invalid)?;
            rp.verify_assertion(
                state,
                response.clone(),
                &id,
                record,
                self.policy.passkey_binding,
                scope,
            )
        })();
        let verified = match verification {
            Ok(verified) => verified,
            Err(error) => {
                if matches!(error, AssertionFailure::Policy | AssertionFailure::Rollback) {
                    self.assertion_failed(&id, matches!(error, AssertionFailure::Rollback), events);
                }
                return Err(REJECTED);
            }
        };
        self.assertion_verified(
            &id,
            response["rawId"].as_str().ok_or(REJECTED)?,
            &rp.host,
            &verified,
            events,
        );
        if let Some((ctx, scope)) = ctx {
            Ok(AssertionResult::Elevated(
                self.elevate(ctx, scope)
                    .ok_or(crate::webauthn::SCOPE_NOT_GRANTED)?,
            ))
        } else {
            let (session, binding) = self.issue_session(&id, &rp.host, verified)?;
            Ok(AssertionResult::LoggedIn {
                session,
                binding,
                device_id: id,
            })
        }
    }
    pub(crate) fn active_session_ids(&mut self) -> HashSet<[u8; 32]> {
        self.purge_expired();
        self.sessions.keys().copied().collect()
    }
    pub(crate) fn can_elevate(&self, ctx: &AuthContext, scope: Scope) -> bool {
        let now = self.clock.now();
        self.devices.get(ctx.device_id()).is_some_and(|d| {
            !d.locked
                && match scope {
                    Scope::View => false,
                    Scope::Control => d.grants.control.as_ref().is_some_and(|g| now < g.until),
                    Scope::Full => self.policy.full_session_enabled && d.grants.full,
                }
        })
    }
    // The only production insertion boundary requires an unforgeable verifier
    // result. Never accepts a request cookie or a caller-selected session ID.
    fn issue_session(
        &mut self,
        device_id: &str,
        rp_id: &str,
        _proof: crate::webauthn::VerifiedAssertion,
    ) -> Result<([u8; 32], String), crate::webauthn::Failure> {
        self.purge_expired();
        if self.clock.now() >= self.policy.enable_until {
            return Err(crate::webauthn::REJECTED);
        }
        let binding = crate::secrets::random::<32>().map_err(|_| crate::webauthn::UNAVAILABLE)?;
        let id = *crate::secrets::random::<32>().map_err(|_| crate::webauthn::UNAVAILABLE)?;
        if self
            .sessions
            .values()
            .filter(|s| s.device_id == device_id)
            .count()
            >= 4
        {
            let oldest = self
                .sessions
                .iter()
                .filter(|(_, s)| s.device_id == device_id)
                .min_by_key(|(_, s)| s.created_at)
                .map(|(id, _)| *id)
                .expect("four sessions");
            self.invalidate_session(&oldest);
        }
        if self.sessions.len() >= self.session_limit() {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "Session limit reached"));
        }
        let now = self.clock.now();
        self.sessions.insert(
            id,
            Session {
                device_id: device_id.into(),
                rp_id: rp_id.into(),
                created_at: now,
                last_seen: now,
                control_until: None,
                full_until: None,
                control_at: None,
                full_at: None,
                binding_hash: crate::secrets::hash(binding.as_ref()),
            },
        );
        Ok((id, crate::secrets::encode(binding.as_ref())))
    }
}
pub(crate) enum AssertionResult {
    LoggedIn {
        session: [u8; 32],
        binding: String,
        device_id: String,
    },
    Elevated(Timestamp),
}
