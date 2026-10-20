//! The network, the OS credential store and the clock are fakes: a test never
//! reaches a push service or the real keyring.
use super::*;
use p256::ecdsa::{signature::Verifier, VerifyingKey};
use std::collections::HashSet;

const DEVICE: &str = include_str!("../../companion/tests/fixtures/device.json");
pub(crate) const DEVICE_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
pub(crate) const OTHER_ID: &str = "BBBBBBBBBBBBBBBBBBBBBQ";

// RFC 8291 section 5 and appendix A.
const RFC_MESSAGE: &str = "When I grow up, I want to be a watermelon";
const RFC_SENDER_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
const RFC_SENDER_PUBLIC: &str =
    "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
const RFC_RECEIVER_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const RFC_RECEIVER_PUBLIC: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const RFC_SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
const RFC_AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const RFC_BODY: &str = concat!(
    "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml",
    "mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT",
    "pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
);

fn b64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).unwrap()
}
fn secret(text: &str) -> SecretKey {
    SecretKey::from_slice(&b64(text)).unwrap()
}
fn receiver() -> PublicKey {
    PublicKey::from_sec1_bytes(&b64(RFC_RECEIVER_PUBLIC)).unwrap()
}

/// The credential store: in memory, and able to fail.
#[derive(Default)]
pub(crate) struct TestVault {
    pub secret: Mutex<Option<String>>,
    pub fail: Mutex<bool>,
    /// Reads work, writes do not.
    pub fail_writes: Mutex<bool>,
    pub writes: Mutex<usize>,
}
impl Vault for TestVault {
    fn load(&self) -> Result<Option<Zeroizing<String>>, String> {
        if *self.fail.lock().unwrap() {
            return Err("companion-push-vault-unavailable".into());
        }
        Ok(self.secret.lock().unwrap().clone().map(Zeroizing::new))
    }
    fn store(&self, secret: &str) -> Result<(), String> {
        if *self.fail.lock().unwrap() || *self.fail_writes.lock().unwrap() {
            return Err("companion-push-vault-write-failed".into());
        }
        *self.writes.lock().unwrap() += 1;
        *self.secret.lock().unwrap() = Some(secret.to_owned());
        Ok(())
    }
}

/// Everything a `Push` touches outside the disk.
#[derive(Clone)]
pub(crate) struct Wire {
    pub vault: Arc<TestVault>,
    /// The requests the fake push service received.
    pub requests: Arc<Mutex<Vec<Request>>>,
    /// What it answers, by URL; 201 otherwise.
    pub answers: Arc<Mutex<HashMap<String, u16>>>,
    /// Time passed since the clock started.
    pub elapsed: Arc<Mutex<Duration>>,
}
impl Wire {
    pub fn push(&self, dir: &Path) -> Arc<Push> {
        let (requests, answers) = (self.requests.clone(), self.answers.clone());
        let (start, elapsed) = (Instant::now(), self.elapsed.clone());
        Push::new(
            dir.to_owned(),
            self.vault.clone(),
            Arc::new(move |request| {
                let status = answers
                    .lock()
                    .unwrap()
                    .get(&request.url)
                    .copied()
                    .unwrap_or(201);
                requests.lock().unwrap().push(request);
                Box::pin(async move { Ok(status) })
            }),
            Arc::new(move || start + *elapsed.lock().unwrap()),
        )
    }
    pub fn answer(&self, url: &str, status: u16) {
        self.answers.lock().unwrap().insert(url.into(), status);
    }
    pub fn pass(&self, time: Duration) {
        *self.elapsed.lock().unwrap() += time;
    }
    pub fn urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect()
    }
}
pub(crate) fn wire() -> Wire {
    Wire {
        vault: Arc::new(TestVault::default()),
        requests: Arc::default(),
        answers: Arc::default(),
        elapsed: Arc::default(),
    }
}

/// Stores devices, as the gateway's events would have.
pub(crate) fn seed(dir: &Path, ids: &[&str]) {
    let mut devices = Devices::load(dir).unwrap();
    for id in ids {
        let mut device: Value = serde_json::from_str(DEVICE).unwrap();
        device["deviceId"] = json!(id);
        device["credentials"][0]["id"] = json!(id);
        devices.upsert(device).unwrap();
    }
    // What the supervisor would load at the next start.
    Devices::load(dir).unwrap();
}

pub(crate) fn endpoint(n: u32) -> String {
    format!("https://fcm.googleapis.com/fcm/send/{n}")
}

pub(crate) fn subscription(n: u32) -> Value {
    json!({
        "endpoint": endpoint(n),
        "p256dh": RFC_RECEIVER_PUBLIC,
        "auth": RFC_AUTH,
    })
}

pub(crate) fn request_payload(pane: &str) -> Value {
    json!({
        "kind": "blocked",
        "session": "default",
        "paneId": pane,
        "workspaceId": "w1",
        "title": "claude",
        "space": "api",
    })
}

/// A push configured and storing the given devices' subscriptions.
pub(crate) fn ready(ids: &[&str]) -> (tempfile::TempDir, Wire, Arc<Push>) {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), ids);
    let wire = wire();
    let push = wire.push(root.path());
    assert!(push.init().is_object());
    for (n, id) in ids.iter().enumerate() {
        assert!(push.subscription(id, &subscription(n as u32)));
    }
    (root, wire, push)
}

