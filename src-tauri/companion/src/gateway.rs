use crate::{
    admin::{AdminResult, CommandBody, EventSender, Init, Policy, Scope},
    clock::Clock,
    guard::Guard,
    pairing::Pairings,
    session::{AuthContext, Identity},
};
use axum::http::{HeaderMap, StatusCode};
use std::sync::{Arc, Mutex};

pub struct Gateway {
    pub policy: Policy,
    pub clock: Clock,
    pub pairings: Pairings,
    pub identity: Mutex<Identity>,
    pub(crate) rp_id: String,
    pub(crate) rp: crate::webauthn::RelyingParty,
    pub(crate) ceremonies: Mutex<crate::webauthn::Ceremonies>,
    pub(crate) events: EventSender,
    pub(crate) herdr: Arc<crate::herdr::Reads>,
    pub(crate) herdr_jobs: Arc<crate::herdr_jobs::Jobs>,
    pub(crate) deferred_audit: Arc<Mutex<crate::audit::Deferred>>,
    pub(crate) full_devices:
        Mutex<std::collections::HashMap<String, std::sync::Weak<tokio::sync::Semaphore>>>,
}
impl Gateway {
    pub fn new(init: &Init, events: EventSender, clock: Clock) -> Result<Arc<Self>, &'static str> {
        Self::with_herdr(
            init,
            events,
            clock,
            Arc::new(crate::herdr::LocalHerdr::new(init.herdr_binary.clone())),
        )
    }
    pub(crate) fn with_herdr(
        init: &Init,
        events: EventSender,
        clock: Clock,
        port: Arc<dyn crate::herdr::HerdrPort>,
    ) -> Result<Arc<Self>, &'static str> {
        Guard::new(&init.listeners.loopback)?;
        if !init.herdr_binary.is_absolute() {
            return Err("invalid-herdr-binary");
        }
        if !init.policy.valid() {
            return Err("invalid-policy");
        }
        if clock.now() >= init.policy.enable_until {
            return Err("policy-expired");
        }
        let config = &init.listeners.loopback;
        Ok(Arc::new(Self {
            policy: init.policy.clone(),
            clock: clock.clone(),
            identity: Mutex::new(Identity::new(
                init.devices.clone(),
                init.policy.clone(),
                clock.clone(),
            )?),
            pairings: Pairings::new(
                &config.public_host,
                &config.required_tailscale_login,
                clock,
                events.clone(),
            ),
            rp_id: config
                .public_host
                .split(':')
                .next()
                .expect("validated authority")
                .into(),
            rp: crate::webauthn::RelyingParty::new(&config.public_host)?,
            ceremonies: Mutex::new(crate::webauthn::Ceremonies::new(init.policy.max_devices)),
            events,
            herdr: Arc::new(crate::herdr::Reads::new(port, &init.session_allowlist)),
            herdr_jobs: crate::herdr_jobs::Jobs::new(),
            deferred_audit: Arc::new(Mutex::new(crate::audit::Deferred::default())),
            full_devices: Mutex::new(std::collections::HashMap::new()),
        }))
    }
    pub fn command(&self, command: CommandBody) -> Result<AdminResult, (StatusCode, &'static str)> {
        if self.clock.now() >= self.policy.enable_until {
            return Err((StatusCode::FORBIDDEN, "Policy expired"));
        }
        match command {
            CommandBody::Pairing(command) => {
                if let crate::admin::PairingCommand::Create {
                    add_to_device: Some(id),
                    ..
                } = &command
                {
                    self.identity
                        .lock()
                        .map_err(|_| crate::webauthn::UNAVAILABLE)?
                        .registration_target(id)?;
                }
                self.pairings.command(command)
            }
            CommandBody::Device(command) => self
                .identity
                .lock()
                .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Device unavailable"))?
                .command(command, &self.events),
            _ => Err((StatusCode::BAD_REQUEST, "Invalid admin command")),
        }
    }
    pub fn maintenance(&self) {
        self.herdr_jobs.reap();
        self.deferred_audit.lock().unwrap().flush(&self.events);
        self.pairings.maintenance();
        if let Ok(mut ceremonies) = self.ceremonies.lock() {
            ceremonies.purge(self.clock.now());
        }
        if let Ok(mut identity) = self.identity.lock() {
            identity.maintenance(&self.events);
        }
    }
    pub fn authenticate(&self, headers: &HeaderMap, scope: Scope) -> Option<AuthContext> {
        self.identity
            .lock()
            .ok()?
            .authenticate(headers, &self.rp_id, scope)
    }
    /// Revalidate and make the synchronous write decision under the identity lock.
    /// The closure MUST be synchronous (never async or return a future). It must not
    /// defer the decision into a future, reenter identity, or acquire pairing /
    /// ceremonies locks. Leaf herdr_jobs queue/permit locks are permitted; they
    /// never call back into identity. Registration lock order is pairing then identity.
    /// Phase 3 must also actively close revoked WS peers with 4401; its first
    /// frame carries the binding (never a URL), before accepting any RPC.
    pub fn with_authorized<T>(
        &self,
        ctx: &AuthContext,
        scope: Scope,
        write: impl FnOnce(&mut Identity) -> T,
    ) -> Result<T, (StatusCode, &'static str)> {
        let denied = (StatusCode::UNAUTHORIZED, "Authentication required");
        let mut identity = self.identity.lock().map_err(|_| denied)?;
        if ctx.rp_id() != self.rp_id || !identity.revalidate(ctx, Scope::View) {
            return Err(denied);
        }
        if scope != Scope::View && !identity.can_elevate(ctx, scope) {
            return Err(crate::webauthn::SCOPE_NOT_GRANTED);
        }
        if !identity.revalidate(ctx, scope) {
            return Err((StatusCode::FORBIDDEN, "Elevation required"));
        }
        Ok(write(&mut identity))
    }
    pub fn logout(&self, ctx: &AuthContext) -> Result<(), (StatusCode, &'static str)> {
        self.with_authorized(ctx, Scope::View, |identity| {
            identity.logout(ctx.session_id())
        })
    }
}
