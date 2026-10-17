//! All cryptographic ceremony verification is delegated to pinned webauthn_rp.
//! Its persistence encoding is versioned, not COSE; upgrading requires migration
//! or re-registration. Never log browser credentials, challenges or bindings.
use crate::{
    admin::{PasskeyBinding, Scope},
    clock::Timestamp,
    devices::{CredentialRecord, Transport},
    pairing::Registration as PairingRegistration,
    secrets,
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Borrow,
    collections::{HashMap, VecDeque},
    num::NonZeroU32,
};
use webauthn_rp::{
    bin::{Decode, Encode},
    request::{
        auth::{
            self, AuthenticationVerificationOptions, AuthenticatorAttachmentEnforcement,
            DiscoverableAuthenticationServerState, NonDiscoverableAuthenticationServerState,
            SignatureCounterEnforcement,
        },
        register::{self, RegistrationServerState, RegistrationVerificationOptions, UserHandle16},
        AsciiDomain, BackupReq, Challenge, CredentialMediationRequirement, Credentials, Hint,
        PublicKeyCredentialDescriptor, RpId, UserVerificationRequirement,
    },
    response::{
        auth::{
            error::AuthCeremonyErr,
            ser_relaxed::{
                DiscoverableAuthenticationRelaxed16, NonDiscoverableAuthenticationRelaxed16,
            },
        },
        register::{ser_relaxed::RegistrationRelaxed, CompressedPubKey, DynamicState, StaticState},
        AuthenticatorAttachment, Backup, CredentialId, SentChallenge,
    },
    AuthenticatedCredential,
};