/// What the phone does with a pushed body.
fn decrypt(body: &[u8], receiver: &SecretKey, auth: &[u8]) -> Vec<u8> {
    let salt: [u8; 16] = body[..16].try_into().unwrap();
    assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
    let key_len = body[20] as usize;
    let sender = PublicKey::from_sec1_bytes(&body[21..21 + key_len]).unwrap();
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend(uncompressed(&receiver.public_key()));
    key_info.extend(&body[21..21 + key_len]);
    let shared = receiver.diffie_hellman(&sender);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .unwrap();
    let keys = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    keys.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .unwrap();
    keys.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .unwrap();
    let record = Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .decrypt(&nonce.into(), &body[21 + key_len..])
        .unwrap();
    let end = record.iter().rposition(|byte| *byte != 0).unwrap();
    assert_eq!(record[end], 2, "padding delimiter");
    record[..end].to_vec()
}

fn header<'a>(request: &'a Request, name: &str) -> &'a str {
    let mut found = request.headers.iter().filter(|(n, _)| *n == name);
    let value = found.next().unwrap_or_else(|| panic!("no {name}"));
    assert!(found.next().is_none(), "{name} twice");
    &value.1
}

fn claims(jwt: &str) -> Value {
    serde_json::from_slice(&b64(jwt.split('.').nth(1).unwrap())).unwrap()
}

#[test]
fn companion_push_encrypts_the_rfc_8291_example() {
    let body = encrypt(
        RFC_MESSAGE.as_bytes(),
        &receiver(),
        &b64(RFC_AUTH),
        &secret(RFC_SENDER_PRIVATE),
        &b64(RFC_SALT).try_into().unwrap(),
    )
    .unwrap();
    assert_eq!(URL_SAFE_NO_PAD.encode(&body), RFC_BODY);
    // The header is the 86 octets the RFC counts, ending in the sender's key;
    // then the message, its delimiter and the tag.
    assert_eq!(&body[21..86], b64(RFC_SENDER_PUBLIC));
    assert_eq!(body.len(), 86 + RFC_MESSAGE.len() + 1 + 16);
}

#[test]
fn companion_push_messages_decrypt_for_the_receiver_and_never_repeat() {
    let (receiver_key, auth) = (secret(RFC_RECEIVER_PRIVATE), b64(RFC_AUTH));
    let seal = || {
        encrypt(
            b"same message",
            &receiver(),
            &auth,
            &random_secret(),
            &rand::random(),
        )
        .unwrap()
    };
    let (first, second) = (seal(), seal());
    assert_eq!(decrypt(&first, &receiver_key, &auth), b"same message");
    assert_eq!(decrypt(&second, &receiver_key, &auth), b"same message");
    // A fresh salt and sender key per message.
    assert_ne!(first[..16], second[..16]);
    assert_ne!(first[21..86], second[21..86]);
    // The wrong authentication secret does not decrypt.
    let result = std::panic::catch_unwind(|| decrypt(&first, &receiver_key, &[0u8; 16]));
    assert!(result.is_err());
}

#[test]
fn companion_push_vapid_jwt_verifies_with_the_public_key() {
    let vapid = Vapid::new(&secret(RFC_SENDER_PRIVATE));
    assert_eq!(vapid.public, RFC_SENDER_PUBLIC);
    let jwt = vapid_jwt(&vapid.key, "https://fcm.googleapis.com", 1_800_000_000);
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(b64(parts[0]), br#"{"typ":"JWT","alg":"ES256"}"#);
    assert_eq!(
        b64(parts[1]),
        br#"{"aud":"https://fcm.googleapis.com","exp":1800000000,"sub":"https://github.com/NakiriYuuzu/Yuzora"}"#
    );
    // JWS wants the raw 64-octet r || s, not DER.
    let signature = Signature::from_slice(&b64(parts[2])).unwrap();
    let verifier = VerifyingKey::from_sec1_bytes(&b64(&vapid.public)).unwrap();
    let signed = format!("{}.{}", parts[0], parts[1]);
    assert!(verifier.verify(signed.as_bytes(), &signature).is_ok());
    // Another audience is another signature.
    let forged = format!(
        "{}.{}",
        parts[0],
        URL_SAFE_NO_PAD.encode(br#"{"aud":"https://evil.example"}"#)
    );
    assert!(verifier.verify(forged.as_bytes(), &signature).is_err());
}

#[tokio::test]
async fn companion_push_requests_have_the_contract_headers_and_only_the_contract_fields() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    let mut payload = request_payload("p1");
    payload["extra"] = json!("leak");
    payload["title"] = json!("claude\u{7}");
    let job = push.request(&json!([DEVICE_ID]), &payload).unwrap();
    assert!(!push.deliver(job).await);

    let requests = wire.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.url, "https://fcm.googleapis.com/fcm/send/0");
    assert_eq!(header(request, "TTL"), "3600");
    assert_eq!(header(request, "Urgency"), "high");
    assert_eq!(header(request, "Content-Encoding"), "aes128gcm");
    assert_eq!(header(request, "Content-Type"), "application/octet-stream");
    let vapid = push.vapid().unwrap();
    let authorization = header(request, "Authorization");
    let jwt = authorization
        .strip_prefix("vapid t=")
        .and_then(|rest| rest.strip_suffix(&format!(", k={}", vapid.public)))
        .expect("vapid t=<jwt>, k=<public key>");
    let claims = claims(jwt);
    assert_eq!(claims["aud"], "https://fcm.googleapis.com");
    assert_eq!(claims["sub"], "https://github.com/NakiriYuuzu/Yuzora");
    let exp = claims["exp"].as_u64().unwrap();
    assert!((exp - unix_now()).abs_diff(JWT_LIFETIME_SECS) <= 5, "{exp}");
    let signed = &jwt[..jwt.rfind('.').unwrap()];
    let signature = Signature::from_slice(&b64(jwt.rsplit('.').next().unwrap())).unwrap();
    VerifyingKey::from_sec1_bytes(&b64(&vapid.public))
        .unwrap()
        .verify(signed.as_bytes(), &signature)
        .unwrap();

    let message = decrypt(&request.body, &secret(RFC_RECEIVER_PRIVATE), &b64(RFC_AUTH));
    assert!(message.len() <= MESSAGE_BYTES);
    assert_eq!(
        serde_json::from_slice::<Value>(&message).unwrap(),
        json!({"v": 1, "kind": "blocked", "title": "claude", "space": "api", "session": "default", "pane": "p1"})
    );
    let text = String::from_utf8(message).unwrap();
    assert!(!text.contains("leak") && !text.contains("w1"));
}

