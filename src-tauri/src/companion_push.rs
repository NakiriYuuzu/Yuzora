//! Web Push for the Companion (plan Phase 7). The gateway only notices that an
//! agent is blocked or done and never talks to the network: this side keeps the
//! phones' subscriptions, encrypts each message (RFC 8291) and signs the request
//! with the VAPID key (RFC 8292). A subscription is a capability to notify a
//! phone: never log or emit one, its keys, the VAPID private key or a message.

use crate::companion_store::{device_id, read_versioned, write_json, Devices, VERSION};
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes128Gcm,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hkdf::Hkdf;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    PublicKey, SecretKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const PUSH_FILE: &str = "push.json";
const VAULT_SERVICE: &str = "yuzora.companion";
const VAULT_ACCOUNT: &str = "vapid-p256";
/// A pane that just caused a push of one kind does not cause another of that
/// kind for this long.
const COOLDOWN: Duration = Duration::from_secs(60);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Push services only accept these origins; so does the gateway.
const PUSH_HOSTS: [&str; 4] = [
    "push.apple.com",
    "fcm.googleapis.com",
    "updates.push.services.mozilla.com",
    "notify.windows.com",
];
const ENDPOINT_BYTES: usize = 2048;
/// Browsers a device may have turned notifications on in: a synced passkey
/// signs in on each of them as the same device.
const ENDPOINTS_PER_DEVICE: usize = 4;
/// The VAPID `sub` claim: where a push service can reach the application.
const SUBJECT: &str = "https://github.com/NakiriYuuzu/Yuzora";
const JWT_LIFETIME_SECS: u64 = 12 * 3600;
/// Seconds a push service keeps a message for a phone that is off.
const TTL_SECS: &str = "3600";
/// RFC 8188 record size: one record holds the whole message.
const RECORD_SIZE: u32 = 4096;
const ID_BYTES: usize = 256;
const TEXT_BYTES: usize = 200;
const MESSAGE_BYTES: usize = 3 * 1024;

/// Where the VAPID private scalar lives (base64url, 32 bytes).
pub trait Vault: Send + Sync {
    fn load(&self) -> Result<Option<Zeroizing<String>>, String>;
    fn store(&self, secret: &str) -> Result<(), String>;
}

/// The OS credential store, like the database credentials.
pub struct KeyringVault;
impl KeyringVault {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(VAULT_SERVICE, VAULT_ACCOUNT)
            .map_err(|_| "companion-push-vault-unavailable".to_string())
    }
}
impl Vault for KeyringVault {
    fn load(&self) -> Result<Option<Zeroizing<String>>, String> {
        match Self::entry()?.get_secret() {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|secret| Some(Zeroizing::new(secret)))
                .map_err(|_| "companion-push-vault-corrupt".to_string()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("companion-push-vault-unavailable".into()),
        }
    }
    fn store(&self, secret: &str) -> Result<(), String> {
        Self::entry()?
            .set_secret(secret.as_bytes())
            .map_err(|_| "companion-push-vault-write-failed".to_string())
    }
}

/// One HTTP request to a push service.
pub struct Request {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}
/// The HTTP status, or why there was none. The reason never holds the URL.
pub type Sent = Pin<Box<dyn Future<Output = Result<u16, String>> + Send>>;
pub type Sender = Arc<dyn Fn(Request) -> Sent + Send + Sync>;
/// Monotonic, so the cooldown does not follow wall-clock changes.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Sends with reqwest on the rustls provider the updater uses. A push service
/// never redirects a valid request, so a redirect is not followed.
pub fn http_sender() -> Sender {
    Arc::new(|request| {
        Box::pin(async move {
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
            let client = reqwest::Client::builder()
                .user_agent("Yuzora-Companion")
                .timeout(SEND_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "client".to_string())?;
            let mut call = client.post(&request.url).body(request.body);
            for (name, value) in request.headers {
                call = call.header(name, value);
            }
            call.send()
                .await
                .map(|response| response.status().as_u16())
                .map_err(|error| {
                    if error.is_timeout() {
                        "timeout"
                    } else if error.is_connect() {
                        "connect"
                    } else {
                        "request"
                    }
                    .to_string()
                })
        })
    })
}