pub(crate) type Failure = (StatusCode, &'static str);
pub(crate) const REJECTED: Failure = (StatusCode::FORBIDDEN, "WebAuthn request rejected");
pub(crate) const UNAVAILABLE: Failure = (
    StatusCode::SERVICE_UNAVAILABLE,
    "Authentication unavailable",
);
pub(crate) const SCOPE_NOT_GRANTED: Failure = (StatusCode::FORBIDDEN, "Scope not granted");
pub const STATE_FORMAT: &str = "webauthn_rp-0.3.0-static";
// The library's largest encoding is 2058 bytes (16384-bit RSA plus extensions).
pub const MAX_KEY_STATE_BYTES: usize = 2058;
pub(crate) type KeyState = StaticState<CompressedPubKey<[u8; 32], [u8; 32], [u8; 48], Vec<u8>>>;
pub(crate) fn key_state(value: &str) -> Option<KeyState> {
    if value.len() > MAX_KEY_STATE_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = B64.decode(value).ok()?;
    if bytes.len() > MAX_KEY_STATE_BYTES {
        return None;
    }
    let state = KeyState::decode(&bytes).ok()?;
    let key: CompressedPubKey<&[u8], &[u8], &[u8], &[u8]> = (&state.credential_public_key).into();
    key.validate().ok()?;
    if matches!(key, CompressedPubKey::P384(_)) {
        return None;
    }
    Some(state)
}
fn user(id: &str) -> Result<UserHandle16, Failure> {
    UserHandle16::decode(crate::devices::decode::<16>(id).ok_or(REJECTED)?).map_err(|_| REJECTED)
}
pub(crate) fn requires_unbacked(policy: PasskeyBinding, scope: Scope) -> bool {
    policy == PasskeyBinding::O1 || (policy == PasskeyBinding::O2 && scope != Scope::View)
}
fn backup_requirement(policy: PasskeyBinding, scope: Scope) -> BackupReq {
    if requires_unbacked(policy, scope) {
        BackupReq::NotEligible
    } else {
        BackupReq::None
    }
}
fn flags(backup: Backup) -> (bool, bool) {
    match backup {
        Backup::NotEligible => (false, false),
        Backup::Eligible => (true, false),
        Backup::Exists => (true, true),
    }
}
fn policy_accepts(policy: PasskeyBinding, scope: Scope, be: bool, bs: bool) -> bool {
    (!bs || be) && (!requires_unbacked(policy, scope) || !be)
}
fn descriptor(
    record: &CredentialRecord,
) -> Result<PublicKeyCredentialDescriptor<Vec<u8>>, Failure> {
    Ok(PublicKeyCredentialDescriptor {
        id: CredentialId::decode(B64.decode(&record.id).map_err(|_| REJECTED)?)
            .map_err(|_| REJECTED)?,
        transports: serde_json::from_value(
            serde_json::to_value(&record.transports).map_err(|_| REJECTED)?,
        )
        .map_err(|_| REJECTED)?,
    })
}
pub(crate) struct RelyingParty {
    pub id: RpId,
    pub host: String,
    pub origin: String,
}
impl RelyingParty {
    pub fn new(public_host: &str) -> Result<Self, &'static str> {
        let host = public_host
            .split(':')
            .next()
            .ok_or("invalid-host")?
            .to_owned();
        if !crate::devices::valid_rp_id(&host) {
            return Err("invalid-host");
        }
        Ok(Self {
            id: RpId::Domain(AsciiDomain::try_from(host.clone()).map_err(|_| "invalid-host")?),
            host,
            origin: format!("https://{public_host}"),
        })
    }
    pub fn registration(
        &self,
        device_id: &str,
        label: &str,
        existing: &[CredentialRecord],
        first: bool,
    ) -> Result<(RegistrationServerState<16>, Value), Failure> {
        let user = user(device_id)?;
        let options = webauthn_rp::PublicKeyCredentialCreationOptions {
            mediation: CredentialMediationRequirement::Optional,
            rp_id: &self.id,
            user: register::PublicKeyCredentialUserEntity {
                name: register::Username::try_from("yuzora-companion").map_err(|_| UNAVAILABLE)?,
                id: &user,
                // Nickname applies stricter RFC8266 normalization than our label
                // contract. Omit rather than alter the label or reject pairing.
                display_name: register::Nickname::try_from(label)
                    .ok()
                    .filter(|name| name.as_ref() == label),
            },
            challenge: Challenge::new(),
            pub_key_cred_params: register::CoseAlgorithmIdentifiers::ALL
                .remove(register::CoseAlgorithmIdentifier::Es384),
            timeout: NonZeroU32::new(120_000).expect("nonzero"),
            exclude_credentials: existing
                .iter()
                .filter(|c| c.rp_id == self.host)
                .map(descriptor)
                .collect::<Result<_, _>>()?,
            authenticator_selection: register::AuthenticatorSelectionCriteria {
                authenticator_attachment: register::AuthenticatorAttachmentReq::None(Hint::None),
                resident_key: if first {
                    register::ResidentKeyRequirement::Required
                } else {
                    register::ResidentKeyRequirement::Preferred
                },
                user_verification: UserVerificationRequirement::Required,
            },
            extensions: register::Extension {
                cred_props: None,
                cred_protect: register::CredProtect::None,
                min_pin_length: None,
                prf: None,
            },
        };
        let (server, client) = options.start_ceremony().map_err(|_| UNAVAILABLE)?;
        let json = serde_json::to_value(client).map_err(|_| UNAVAILABLE)?;
        Ok((server, json)) // library serializer explicitly emits attestation:none
    }
    fn request(&self) -> auth::PublicKeyCredentialRequestOptions<'_, 'static> {
        auth::PublicKeyCredentialRequestOptions {
            challenge: Challenge::new(),
            timeout: NonZeroU32::new(120_000).expect("nonzero"),
            rp_id: &self.id,
            user_verification: UserVerificationRequirement::Required,
            hints: Hint::None,
            extensions: auth::Extension { prf: None },
        }
    }
    pub fn login(&self) -> Result<(DiscoverableAuthenticationServerState, Value), Failure> {
        let (server, client) = webauthn_rp::DiscoverableCredentialRequestOptions(self.request())
            .start_ceremony()
            .map_err(|_| UNAVAILABLE)?;
        Ok((
            server,
            serde_json::to_value(client).map_err(|_| UNAVAILABLE)?,
        ))
    }
    pub fn elevation(
        &self,
        records: &[CredentialRecord],
        policy: PasskeyBinding,
        scope: Scope,
    ) -> Result<(NonDiscoverableAuthenticationServerState, Value), Failure> {
        let mut allowed = auth::AllowedCredentials::with_capacity(records.len());
        for record in records.iter().filter(|c| {
            c.rp_id == self.host && policy_accepts(policy, scope, c.backup_eligible, c.backup_state)
        }) {
            allowed.push(auth::AllowedCredential {
                credential: descriptor(record)?,
                extension: auth::CredentialSpecificExtension { prf: None },
            });
        }
        let mut options =
            webauthn_rp::NonDiscoverableCredentialRequestOptions::second_factor(&self.id, allowed)
                .map_err(|_| REJECTED)?;
        *options.options() = self.request();
        let (server, client) = options.start_ceremony().map_err(|_| UNAVAILABLE)?;
        Ok((
            server,
            serde_json::to_value(client).map_err(|_| UNAVAILABLE)?,
        ))
    }
    pub fn verify_registration(
        &self,
        server: RegistrationServerState<16>,
        credential: Value,
        policy: PasskeyBinding,
    ) -> Result<VerifiedRegistration, Failure> {
        let response = serde_json::from_value::<RegistrationRelaxed>(credential)
            .map_err(|_| REJECTED)?
            .0;
        let registered = server
            .verify(
                &self.id,
                &response,
                &RegistrationVerificationOptions::<&str, &str> {
                    allowed_origins: &[&self.origin],
                    allowed_top_origins: None,
                    backup_requirement: backup_requirement(policy, Scope::View),
                    error_on_unsolicited_extensions: true,
                    require_authenticator_attachment: false,
                    client_data_json_relaxed: true,
                },
            )
            .map_err(|_| REJECTED)?;
        let dynamic = registered.dynamic_state();
        let (be, bs) = flags(dynamic.backup);
        if !dynamic.user_verified || !policy_accepts(policy, Scope::View, be, bs) {
            return Err(REJECTED);
        }
        let transports: Vec<Transport> = serde_json::from_value(
            serde_json::to_value(registered.transports()).map_err(|_| REJECTED)?,
        )
        .map_err(|_| REJECTED)?;
        Ok(VerifiedRegistration {
            device_id: secrets::encode(registered.user_id().as_ref()),
            record: CredentialRecord {
                id: secrets::encode(registered.id().as_ref()),
                rp_id: self.host.clone(),
                public_key_state: secrets::encode(
                    &registered.static_state().encode().map_err(|_| REJECTED)?,
                ),
                state_format: STATE_FORMAT.into(),
                sign_count: dynamic.sign_count,
                backup_eligible: be,
                backup_state: bs,
                transports,
            },
        })
    }
    pub fn verify_assertion(
        &self,
        server: AssertionState,
        response: Value,
        device_id: &str,
        record: &CredentialRecord,
        policy: PasskeyBinding,
        scope: Scope,
    ) -> Result<VerifiedAssertion, AssertionFailure> {
        if record.rp_id != self.host {
            return Err(AssertionFailure::Invalid);
        }
        let user = user(device_id).map_err(|_| AssertionFailure::Invalid)?;
        let id = B64
            .decode(&record.id)
            .map_err(|_| AssertionFailure::Invalid)?;
        let state = key_state(&record.public_key_state).ok_or(AssertionFailure::Invalid)?;
        let mut credential = AuthenticatedCredential::new(
            CredentialId::decode(id.as_slice()).map_err(|_| AssertionFailure::Invalid)?,
            &user,
            state,
            DynamicState {
                user_verified: true,
                backup: if record.backup_state {
                    Backup::Exists
                } else if record.backup_eligible {
                    Backup::Eligible
                } else {
                    Backup::NotEligible
                },
                sign_count: record.sign_count,
                authenticator_attachment: AuthenticatorAttachment::None,
            },
        )
        .map_err(|_| AssertionFailure::Invalid)?;
        let origins = [self.origin.as_str()];
        let options = AuthenticationVerificationOptions::<&str, &str> {
            allowed_origins: &origins,
            allowed_top_origins: None,
            backup_requirement: if requires_unbacked(policy, scope) {
                Some(BackupReq::NotEligible)
            } else {
                None
            },
            error_on_unsolicited_extensions: true,
            auth_attachment_enforcement: AuthenticatorAttachmentEnforcement::Ignore(false),
            update_uv: true,
            sig_counter_enforcement: SignatureCounterEnforcement::Fail,
            client_data_json_relaxed: true,
        };
        let result = match server {
            AssertionState::Login(server) => {
                let response =
                    serde_json::from_value::<DiscoverableAuthenticationRelaxed16>(response)
                        .map_err(|_| AssertionFailure::Invalid)?
                        .0;
                server.verify(&self.id, &response, &mut credential, &options)
            }
            AssertionState::Elevation(server) => {
                let response =
                    serde_json::from_value::<NonDiscoverableAuthenticationRelaxed16>(response)
                        .map_err(|_| AssertionFailure::Invalid)?
                        .0;
                server.verify(&self.id, &response, &mut credential, &options)
            }
        };
        result.map_err(AssertionFailure::from)?;
        let dynamic = credential.dynamic_state();
        let (be, bs) = flags(dynamic.backup);
        // Policy is checked AGAIN on the verified flags, not merely stored metadata.
        if !dynamic.user_verified
            || !policy_accepts(policy, scope, be, bs)
            || be != record.backup_eligible
        {
            return Err(AssertionFailure::Policy);
        }
        Ok(VerifiedAssertion {
            sign_count: dynamic.sign_count,
            backup_state: bs,
        })
    }
}
pub(crate) struct VerifiedRegistration {
    device_id: String,
    record: CredentialRecord,
}
impl VerifiedRegistration {
    pub fn into_parts(self) -> (String, CredentialRecord) {
        (self.device_id, self.record)
    }
}
pub(crate) struct VerifiedAssertion {
    sign_count: u32,
    backup_state: bool,
}
impl VerifiedAssertion {
    pub fn values(&self) -> (u32, bool) {
        (self.sign_count, self.backup_state)
    }
}
pub(crate) enum AssertionFailure {
    Invalid,
    Policy,
    Rollback,
}
impl From<AuthCeremonyErr> for AssertionFailure {
    fn from(error: AuthCeremonyErr) -> Self {
        use webauthn_rp::response::auth::error::ExtensionErr;
        // Pinned 0.3.0: request.rs:1386-1497 verifies the signature BEFORE
        // these policy checks; request/auth.rs:1282-1294 checks extensions/counter.
        // An explicit allowlist fails safe for pre-verification or new errors.
        match error {
            AuthCeremonyErr::SignatureCounter => Self::Rollback,
            AuthCeremonyErr::OriginMismatch
            | AuthCeremonyErr::CrossOrigin
            | AuthCeremonyErr::TopOriginMismatch
            | AuthCeremonyErr::RpIdHashMismatch
            | AuthCeremonyErr::UserNotVerified
            | AuthCeremonyErr::BackupEligible
            | AuthCeremonyErr::BackupNotEligible
            | AuthCeremonyErr::BackupDoesNotExist
            | AuthCeremonyErr::Extension(ExtensionErr::ForbiddenHmacSecret
                // We never request PRF. With prf=None this variant can only
                // mean unsolicited signed hmac-secret output (auth.rs:691/713).
                | ExtensionErr::HmacSecretForPrfIncapableCred) => Self::Policy,
            // ChallengeMismatch is Invalid: stale/replaced sheets are not a
            // policy violation. Challenge-keyed lookup normally rejects them
            // before verification, without consuming the replacement ceremony.
            // Includes attachment/credential-state errors (not our policy),
            // and all non-unsolicited extension errors. Never count ambiguity.
            _ => Self::Invalid,
        }
    }
}
pub(crate) enum AssertionState {
    Login(DiscoverableAuthenticationServerState),
    Elevation(NonDiscoverableAuthenticationServerState),
}
pub(crate) enum CeremonyKind {
    Registration {
        pairing: PairingRegistration,
        device_id: String,
        state: RegistrationServerState<16>,
    },
    Login(DiscoverableAuthenticationServerState),
    Elevation {
        session: [u8; 32],
        scope: Scope,
        state: NonDiscoverableAuthenticationServerState,
    },
}
pub(crate) struct Ceremony {
    pub until: Timestamp,
    pub kind: CeremonyKind,
}
pub(crate) struct Ceremonies {
    entries: HashMap<SentChallenge, Ceremony>,
    elevation_limit: usize,
    login_order: VecDeque<SentChallenge>,
}
const LOGIN_LIMIT: usize = 48;
const REGISTRATION_LIMIT: usize = 16;
impl Ceremonies {
    pub fn new(max_devices: u32) -> Self {
        Self {
            entries: HashMap::new(),
            elevation_limit: max_devices as usize * 4,
            login_order: VecDeque::new(),
        }
    }
    pub fn retain_sessions(&mut self, active: &std::collections::HashSet<[u8; 32]>) {
        self.entries.retain(|_, c| match &c.kind {
            CeremonyKind::Elevation { session, .. } => active.contains(session),
            _ => true,
        });
    }
    pub fn purge(&mut self, now: Timestamp) {
        self.entries.retain(|_, c| now < c.until);
        self.login_order
            .retain(|key| self.entries.contains_key(key));
    }
    pub fn insert(&mut self, ceremony: Ceremony, now: Timestamp) -> Result<(), Failure> {
        self.purge(now);
        // Independent pools: anonymous options cannot displace registration or
        // elevation. FIFO remains exact even when deadlines/timestamps coincide.
        match &ceremony.kind {
            CeremonyKind::Login(_) => {
                if self.login_order.len() >= LOGIN_LIMIT {
                    let oldest = self.login_order.pop_front().expect("full login pool");
                    self.entries.remove(&oldest);
                }
            }
            CeremonyKind::Elevation { session, .. } => {
                self.entries.retain(|_, c| !matches!(&c.kind, CeremonyKind::Elevation { session: old, .. } if old == session));
                if self.entries.values().filter(|c| matches!(c.kind, CeremonyKind::Elevation { .. })).count() >= self.elevation_limit {
                    return Err((StatusCode::TOO_MANY_REQUESTS, "Challenge limit reached"));
                }
            }
            CeremonyKind::Registration { pairing, .. } => {
                if self.entries.values().filter(|c| matches!(c.kind, CeremonyKind::Registration { .. })).count() >= REGISTRATION_LIMIT
                    || self.entries.values().any(|c| matches!(&c.kind, CeremonyKind::Registration { pairing: old, .. } if old.pairing_id == pairing.pairing_id)) {
                    return Err((StatusCode::TOO_MANY_REQUESTS, "Challenge limit reached"));
                }
            }
        }
        let key = match &ceremony.kind {
            CeremonyKind::Registration { state, .. } => *state.borrow(),
            CeremonyKind::Login(state) => *state.borrow(),
            CeremonyKind::Elevation { state, .. } => *state.borrow(),
        };
        if matches!(ceremony.kind, CeremonyKind::Login(_)) {
            self.login_order.push_back(key);
        }
        self.entries.insert(key, ceremony);
        Ok(())
    }
    pub fn take_registration(
        &mut self,
        pairing_id: &str,
        response: &Value,
        now: Timestamp,
    ) -> Result<Ceremony, Failure> {
        self.purge(now);
        let key = serde_json::from_value::<RegistrationRelaxed>(response.clone())
            .map_err(|_| REJECTED)?
            .0
            .challenge_relaxed()
            .map_err(|_| REJECTED)?;
        if !self.entries.get(&key).is_some_and(|c| {
            matches!(&c.kind,
            CeremonyKind::Registration { pairing, .. } if pairing.pairing_id == pairing_id)
        }) {
            return Err(REJECTED);
        }
        self.entries.remove(&key).ok_or(REJECTED)
    }
    pub fn take_assertion(
        &mut self,
        response: &Value,
        session: Option<(&[u8; 32], Scope)>,
        now: Timestamp,
    ) -> Result<Ceremony, Failure> {
        self.purge(now);
        let key = if session.is_some() {
            serde_json::from_value::<NonDiscoverableAuthenticationRelaxed16>(response.clone())
                .map_err(|_| REJECTED)?
                .0
                .challenge_relaxed()
                .map_err(|_| REJECTED)?
        } else {
            serde_json::from_value::<DiscoverableAuthenticationRelaxed16>(response.clone())
                .map_err(|_| REJECTED)?
                .0
                .challenge_relaxed()
                .map_err(|_| REJECTED)?
        };
        // Bind challenge + kind + session/scope BEFORE consuming. An old sheet
        // must not consume a replacement or count against the device's lockout.
        let ceremony = self.entries.get(&key).ok_or(REJECTED)?;
        let valid = match (&ceremony.kind, session) {
            (CeremonyKind::Login(_), None) => true,
            (CeremonyKind::Elevation { session, scope, .. }, Some((id, requested))) => {
                session == id && *scope == requested
            }
            _ => false,
        };
        if !valid {
            return Err(REJECTED);
        }
        // Once correctly bound, verification failures still consume single use.
        self.entries.remove(&key).ok_or(REJECTED)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistrationOptions {
    pub pairing_id: String,
    pub registration_ticket: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistrationFinish {
    pub pairing_id: String,
    pub credential: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginOptions {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginFinish {
    pub credential: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ElevationOptions {
    pub scope: Scope,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ElevationFinish {
    pub scope: Scope,
    pub credential: Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginResponse {
    pub binding: String,
    pub device_id: String,
    pub scopes: [Scope; 1],
}

#[cfg(test)]
#[path = "webauthn_tests.rs"]
mod tests;