#[tokio::test]
async fn companion_push_done_is_normal_urgency_and_a_missing_space_is_null() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    let payload = json!({"kind": "done", "session": "s", "paneId": "p", "workspaceId": null, "title": "agent", "space": null});
    let job = push.request(&json!([DEVICE_ID]), &payload).unwrap();
    push.deliver(job).await;
    let requests = wire.requests.lock().unwrap();
    assert_eq!(header(&requests[0], "Urgency"), "normal");
    let message = decrypt(
        &requests[0].body,
        &secret(RFC_RECEIVER_PRIVATE),
        &b64(RFC_AUTH),
    );
    let message: Value = serde_json::from_slice(&message).unwrap();
    assert_eq!(message["kind"], "done");
    assert_eq!(message["space"], Value::Null);
}

#[test]
fn companion_push_requests_that_do_not_look_right_send_nothing() {
    let (_root, _wire, push) = ready(&[DEVICE_ID]);
    let devices = json!([DEVICE_ID]);
    let with = |field: &str, value: Value| {
        let mut payload = request_payload("p1");
        payload[field] = value;
        payload
    };
    for payload in [
        with("kind", json!("working")),
        with("kind", Value::Null),
        with("title", json!(7)),
        with("session", json!("")),
        with("session", json!("x".repeat(257))),
        with("paneId", json!("a\nb")),
        with("paneId", Value::Null),
    ] {
        assert!(push.request(&devices, &payload).is_none(), "{payload}");
    }
    // Devices without a subscription, and a request without a device list.
    assert!(push
        .request(&json!([OTHER_ID]), &request_payload("p1"))
        .is_none());
    assert!(push.request(&json!([]), &request_payload("p1")).is_none());
    assert!(push.request(&Value::Null, &request_payload("p1")).is_none());
    // None of these used up the pane's cooldown.
    assert!(push.request(&devices, &request_payload("p1")).is_some());
}

#[test]
fn companion_push_long_text_is_cut_to_fit() {
    let (_root, _wire, push) = ready(&[DEVICE_ID]);
    let mut payload = request_payload("p1");
    payload["title"] = json!("題".repeat(500));
    payload["space"] = json!("s".repeat(500));
    let job = push.request(&json!([DEVICE_ID]), &payload).unwrap();
    let message: Value = serde_json::from_slice(&job.message).unwrap();
    assert!(message["title"].as_str().unwrap().len() <= TEXT_BYTES);
    assert!(message["space"].as_str().unwrap().len() <= TEXT_BYTES);
    assert!(job.message.len() <= MESSAGE_BYTES);
}

/// Asks for a push and, if there is one, sends it.
async fn pushed(push: &Push, payload: &Value) -> bool {
    match push.request(&json!([DEVICE_ID]), payload) {
        Some(job) => {
            push.deliver(job).await;
            true
        }
        None => false,
    }
}

fn of_kind(pane: &str, kind: &str) -> Value {
    let mut payload = request_payload(pane);
    payload["kind"] = json!(kind);
    payload
}

#[tokio::test]
async fn companion_push_cooldown_holds_a_pane_and_kind_for_a_minute() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    assert!(pushed(&push, &request_payload("p1")).await);
    // Same pane, same session, same kind: held.
    wire.pass(Duration::from_secs(30));
    assert!(!pushed(&push, &request_payload("p1")).await);
    // Another kind is not: a `blocked` is never lost to an earlier `done`.
    assert!(pushed(&push, &of_kind("p1", "done")).await);
    assert!(!pushed(&push, &of_kind("p1", "done")).await);
    // Another pane and another session are not held.
    assert!(pushed(&push, &request_payload("p2")).await);
    let mut other_session = request_payload("p1");
    other_session["session"] = json!("work");
    assert!(pushed(&push, &other_session).await);
    // A held request does not extend the hold: a minute after the first.
    wire.pass(Duration::from_secs(29));
    assert!(!pushed(&push, &request_payload("p1")).await);
    wire.pass(Duration::from_secs(1));
    assert!(pushed(&push, &request_payload("p1")).await);
    // The table of held panes does not grow without bound.
    wire.pass(Duration::from_secs(61));
    assert!(pushed(&push, &request_payload("p9")).await);
    assert_eq!(push.state.lock().unwrap().sent.len(), 1);
}