/// Web Push as the settings UI sees it.
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushOverview {
    /// The VAPID key is loaded: the gateway got a push configuration.
    pub available: bool,
    /// Devices that get notifications.
    pub subscribed: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct PushFile {
    version: u32,
    /// Per device, the one subscribed longest ago first.
    subscriptions: BTreeMap<String, Vec<Subscription>>,
}

/// How the gateway names a subscription: base64url, unpadded, of the SHA-256
/// of its endpoint. The endpoint is a capability to notify a phone, so only
/// this leaves the desktop (the gateway computes the same from what a browser
/// asks about).
pub(crate) fn endpoint_id(endpoint: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(endpoint.as_bytes()))
}

/// A phone's `PushSubscription`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Subscription {
    endpoint: String,
    p256dh: String,
    auth: String,
    created_at: String,
}
impl Subscription {
    /// What a stored or reported subscription must be to be sent to: the
    /// gateway checks the same, and this side does not rely on it, because it
    /// is the one that makes the request.
    fn keys(&self) -> Option<(PublicKey, Vec<u8>)> {
        let url = reqwest::Url::parse(&self.endpoint).ok()?;
        let host = url.host_str()?;
        let allowed = PUSH_HOSTS
            .iter()
            .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")));
        if !(self.endpoint.len() <= ENDPOINT_BYTES
            && self.endpoint.bytes().all(|b| b.is_ascii_graphic())
            && url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && allowed)
        {
            return None;
        }
        let public = URL_SAFE_NO_PAD.decode(&self.p256dh).ok()?;
        let auth = URL_SAFE_NO_PAD.decode(&self.auth).ok()?;
        if public.len() != 65 || public[0] != 4 || auth.len() != 16 {
            return None;
        }
        Some((PublicKey::from_sec1_bytes(&public).ok()?, auth))
    }
}

/// The `subscription` of a `push.subscription` event.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reported {
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// The application server key pair. The private half never leaves this process.
struct Vapid {
    key: SigningKey,
    /// Uncompressed point, base64url: the phone's `applicationServerKey`.
    public: String,
}
impl Vapid {
    fn new(secret: &SecretKey) -> Self {
        Self {
            key: SigningKey::from(secret.clone()),
            public: URL_SAFE_NO_PAD.encode(uncompressed(&secret.public_key())),
        }
    }
    /// The stored key, or a new one, stored (true then). A key that is there
    /// but cannot be read is not replaced: every subscription was made for it.
    fn load(vault: &dyn Vault) -> Result<(Self, bool), String> {
        let Some(stored) = vault.load()? else {
            let secret = random_secret();
            let text = Zeroizing::new(URL_SAFE_NO_PAD.encode(secret.to_bytes()));
            vault.store(&text)?;
            return Ok((Self::new(&secret), true));
        };
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(stored.as_bytes())
                .map_err(|_| "companion-push-vault-corrupt")?,
        );
        let secret = SecretKey::from_slice(&bytes)
            .map_err(|_| "companion-push-vault-corrupt".to_string())?;
        Ok((Self::new(&secret), false))
    }
}

/// What the gateway reports and the phone gets, nothing more.
#[derive(Serialize)]
struct Message {
    v: u8,
    kind: String,
    title: String,
    space: Option<String>,
    session: String,
    pane: String,
}
impl Message {
    /// The payload of a `push.request`; a request that does not look like one
    /// is not sent.
    fn from_event(payload: &Value) -> Option<Self> {
        let id = |field: &str| {
            payload[field]
                .as_str()
                .filter(|id| {
                    (1..=ID_BYTES).contains(&id.len()) && !id.chars().any(char::is_control)
                })
                .map(str::to_owned)
        };
        let kind = payload["kind"]
            .as_str()
            .filter(|kind| ["blocked", "done"].contains(kind))?;
        Some(Self {
            v: 1,
            kind: kind.to_owned(),
            title: clip(payload["title"].as_str()?),
            space: payload["space"].as_str().map(clip),
            session: id("session")?,
            pane: id("paneId")?,
        })
    }
}

