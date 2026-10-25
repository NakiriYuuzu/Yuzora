//! Ceremony orchestration. No lock is held across an await. Registration commit
//! exclusively uses Pairings::finish: pairing -> identity. Authorization closures
//! must never acquire pairing/ceremony locks or return an async/future decision.
use crate::{
    admin::{Event, Scope},
    gateway::Gateway,
    secrets,
    session::{AssertionResult, AuthContext},
    webauthn::{self, AssertionState, Ceremony, CeremonyKind, Failure, REJECTED, UNAVAILABLE},
};
use serde_json::{json, Value};
use std::time::Duration;
impl Gateway {
    pub(crate) fn registration_options(
        &self,
        request: webauthn::RegistrationOptions,
    ) -> Result<Value, Failure> {
        let pairing = self
            .pairings
            .consume_ticket(&request.pairing_id, &request.registration_ticket)?;
        let result = (|| {
            let (id, existing) = if let Some(target) = &pairing.add_to_device {
                let identity = self.identity.lock().map_err(|_| UNAVAILABLE)?;
                (
                    target.clone(),
                    identity.registration_target(target)?.credentials.clone(),
                )
            } else {
                (
                    secrets::encode(secrets::random::<16>().map_err(|_| UNAVAILABLE)?.as_ref()),
                    Vec::new(),
                )
            };
            let (state, options) = self.rp.registration(
                &id,
                &pairing.device_label,
                &existing,
                pairing.add_to_device.is_none(),
            )?;
            self.ceremonies.lock().map_err(|_| UNAVAILABLE)?.insert(
                Ceremony {
                    until: pairing.deadline,
                    kind: CeremonyKind::Registration {
                        pairing: pairing.clone(),
                        device_id: id,
                        state,
                    },
                },
                self.clock.now(),
            )?;
            Ok(options)
        })();
        if result.is_err() {
            self.pairings.retry_registration(&pairing)?;
        }
        result
    }
    pub(crate) fn registration_finish(
        &self,
        request: webauthn::RegistrationFinish,
    ) -> Result<Value, Failure> {
        let result = (|| {
            let ceremony = self
                .ceremonies
                .lock()
                .map_err(|_| UNAVAILABLE)?
                .take_registration(&request.pairing_id, &request.credential, self.clock.now())?;
            let CeremonyKind::Registration {
                pairing,
                device_id,
                state,
            } = ceremony.kind
            else {
                return Err(REJECTED);
            };
            let verified = self.rp.verify_registration(
                state,
                request.credential,
                self.policy.passkey_binding,
            )?;
            let id = self.pairings.finish(&pairing, || {
                let mut identity = self.identity.lock().map_err(|_| UNAVAILABLE)?;
                identity.register_verified(&pairing, verified, &self.events)
            })?;
            debug_assert_eq!(id, device_id);
            Ok(json!({"deviceId": id}))
        })();
        self.outcome(
            "registration",
            result.is_ok(),
            result
                .as_ref()
                .ok()
                .and_then(|v| v["deviceId"].as_str().map(str::to_owned)),
        );
        result
    }
    pub(crate) fn login_options(&self) -> Result<Value, Failure> {
        let (state, options) = self.rp.login()?;
        self.ceremonies.lock().map_err(|_| UNAVAILABLE)?.insert(
            Ceremony {
                until: self
                    .clock
                    .now()
                    .after(Duration::from_secs(120))
                    .min(self.policy.enable_until),
                kind: CeremonyKind::Login(state),
            },
            self.clock.now(),
        )?;
        Ok(options)
    }
    pub(crate) fn login_finish(
        &self,
        request: webauthn::LoginFinish,
    ) -> Result<AssertionResult, Failure> {
        let ceremony = self
            .ceremonies
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .take_assertion(&request.credential, None, self.clock.now());
        let mut identity = self.identity.lock().map_err(|_| UNAVAILABLE)?;
        let result = match ceremony {
            Ok(Ceremony {
                kind: CeremonyKind::Login(state),
                ..
            }) => identity.finish_assertion(
                &self.rp,
                AssertionState::Login(state),
                request.credential,
                None,
                &self.events,
            ),
            _ => Err(REJECTED),
        };
        let device_id = match &result {
            Ok(AssertionResult::LoggedIn { device_id, .. }) => Some(device_id.clone()),
            _ => None,
        };
        self.outcome("login", result.is_ok(), device_id);
        result
    }
    pub(crate) fn elevation_options(
        &self,
        ctx: &AuthContext,
        scope: Scope,
    ) -> Result<Value, Failure> {
        // Only this dedicated options path nests identity -> ceremonies.
        // Global lock order: pairing -> identity -> ceremonies. Never reverse;
        // finish handlers release ceremonies before acquiring either other lock.
        // Do not acquire ceremonies from arbitrary with_authorized closures.
        let mut identity = self.identity.lock().map_err(|_| UNAVAILABLE)?;
        if ctx.rp_id() != self.rp_id || !identity.revalidate(ctx, Scope::View) {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                "Authentication required",
            ));
        }
        if !identity.can_elevate(ctx, scope) {
            return Err(webauthn::SCOPE_NOT_GRANTED);
        }
        let device = identity.device(ctx.device_id()).ok_or(REJECTED)?;
        let (state, options) =
            self.rp
                .elevation(&device.credentials, self.policy.passkey_binding, scope)?;
        let active = identity.active_session_ids();
        let mut ceremonies = self.ceremonies.lock().map_err(|_| UNAVAILABLE)?;
        ceremonies.retain_sessions(&active);
        ceremonies.insert(
            Ceremony {
                until: self
                    .clock
                    .now()
                    .after(Duration::from_secs(120))
                    .min(self.policy.enable_until),
                kind: CeremonyKind::Elevation {
                    session: *ctx.session_id(),
                    scope,
                    state,
                },
            },
            self.clock.now(),
        )?;
        Ok(options)
    }
    pub(crate) fn elevation_finish(
        &self,
        ctx: &AuthContext,
        request: webauthn::ElevationFinish,
    ) -> Result<Value, Failure> {
        let ceremony = self
            .ceremonies
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .take_assertion(
                &request.credential,
                Some((ctx.session_id(), request.scope)),
                self.clock.now(),
            );
        let result = self
            .with_authorized(ctx, Scope::View, |identity| {
                if !identity.can_elevate(ctx, request.scope) {
                    return Err(webauthn::SCOPE_NOT_GRANTED);
                }
                let state = match ceremony {
                    Ok(Ceremony {
                        kind: CeremonyKind::Elevation { state, .. },
                        ..
                    }) => state,
                    _ => {
                        return Err(REJECTED);
                    }
                };
                match identity.finish_assertion(
                    &self.rp,
                    AssertionState::Elevation(state),
                    request.credential,
                    Some((ctx, request.scope)),
                    &self.events,
                )? {
                    AssertionResult::Elevated(until) => {
                        Ok(json!({"scope": request.scope, "until": until}))
                    }
                    _ => Err(REJECTED),
                }
            })
            .and_then(|v| v);
        self.outcome(
            "elevation",
            result.is_ok(),
            result.is_ok().then(|| ctx.device_id().into()),
        );
        result
    }
    fn outcome(&self, ceremony: &'static str, success: bool, device_id: Option<String>) {
        // Best-effort informational outcome. Security/persistence snapshots use
        // the commit-first deferred queue, never this lossy informational event.
        self.events.try_emit(Event::auth_outcome(
            ceremony,
            if success { "succeeded" } else { "rejected" },
            device_id,
        ));
    }
}