#[tokio::test]
async fn companion_push_a_blocked_agent_is_not_silenced_by_an_earlier_done() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    assert!(pushed(&push, &of_kind("p1", "done")).await);
    wire.pass(Duration::from_secs(20));
    assert!(pushed(&push, &of_kind("p1", "blocked")).await);
    assert_eq!(wire.urls().len(), 2);
}

#[tokio::test]
async fn companion_push_the_hold_starts_with_a_send_that_was_tried_not_with_the_request() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    let devices = json!([DEVICE_ID]);
    // A request that is not delivered holds nothing.
    drop(push.request(&devices, &request_payload("p1")).unwrap());
    assert!(push.state.lock().unwrap().sent.is_empty());
    assert!(push.request(&devices, &request_payload("p1")).is_some());

    // One on its way is not sent twice, before and after its first send.
    let job = push.request(&devices, &request_payload("p1")).unwrap();
    assert!(push.request(&devices, &request_payload("p1")).is_none());
    push.deliver(job).await;
    assert!(push.request(&devices, &request_payload("p1")).is_none());
    assert_eq!(wire.urls().len(), 1);

    // A job that never got as far as sending leaves no hold behind: here the
    // stored subscription cannot be built into a request.
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    push.state.lock().unwrap().subscriptions.insert(
        DEVICE_ID.into(),
        vec![Subscription {
            endpoint: "https://example.com/not-a-push-service".into(),
            p256dh: "x".into(),
            auth: "y".into(),
            created_at: "2026-10-20T00:00:00Z".into(),
        }],
    );
    assert!(pushed(&push, &request_payload("p1")).await);
    assert!(wire.urls().is_empty(), "nothing was sent");
    assert!(push.state.lock().unwrap().sent.is_empty());
    assert!(push.request(&devices, &request_payload("p1")).is_some());

    // A send that failed was tried: the hold stays, so a phone that is
    // unreachable is not retried on every flicker.
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    wire.answer("https://fcm.googleapis.com/fcm/send/0", 503);
    assert!(pushed(&push, &request_payload("p1")).await);
    assert_eq!(wire.urls().len(), 1);
    assert!(!pushed(&push, &request_payload("p1")).await);
}

#[tokio::test]
async fn companion_push_gone_subscriptions_are_deleted_and_others_kept() {
    let (root, wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    let devices = json!([DEVICE_ID, OTHER_ID]);
    wire.answer("https://fcm.googleapis.com/fcm/send/0", 410);
    wire.answer("https://fcm.googleapis.com/fcm/send/1", 503);
    let job = push.request(&devices, &request_payload("p1")).unwrap();
    // 410: the push service no longer knows it. 503: it might again.
    assert!(push.deliver(job).await);
    assert_eq!(push.devices(), [OTHER_ID]);
    let file: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("push.json")).unwrap()).unwrap();
    assert_eq!(
        file["subscriptions"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        [OTHER_ID]
    );
    assert_eq!(wire.urls().len(), 2);

    // 404 is gone as well.
    wire.answer("https://fcm.googleapis.com/fcm/send/1", 404);
    wire.pass(Duration::from_secs(61));
    let job = push.request(&devices, &request_payload("p1")).unwrap();
    assert!(push.deliver(job).await);
    assert!(push.devices().is_empty());
}

#[tokio::test]
async fn companion_push_a_new_subscription_survives_the_old_ones_410() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    wire.answer("https://fcm.googleapis.com/fcm/send/0", 410);
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    // The phone subscribes again while the old endpoint is being tried.
    assert!(push.subscription(DEVICE_ID, &subscription(7)));
    // The gone endpoint leaves, and only it: the gateway is told.
    assert!(push.deliver(job).await);
    assert_eq!(push.devices(), [DEVICE_ID]);
    assert_eq!(
        push.listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(7))]}])
    );
}

#[test]
fn companion_push_subscriptions_are_pruned_on_revoke_and_on_load() {
    let (root, wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    assert_eq!(push.devices(), [DEVICE_ID, OTHER_ID]);
    assert!(push.forget(DEVICE_ID));
    assert!(!push.forget(DEVICE_ID), "nothing left to forget");
    assert_eq!(push.devices(), [OTHER_ID]);
    assert!(!std::fs::read_to_string(root.path().join("push.json"))
        .unwrap()
        .contains("/send/0"));

    // A device revoked while no gateway ran is gone from devices.json: load
    // drops its subscription, and so does the next init.
    Devices::load(root.path())
        .unwrap()
        .remove(OTHER_ID)
        .unwrap();
    let reloaded = wire.push(root.path());
    assert!(reloaded.devices().is_empty());
    assert_eq!(push.devices(), [OTHER_ID], "not yet noticed");
    push.init();
    assert!(push.devices().is_empty());
    assert!(!std::fs::read_to_string(root.path().join("push.json"))
        .unwrap()
        .contains("/send/1"));
}

#[test]
fn companion_push_entries_that_no_longer_validate_are_dropped_on_load() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID, OTHER_ID]);
    let good = json!({"endpoint": "https://fcm.googleapis.com/x", "p256dh": RFC_RECEIVER_PUBLIC, "auth": RFC_AUTH, "createdAt": "2026-10-20T00:00:00Z"});
    let mut bad = good.clone();
    bad["endpoint"] = json!("https://evil.example/x");
    std::fs::write(
        root.path().join("push.json"),
        json!({"version": 1, "subscriptions": {DEVICE_ID: [good.clone()], OTHER_ID: [bad.clone()]}}).to_string(),
    )
    .unwrap();
    assert_eq!(wire().push(root.path()).devices(), [DEVICE_ID]);
    // A device keeps its valid subscriptions, whatever else was stored with them.
    std::fs::write(
        root.path().join("push.json"),
        json!({"version": 1, "subscriptions": {DEVICE_ID: [bad, good]}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        wire().push(root.path()).listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id("https://fcm.googleapis.com/x")]}])
    );
}