/// Free text from an agent: no control characters, and short.
fn clip(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > TEXT_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

/// A push to send: the message once, and every subscription of the devices it
/// goes to.
pub struct Job {
    hold: Hold,
    message: Vec<u8>,
    urgency: &'static str,
    targets: Vec<(String, Subscription)>,
}

/// (session, pane, kind): a `blocked` is not held back by an earlier `done`.
type Key = (String, String, String);

struct State {
    subscriptions: BTreeMap<String, Vec<Subscription>>,
    /// When each pane last caused a push of each kind: the first send of it.
    sent: HashMap<Key, Instant>,
}

/// A job's key while it is on its way: until it is stamped, the same
/// transition would otherwise go out twice. Dropping the job frees it.
struct Hold {
    held: Arc<Mutex<HashSet<Key>>>,
    key: Key,
}
impl Drop for Hold {
    fn drop(&mut self) {
        self.held.lock().unwrap().remove(&self.key);
    }
}

pub struct Push {
    dir: PathBuf,
    vault: Arc<dyn Vault>,
    sender: Sender,
    clock: Clock,
    /// Loaded on the first enable, so the OS credential store is not touched
    /// until the Companion is used.
    vapid: Mutex<Option<Arc<Vapid>>>,
    state: Mutex<State>,
    held: Arc<Mutex<HashSet<Key>>>,
}

impl Push {
    pub fn new(dir: PathBuf, vault: Arc<dyn Vault>, sender: Sender, clock: Clock) -> Arc<Self> {
        let stored = match read_versioned::<PushFile>(&dir.join(PUSH_FILE), |file| file.version) {
            Ok(file) => file.map(|file| file.subscriptions).unwrap_or_default(),
            // Kept as a `.corrupt-` copy; the phones subscribe again.
            Err(code) => {
                log_warning("companion_push_load", &code);
                BTreeMap::new()
            }
        };
        Arc::new(Self {
            vault,
            sender,
            clock,
            vapid: Mutex::new(None),
            state: Mutex::new(State {
                subscriptions: retain_known(&dir, stored),
                sent: HashMap::new(),
            }),
            held: Arc::default(),
            dir,
        })
    }

    /// The production wiring.
    pub fn system(dir: PathBuf) -> Arc<Self> {
        Self::new(
            dir,
            Arc::new(KeyringVault),
            http_sender(),
            Arc::new(Instant::now),
        )
    }

    pub fn overview(&self) -> PushOverview {
        PushOverview {
            available: self.vapid.lock().unwrap().is_some(),
            subscribed: self.devices(),
        }
    }

    pub fn event(&self) -> Value {
        json!({"type": "push", "push": self.overview()})
    }

    /// Devices that have a subscription.
    pub fn devices(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .subscriptions
            .keys()
            .cloned()
            .collect()
    }

    /// What the gateway is told: each device with a subscription and the ids of
    /// its endpoints.
    pub fn listing(&self) -> Value {
        listing(&self.state.lock().unwrap().subscriptions)
    }

    /// The gateway's `push` init field: `null` when the key cannot be had.
    /// Blocks on the OS credential store.
    pub fn init(&self) -> Value {
        let vapid = match self.vapid() {
            Ok(vapid) => vapid,
            Err(code) => {
                log_warning("companion_push_unavailable", &code);
                return Value::Null;
            }
        };
        // A revoke made while no gateway ran left its subscription behind.
        let mut state = self.state.lock().unwrap();
        let kept = retain_known(&self.dir, state.subscriptions.clone());
        if count(&kept) != count(&state.subscriptions) {
            if let Err(code) = self.commit(&mut state, kept) {
                log_warning("companion_push_prune", &code);
            }
        }
        json!({
            "vapidPublicKey": vapid.public,
            "devices": listing(&state.subscriptions),
        })
    }

    fn vapid(&self) -> Result<Arc<Vapid>, String> {
        let mut loaded = self.vapid.lock().unwrap();
        if let Some(vapid) = loaded.as_ref() {
            return Ok(vapid.clone());
        }
        let (vapid, created) = Vapid::load(self.vault.as_ref())?;
        if created {
            // Every stored subscription was made for the key that is gone: the
            // push services would refuse each send, and nothing would say so.
            let mut state = self.state.lock().unwrap();
            if !state.subscriptions.is_empty() {
                log_warning("companion_push_key_replaced", "subscriptions dropped");
                self.saved(&mut state, BTreeMap::new());
            }
        }
        let vapid = Arc::new(vapid);
        *loaded = Some(vapid.clone());
        Ok(vapid)
    }

    /// `push.subscription` of a device that exists: adds the subscription of
    /// one more of its browsers, or renews the one with the same endpoint. A
    /// fifth endpoint takes the place of the one subscribed longest ago. False
    /// when nothing changed.
    pub fn subscription(&self, device: &str, reported: &Value) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut next = state.subscriptions.clone();
        let Ok(reported) = serde_json::from_value::<Reported>(reported.clone()) else {
            return false;
        };
        let subscription = Subscription {
            endpoint: reported.endpoint,
            p256dh: reported.p256dh,
            auth: reported.auth,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        if subscription.keys().is_none() {
            log_warning("companion_push_subscription", "invalid");
            return false;
        }
        let list = next.entry(device.to_owned()).or_default();
        list.push(subscription);
        *list = tidy(std::mem::take(list));
        self.saved(&mut state, next)
    }

    /// `push.subscription` with `null`: the browser whose endpoint has this id
    /// turned notifications off. The device's other browsers keep theirs.
    /// False when it had none such.
    pub fn unsubscribe(&self, device: &str, endpoint: &str) -> bool {
        self.remove(device, |subscription| {
            endpoint_id(&subscription.endpoint) == endpoint
        })
    }

    /// Removes the device's subscriptions that `gone` picks, deciding and
    /// writing under one lock.
    fn remove(&self, device: &str, gone: impl Fn(&Subscription) -> bool) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut next = state.subscriptions.clone();
        let Some(list) = next.get_mut(device) else {
            return false;
        };
        let before = list.len();
        list.retain(|subscription| !gone(subscription));
        if list.len() == before {
            return false;
        }
        if list.is_empty() {
            next.remove(device);
        }
        self.saved(&mut state, next)
    }

    /// A revoked device is gone, with all its subscriptions. False when it had
    /// none.
    pub fn forget(&self, device: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        let mut next = state.subscriptions.clone();
        next.remove(device).is_some() && self.saved(&mut state, next)
    }

    /// Disk first: memory only follows a write that worked.
    fn commit(
        &self,
        state: &mut State,
        next: BTreeMap<String, Vec<Subscription>>,
    ) -> Result<(), String> {
        write_json(
            &self.dir.join(PUSH_FILE),
            &PushFile {
                version: VERSION,
                subscriptions: next.clone(),
            },
        )?;
        state.subscriptions = next;
        Ok(())
    }

    fn saved(&self, state: &mut State, next: BTreeMap<String, Vec<Subscription>>) -> bool {
        match self.commit(state, next) {
            Ok(()) => true,
            Err(code) => {
                log_warning("companion_push_save", &code);
                false
            }
        }
    }

    /// `push.request`: the push to send, if there is one. A pane that caused a
    /// push of the same kind less than a minute ago causes none; the minute
    /// starts when `deliver` first tries a send, not here.
    pub fn request(&self, device_ids: &Value, payload: &Value) -> Option<Job> {
        self.vapid.lock().unwrap().as_ref()?;
        let message = Message::from_event(payload)?;
        let body = serde_json::to_vec(&message).ok()?;
        if body.len() > MESSAGE_BYTES {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        let devices: BTreeSet<&str> = device_ids
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let targets: Vec<_> = devices
            .into_iter()
            .filter_map(|id| Some((id, state.subscriptions.get(id)?)))
            .flat_map(|(id, list)| list.iter().map(move |s| (id.to_owned(), s.clone())))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let now = (self.clock)();
        state
            .sent
            .retain(|_, at| now.saturating_duration_since(*at) < COOLDOWN);
        let urgency = if message.kind == "blocked" {
            "high"
        } else {
            "normal"
        };
        let key = (message.session, message.pane, message.kind);
        // Held requests do not push the hold further out.
        if state.sent.contains_key(&key) || !self.held.lock().unwrap().insert(key.clone()) {
            return None;
        }
        Some(Job {
            hold: Hold {
                held: self.held.clone(),
                key,
            },
            message: body,
            urgency,
            targets,
        })
    }

    /// Sends a job to every subscription of its devices, all at once: a push
    /// service that does not answer holds up only its own request. A
    /// subscription its push service no longer knows is deleted, that one
    /// only; true then, so the gateway's endpoint set follows. Other failures
    /// are only logged, without the message.
    pub async fn deliver(&self, job: Job) -> bool {
        let Some(vapid) = self.vapid.lock().unwrap().clone() else {
            return false;
        };
        let mut requests = Vec::new();
        for (device, subscription) in job.targets {
            match build_request(&vapid, &subscription, &job.message, job.urgency) {
                Ok(request) => requests.push((device, subscription, request)),
                Err(code) => log_warning("companion_push_build", &code),
            }
        }
        // The hold starts before the first send, whatever its outcome; a job
        // that never got as far as sending leaves none.
        if requests.is_empty() {
            return false;
        }
        let now = (self.clock)();
        self.state
            .lock()
            .unwrap()
            .sent
            .insert(job.hold.key.clone(), now);
        let sends = requests
            .into_iter()
            .map(|(device, subscription, request)| async move {
                match (self.sender)(request).await {
                    Ok(200..=299) => false,
                    Ok(404 | 410) => self.expire(&device, &subscription),
                    Ok(status) => {
                        log_warning("companion_push_rejected", &format!("status {status}"));
                        false
                    }
                    Err(reason) => {
                        log_warning("companion_push_failed", &reason);
                        false
                    }
                }
            });
        futures_util::future::join_all(sends)
            .await
            .into_iter()
            .any(|gone| gone)
    }

    /// Deletes the subscription the push service reported as gone, unless the
    /// phone has since made a new one (another endpoint is another browser's;
    /// the same endpoint with other keys is a renewal, and the gone answer was
    /// for the keys it replaced).
    fn expire(&self, device: &str, sent: &Subscription) -> bool {
        self.remove(device, |stored| {
            stored.endpoint == sent.endpoint
                && stored.p256dh == sent.p256dh
                && stored.auth == sent.auth
        })
    }
}

/// A device's subscriptions as they may be kept: valid ones, each endpoint once
/// (the later one wins), the newest `ENDPOINTS_PER_DEVICE`.
fn tidy(list: Vec<Subscription>) -> Vec<Subscription> {
    let mut kept: Vec<Subscription> = Vec::new();
    for subscription in list.into_iter().filter(|s| s.keys().is_some()) {
        kept.retain(|other| other.endpoint != subscription.endpoint);
        kept.push(subscription);
    }
    kept.drain(..kept.len().saturating_sub(ENDPOINTS_PER_DEVICE));
    kept
}

/// Subscriptions of devices that still exist and still validate. When
/// devices.json cannot be read, no device is judged gone.
fn retain_known(
    dir: &Path,
    subscriptions: BTreeMap<String, Vec<Subscription>>,
) -> BTreeMap<String, Vec<Subscription>> {
    let devices = Devices::load(dir).ok();
    subscriptions
        .into_iter()
        .filter(|(id, _)| {
            devices
                .as_ref()
                .is_none_or(|devices| devices.all().iter().any(|d| device_id(d) == Some(id)))
        })
        .map(|(id, list)| (id, tidy(list)))
        .filter(|(_, list)| !list.is_empty())
        .collect()
}

fn count(subscriptions: &BTreeMap<String, Vec<Subscription>>) -> usize {
    subscriptions.values().map(Vec::len).sum()
}

/// `[{deviceId, endpoints: [endpoint id, …]}]`, as the gateway takes it.
fn listing(subscriptions: &BTreeMap<String, Vec<Subscription>>) -> Value {
    subscriptions
        .iter()
        .map(|(device, list)| {
            let endpoints: Vec<_> = list.iter().map(|s| endpoint_id(&s.endpoint)).collect();
            json!({"deviceId": device, "endpoints": endpoints})
        })
        .collect()
}

/// The request for one phone: the encrypted message, signed for the push
/// service of its endpoint.
fn build_request(
    vapid: &Vapid,
    subscription: &Subscription,
    message: &[u8],
    urgency: &'static str,
) -> Result<Request, String> {
    let (receiver, auth) = subscription
        .keys()
        .ok_or("companion-push-subscription-invalid")?;
    let audience = reqwest::Url::parse(&subscription.endpoint)
        .map_err(|_| "companion-push-subscription-invalid")?
        .origin()
        .ascii_serialization();
    let expires = unix_now() + JWT_LIFETIME_SECS;
    let body = encrypt(message, &receiver, &auth, &random_secret(), &rand::random())?;
    Ok(Request {
        url: subscription.endpoint.clone(),
        headers: vec![
            ("TTL", TTL_SECS.to_string()),
            ("Urgency", urgency.to_string()),
            ("Content-Encoding", "aes128gcm".to_string()),
            ("Content-Type", "application/octet-stream".to_string()),
            (
                "Authorization",
                format!(
                    "vapid t={}, k={}",
                    vapid_jwt(&vapid.key, &audience, expires),
                    vapid.public
                ),
            ),
        ],
        body,
    })
}

/// RFC 8291 over RFC 8188, one record. `sender` and `salt` are new for every
/// message.
fn encrypt(
    message: &[u8],
    receiver: &PublicKey,
    auth: &[u8],
    sender: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, String> {
    fn failed<E>(_: E) -> String {
        "companion-push-encrypt-failed".into()
    }
    let (sender_public, receiver_public) =
        (uncompressed(&sender.public_key()), uncompressed(receiver));
    let shared = sender.diffie_hellman(receiver);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(&receiver_public);
    key_info.extend_from_slice(&sender_public);
    let mut ikm = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, ikm.as_mut())
        .map_err(failed)?;
    let keys = Hkdf::<Sha256>::new(Some(salt), ikm.as_ref());
    let mut cek = Zeroizing::new([0u8; 16]);
    let mut nonce = [0u8; 12];
    keys.expand(b"Content-Encoding: aes128gcm\0", cek.as_mut())
        .map_err(failed)?;
    keys.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .map_err(failed)?;
    // The last record ends in the delimiter 0x02, without padding after it.
    let mut record = Zeroizing::new(message.to_vec());
    record.push(2);
    let sealed = Aes128Gcm::new_from_slice(cek.as_ref())
        .map_err(failed)?
        .encrypt(&nonce.into(), record.as_slice())
        .map_err(failed)?;
    // Header: salt, record size, key id length and the key id: the sender's key.
    let mut body = Vec::with_capacity(21 + sender_public.len() + sealed.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(sender_public.len() as u8);
    body.extend_from_slice(&sender_public);
    body.extend_from_slice(&sealed);
    Ok(body)
}

/// The ES256 JWT of RFC 8292: the push service takes the origin it is
/// addressed by as `aud`.
fn vapid_jwt(key: &SigningKey, audience: &str, expires: u64) -> String {
    #[derive(Serialize)]
    struct Header {
        typ: &'static str,
        alg: &'static str,
    }
    #[derive(Serialize)]
    struct Claims<'a> {
        aud: &'a str,
        exp: u64,
        sub: &'static str,
    }
    let part = |json: Vec<u8>| URL_SAFE_NO_PAD.encode(json);
    let input = format!(
        "{}.{}",
        part(
            serde_json::to_vec(&Header {
                typ: "JWT",
                alg: "ES256"
            })
            .unwrap_or_default()
        ),
        part(
            serde_json::to_vec(&Claims {
                aud: audience,
                exp: expires,
                sub: SUBJECT
            })
            .unwrap_or_default()
        ),
    );
    // Raw r || s, as JWS wants it.
    let signature: Signature = key.sign(input.as_bytes());
    format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn random_secret() -> SecretKey {
    loop {
        // A draw outside the curve order or zero is vanishingly rare; draw again.
        let bytes = Zeroizing::new(rand::random::<[u8; 32]>());
        if let Ok(secret) = SecretKey::from_slice(bytes.as_ref()) {
            return secret;
        }
    }
}

fn uncompressed(key: &PublicKey) -> Vec<u8> {
    p256::Sec1Point::from(key).to_bytes().to_vec()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn log_warning(event: &str, message: &str) {
    crate::logging::write_global(crate::logging::LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: "companion".to_string(),
        workspace_path: None,
        event: event.to_string(),
        message: message.to_string(),
        metadata: json!({}),
    });
}

#[cfg(test)]
pub(crate) mod tests;