#[test]
fn companion_push_a_corrupt_file_is_kept_aside_and_starts_empty() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID]);
    std::fs::write(root.path().join("push.json"), "{not json").unwrap();
    let wire = wire();
    let push = wire.push(root.path());
    assert!(push.devices().is_empty());
    let kept: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("push.json.corrupt-"))
        .collect();
    assert_eq!(kept.len(), 1);
    // And it works again: the phones subscribe again.
    assert!(push.init().is_object());
    assert!(push.subscription(DEVICE_ID, &subscription(1)));
    assert_eq!(wire.push(root.path()).devices(), [DEVICE_ID]);
}

#[test]
fn companion_push_without_the_key_vault_push_is_unavailable() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID]);
    let wire = wire();
    *wire.vault.fail.lock().unwrap() = true;
    let push = wire.push(root.path());
    assert_eq!(push.init(), Value::Null);
    assert!(!push.overview().available);
    assert!(push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .is_none());
    // Nothing was invented behind its back.
    assert!(wire.vault.secret.lock().unwrap().is_none());

    // The vault comes back: the next enable has push.
    *wire.vault.fail.lock().unwrap() = false;
    assert!(push.init().is_object());
    assert!(push.overview().available);
}

#[test]
fn companion_push_a_key_that_cannot_be_read_is_never_replaced() {
    let root = tempfile::tempdir().unwrap();
    let wire = wire();
    for corrupt in ["not base64 !!", "AAAA", &URL_SAFE_NO_PAD.encode([0u8; 32])] {
        *wire.vault.secret.lock().unwrap() = Some(corrupt.to_owned());
        let push = wire.push(root.path());
        assert_eq!(push.init(), Value::Null, "{corrupt}");
        assert_eq!(wire.vault.secret.lock().unwrap().as_deref(), Some(corrupt));
    }
    assert_eq!(*wire.vault.writes.lock().unwrap(), 0);
}

#[test]
fn companion_push_a_new_key_drops_the_subscriptions_made_for_the_old_one() {
    let (root, wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    let old_key = push.init()["vapidPublicKey"].clone();
    assert_eq!(push.devices(), [DEVICE_ID, OTHER_ID]);
    // The credential store lost the key (reset, a restore onto a new Mac);
    // push.json came along.
    *wire.vault.secret.lock().unwrap() = None;
    let restarted = wire.push(root.path());
    assert_eq!(restarted.devices(), [DEVICE_ID, OTHER_ID]);
    let init = restarted.init();
    assert_ne!(init["vapidPublicKey"], old_key);
    // Every one was made for the old key: none is kept, the phones subscribe again.
    assert_eq!(init["devices"], json!([]));
    assert!(restarted.devices().is_empty());
    assert!(!std::fs::read_to_string(root.path().join("push.json"))
        .unwrap()
        .contains("/send/"));
    assert!(wire.push(root.path()).devices().is_empty());

    // New subscriptions are for the new key, and stay.
    assert!(restarted.subscription(DEVICE_ID, &subscription(5)));
    let next = wire.push(root.path());
    assert_eq!(
        next.init()["devices"],
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(5))]}])
    );
}

#[test]
fn companion_push_a_new_key_drops_every_browser_of_a_device() {
    let (root, wire, push) = ready(&[DEVICE_ID]);
    assert!(push.subscription(DEVICE_ID, &subscription(8)));
    assert_eq!(push.listing()[0]["endpoints"].as_array().unwrap().len(), 2);
    *wire.vault.secret.lock().unwrap() = None;
    let restarted = wire.push(root.path());
    assert_eq!(restarted.init()["devices"], json!([]));
    assert!(restarted.devices().is_empty());
    assert!(wire.push(root.path()).devices().is_empty());
}

#[test]
fn companion_push_a_key_that_could_not_be_stored_drops_nothing() {
    let (root, wire, _push) = ready(&[DEVICE_ID]);
    *wire.vault.secret.lock().unwrap() = None;
    *wire.vault.fail_writes.lock().unwrap() = true;
    let restarted = wire.push(root.path());
    assert_eq!(restarted.init(), Value::Null);
    assert_eq!(restarted.devices(), [DEVICE_ID]);
    assert!(std::fs::read_to_string(root.path().join("push.json"))
        .unwrap()
        .contains("/send/0"));
}

#[test]
fn companion_push_the_vapid_key_is_made_once_and_shown_as_a_p256_point() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID]);
    let wire = wire();
    let init = wire.push(root.path()).init();
    let public = b64(init["vapidPublicKey"].as_str().unwrap());
    assert_eq!(init["vapidPublicKey"].as_str().unwrap().len(), 87);
    assert_eq!((public.len(), public[0]), (65, 4));
    assert_eq!(init["devices"], json!([]));
    assert_eq!(*wire.vault.writes.lock().unwrap(), 1);
    // The private scalar is what the vault holds: 32 bytes of base64url.
    let stored = wire.vault.secret.lock().unwrap().clone().unwrap();
    assert_eq!(b64(&stored).len(), 32);
    // A second start finds it.
    assert_eq!(
        wire.push(root.path()).init()["vapidPublicKey"],
        init["vapidPublicKey"]
    );
    assert_eq!(*wire.vault.writes.lock().unwrap(), 1);
}

#[test]
fn companion_push_init_lists_the_devices_that_subscribed() {
    let (_root, _wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    let init = push.init();
    // The gateway is told ids, never the endpoints themselves.
    assert_eq!(
        init["devices"],
        json!([
            {"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(0))]},
            {"deviceId": OTHER_ID, "endpoints": [endpoint_id(&endpoint(1))]},
        ])
    );
    assert!(!init.to_string().contains("fcm.googleapis.com"));
    assert_eq!(push.overview().subscribed, [DEVICE_ID, OTHER_ID], "{init}");
    assert_eq!(
        serde_json::to_value(push.overview()).unwrap(),
        json!({"available": true, "subscribed": [DEVICE_ID, OTHER_ID]})
    );
    assert_eq!(
        push.event(),
        json!({"type": "push", "push": {"available": true, "subscribed": [DEVICE_ID, OTHER_ID]}})
    );
}

#[test]
fn companion_push_only_push_services_and_valid_keys_are_accepted() {
    let (_root, _wire, push) = ready(&[]);
    let with = |field: &str, value: Value| {
        let mut sub = subscription(1);
        sub[field] = value;
        sub
    };
    let host = |url: &str| with("endpoint", json!(url));
    let long = format!("https://fcm.googleapis.com/{}", "a".repeat(2048));
    let mut extra = subscription(1);
    extra["expirationTime"] = json!(null);
    let off_curve = URL_SAFE_NO_PAD.encode([&[4u8][..], &[1u8; 64][..]].concat());
    let compressed = URL_SAFE_NO_PAD.encode([&[2u8][..], &[1u8; 32][..]].concat());
    for rejected in [
        host("http://fcm.googleapis.com/x"),
        host("https://evil.example/x"),
        host("https://fcm.googleapis.com.evil.example/x"),
        host("https://evilfcm.googleapis.com/x"),
        host("https://user@fcm.googleapis.com/x"),
        host("https://user:pw@fcm.googleapis.com/x"),
        host("https://fcm.googleapis.com/x#frag"),
        host("https://127.0.0.1/x"),
        host("https://fcm.googleapis.com/x y"),
        host("https://fcm.googleapis.com/\nx"),
        host(&long),
        host("push.apple.com"),
        with("p256dh", json!(RFC_AUTH)),
        with("p256dh", json!(off_curve)),
        with("p256dh", json!(compressed)),
        with("auth", json!("AAAAAAAAAAAAAAAAAAA")),
        with("auth", json!(7)),
        extra,
        json!("https://fcm.googleapis.com/x"),
    ] {
        assert!(!push.subscription(DEVICE_ID, &rejected), "{rejected}");
        assert!(push.devices().is_empty(), "{rejected}");
    }
    for accepted in [
        "https://fcm.googleapis.com/fcm/send/abc",
        "https://web.push.apple.com/abc",
        "https://updates.push.services.mozilla.com/wpush/v2/abc",
        "https://par02p.wns.notify.windows.com/?token=abc",
    ] {
        assert!(push.subscription(DEVICE_ID, &host(accepted)), "{accepted}");
        assert!(push.forget(DEVICE_ID));
    }
}

#[test]
fn companion_push_a_removal_deletes_one_endpoint_and_nothing_is_deleted_twice() {
    let (root, _wire, push) = ready(&[DEVICE_ID]);
    let id = endpoint_id(&endpoint(0));
    assert!(push.unsubscribe(DEVICE_ID, &id));
    assert!(push.devices().is_empty());
    assert!(!push.unsubscribe(DEVICE_ID, &id));
    // `null` alone names nothing to remove.
    assert!(push.subscription(DEVICE_ID, &subscription(0)));
    assert!(!push.subscription(DEVICE_ID, &Value::Null));
    assert_eq!(push.devices(), [DEVICE_ID]);
    // An id nothing has, and a device nothing is stored for.
    let before = std::fs::read(root.path().join("push.json")).unwrap();
    assert!(!push.unsubscribe(
        DEVICE_ID,
        &endpoint_id("https://fcm.googleapis.com/fcm/send/9")
    ));
    assert!(!push.unsubscribe(OTHER_ID, &id));
    assert!(!push.unsubscribe(DEVICE_ID, ""));
    assert_eq!(
        std::fs::read(root.path().join("push.json")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn companion_push_file_is_private_and_has_the_documented_shape() {
    use std::os::unix::fs::PermissionsExt;
    let (root, _wire, _push) = ready(&[DEVICE_ID]);
    let path = root.path().join("push.json");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(file["version"], 1);
    let stored = &file["subscriptions"][DEVICE_ID][0];
    assert_eq!(
        stored.as_object().unwrap().keys().collect::<HashSet<_>>(),
        HashSet::from([
            &"endpoint".to_string(),
            &"p256dh".into(),
            &"auth".into(),
            &"createdAt".into()
        ])
    );
    assert_eq!(stored["endpoint"], "https://fcm.googleapis.com/fcm/send/0");
    assert!(chrono::DateTime::parse_from_rfc3339(stored["createdAt"].as_str().unwrap()).is_ok());
}

#[test]
fn companion_push_endpoint_ids_are_the_unpadded_base64url_sha256_of_the_endpoint() {
    // The gateway computes the same from what a browser asks about; these
    // values are repeated in its tests.
    assert_eq!(
        endpoint_id("https://fcm.googleapis.com/fcm/send/0"),
        "tRqoN_kd32hzj5odxkArmczU-CyOIuh-D_DrZP6IVJ8"
    );
    assert_eq!(
        endpoint_id("https://fcm.googleapis.com/fcm/send/1"),
        "67XxfEniWMkw1hRk0LiNv7KN4zCzWwas3xJdg_G1GUQ"
    );
    // The gateway's own vectors (its push tests hold the same three).
    for (endpoint, id) in [
        (
            "https://fcm.googleapis.com/fcm/send/abc",
            "TpvauLvnGJwArrsJIvxJ_kDgBZTEAPPhtdz5ejXHn4U",
        ),
        (
            "https://web.push.apple.com/QGkYcR",
            "sFvvt2WUyUIAqagyTBvQJeujuvIhEs9A_S9n-_FvSXQ",
        ),
        ("", "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"),
    ] {
        assert_eq!(endpoint_id(endpoint), id);
    }
    assert_eq!(endpoint_id("https://web.push.apple.com/abc").len(), 43);
}

#[tokio::test]
async fn companion_push_a_second_browser_of_a_device_does_not_replace_the_first() {
    // The same synced passkey signs in on an iPhone and an iPad: one device.
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    assert!(push.subscription(DEVICE_ID, &subscription(5)));
    assert_eq!(push.devices(), [DEVICE_ID]);
    assert_eq!(
        push.listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(0)), endpoint_id(&endpoint(5))]}])
    );

    // A push reaches both.
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    assert!(!push.deliver(job).await);
    assert_eq!(wire.urls(), [endpoint(0), endpoint(5)]);

    // The iPad turning it off removes the iPad's, and the iPhone's stays.
    assert!(push.unsubscribe(DEVICE_ID, &endpoint_id(&endpoint(5))));
    assert_eq!(push.devices(), [DEVICE_ID]);
    wire.pass(COOLDOWN);
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    push.deliver(job).await;
    assert_eq!(wire.urls()[2..], [endpoint(0)]);
}

#[test]
fn companion_push_a_device_keeps_four_browsers_and_a_fifth_replaces_the_oldest() {
    let (root, _wire, push) = ready(&[DEVICE_ID]);
    for n in 1..=3 {
        assert!(push.subscription(DEVICE_ID, &subscription(n)));
    }
    let listed = |push: &Push| -> Vec<String> {
        push.listing()[0]["endpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    };
    let ids =
        |ns: &[u32]| -> Vec<String> { ns.iter().map(|n| endpoint_id(&endpoint(*n))).collect() };
    assert_eq!(listed(&push), ids(&[0, 1, 2, 3]));

    // Subscribing again renews: the same endpoint is not a fifth, and it is now
    // the newest.
    assert!(push.subscription(DEVICE_ID, &subscription(0)));
    assert_eq!(listed(&push), ids(&[1, 2, 3, 0]));

    // A fifth distinct endpoint takes the place of the one subscribed longest ago.
    assert!(push.subscription(DEVICE_ID, &subscription(4)));
    assert_eq!(listed(&push), ids(&[2, 3, 0, 4]));
    let file = std::fs::read_to_string(root.path().join("push.json")).unwrap();
    assert!(!file.contains("/send/1\"") && file.contains("/send/4\""));
    // And it holds across a restart.
    assert_eq!(listed(&wire().push(root.path())), ids(&[2, 3, 0, 4]));
}

#[test]
fn companion_push_a_stored_file_is_held_to_the_same_limits_on_load() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID]);
    let entry = |n: u32| json!({"endpoint": endpoint(n), "p256dh": RFC_RECEIVER_PUBLIC, "auth": RFC_AUTH, "createdAt": "2026-10-20T00:00:00Z"});
    // Seven entries, one endpoint twice: each endpoint once, at the place of its
    // later copy, and the newest four of those.
    let list: Vec<Value> = [0, 1, 2, 3, 1, 4, 5].iter().map(|n| entry(*n)).collect();
    std::fs::write(
        root.path().join("push.json"),
        json!({"version": 1, "subscriptions": {DEVICE_ID: list}}).to_string(),
    )
    .unwrap();
    let ids =
        |ns: &[u32]| -> Vec<String> { ns.iter().map(|n| endpoint_id(&endpoint(*n))).collect() };
    assert_eq!(
        wire().push(root.path()).listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": ids(&[3, 1, 4, 5])}])
    );
}

#[tokio::test]
async fn companion_push_a_gone_endpoint_leaves_alone_and_the_others_still_get_pushes() {
    let (_root, wire, push) = ready(&[DEVICE_ID]);
    assert!(push.subscription(DEVICE_ID, &subscription(5)));
    wire.answer(&endpoint(0), 410);
    wire.answer(&endpoint(5), 503);
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    // 410 for the first browser: it is deleted. 503 for the second: it is kept.
    assert!(push.deliver(job).await);
    assert_eq!(wire.urls(), [endpoint(0), endpoint(5)]);
    assert_eq!(
        push.listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(5))]}])
    );

    // 404 for the last one empties the device.
    wire.answer(&endpoint(5), 404);
    wire.pass(COOLDOWN);
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    assert!(push.deliver(job).await);
    assert!(push.devices().is_empty());
    assert_eq!(push.listing(), json!([]));
}

#[tokio::test]
async fn companion_push_a_push_goes_to_every_browser_of_every_listed_device_once() {
    let (_root, wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    assert!(push.subscription(DEVICE_ID, &subscription(5)));
    assert!(push.subscription(OTHER_ID, &subscription(6)));
    // A device listed twice is one device; one not listed gets nothing.
    let job = push
        .request(&json!([DEVICE_ID, DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    push.deliver(job).await;
    assert_eq!(wire.urls(), [endpoint(0), endpoint(5)]);
    let job = push
        .request(&json!([OTHER_ID, DEVICE_ID]), &request_payload("p2"))
        .unwrap();
    push.deliver(job).await;
    assert_eq!(
        wire.urls()[2..],
        [endpoint(0), endpoint(5), endpoint(1), endpoint(6)]
    );
}

#[tokio::test]
async fn companion_push_an_endpoint_that_does_not_answer_holds_up_only_itself() {
    // The older browser's push service never answers; the newer one's says the
    // subscription is gone. Each request is made without waiting for the other.
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), &[DEVICE_ID]);
    let started: Arc<Mutex<Vec<String>>> = Arc::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let (seen, hung) = (started.clone(), gate.clone());
    let push = Push::new(
        root.path().to_owned(),
        Arc::new(TestVault::default()),
        Arc::new(move |request| {
            let (seen, hung) = (seen.clone(), hung.clone());
            Box::pin(async move {
                seen.lock().unwrap().push(request.url.clone());
                if request.url == endpoint(0) {
                    hung.notified().await;
                    Err("timeout".to_string())
                } else {
                    Ok(410)
                }
            })
        }),
        Arc::new(Instant::now),
    );
    assert!(push.init().is_object());
    assert!(push.subscription(DEVICE_ID, &subscription(0)));
    assert!(push.subscription(DEVICE_ID, &subscription(5)));
    let job = push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .unwrap();
    let sending = {
        let push = push.clone();
        tokio::spawn(async move { push.deliver(job).await })
    };
    for _ in 0..200 {
        if started.lock().unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *started.lock().unwrap(),
        [endpoint(0), endpoint(5)],
        "the second request waited for the first"
    );
    // The gone one is deleted at once, not when the other one gives up.
    assert!(!sending.is_finished());
    assert_eq!(
        push.listing(),
        json!([{"deviceId": DEVICE_ID, "endpoints": [endpoint_id(&endpoint(0))]}])
    );
    // The job still holds its pane: no second push of that kind meanwhile.
    assert!(push
        .request(&json!([DEVICE_ID]), &request_payload("p1"))
        .is_none());

    gate.notify_one();
    // A timeout is not "gone", but the other endpoint was.
    assert!(sending.await.unwrap());
    assert_eq!(push.devices(), [DEVICE_ID]);
}

#[test]
fn companion_push_a_revoked_device_loses_every_browser() {
    let (root, _wire, push) = ready(&[DEVICE_ID, OTHER_ID]);
    assert!(push.subscription(DEVICE_ID, &subscription(5)));
    assert!(push.forget(DEVICE_ID));
    assert_eq!(push.devices(), [OTHER_ID]);
    let file = std::fs::read_to_string(root.path().join("push.json")).unwrap();
    assert!(!file.contains("/send/0\"") && !file.contains("/send/5\""));
    assert!(file.contains("/send/1\""));
}

/// One request in, one canned response out. Returns the address and, when
/// done, the request as received.
async fn serve_once(response: &'static str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await.unwrap();
            received.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&received).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .and_then(|n| n.trim().parse::<usize>().ok())
                    .unwrap_or_default();
                if received.len() >= end + 4 + length {
                    break;
                }
            }
            if read == 0 {
                break;
            }
        }
        stream.write_all(response.as_bytes()).await.unwrap();
        received
    });
    (format!("http://{address}/push/abc"), server)
}

#[tokio::test]
async fn companion_push_http_sender_posts_the_request_and_reports_the_status() {
    let (url, server) =
        serve_once("HTTP/1.1 410 Gone\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    let status = http_sender()(Request {
        url,
        headers: vec![
            ("TTL", "3600".into()),
            ("Authorization", "vapid t=a.b.c, k=K".into()),
        ],
        body: b"ciphertext".to_vec(),
    })
    .await;
    assert_eq!(status, Ok(410));
    let received = String::from_utf8_lossy(&server.await.unwrap()).into_owned();
    assert!(
        received.starts_with("POST /push/abc HTTP/1.1"),
        "{received}"
    );
    let lower = received.to_lowercase();
    assert!(lower.contains("\r\nttl: 3600\r\n"), "{received}");
    assert!(
        lower.contains("\r\nauthorization: vapid t=a.b.c, k=k\r\n"),
        "{received}"
    );
    assert!(
        lower.contains("\r\nuser-agent: yuzora-companion\r\n"),
        "{received}"
    );
    assert!(received.ends_with("ciphertext"), "{received}");
}

#[tokio::test]
async fn companion_push_http_sender_does_not_follow_redirects_and_hides_the_url() {
    let (url, _server) = serve_once("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    let send = http_sender();
    let status = send(Request {
        url,
        headers: Vec::new(),
        body: Vec::new(),
    })
    .await;
    assert_eq!(status, Ok(302));
    // A failure names the kind of failure and never the endpoint.
    let refused = send(Request {
        url: "http://127.0.0.1:1/secret-token".into(),
        headers: Vec::new(),
        body: Vec::new(),
    })
    .await;
    let reason = refused.unwrap_err();
    assert_eq!(reason, "connect");
    assert!(!reason.contains("secret-token"));
}
