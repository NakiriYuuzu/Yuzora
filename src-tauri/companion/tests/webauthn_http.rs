//! Software authenticator fixtures only. All RP verification runs through the
//! production verifier over real loopback HTTP, never browser/E2E automation.
#[path = "support/init.rs"]
mod init_fixture;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::oneshot,
    task::JoinHandle,
};
use yuzora_companion::{
    admin::{self, CommandBody, Init},
    clock::Clock,
    gateway::Gateway,
    http::{Server, SECURITY_HEADERS},
};
const ORIGIN: &str = "https://desktop.tailnet.ts.net:8443";
const REGISTER: &str = "/api/pair/webauthn/";
const AUTH: &str = "/api/auth/";
#[path = "support/authenticator.rs"]
mod authenticator;
use authenticator::Authenticator;
struct Reply {
    status: u16,
    body: Value,
    headers: String,
}
struct Login {
    cookie: String,
    binding: String,
    device_id: String,
}
struct Fixture {
    gateway: Arc<Gateway>,
    port: u16,
    events: Arc<Mutex<Vec<Value>>>,
    _root: tempfile::TempDir,
    tasks: Vec<JoinHandle<()>>,
    server: JoinHandle<bool>,
    _stop: oneshot::Sender<()>,
}
impl Fixture {
    async fn new(policy: &str, max: u32) -> Self {
        Self::restore(policy, max, Vec::new(), true).await
    }
    async fn restore(
        policy: &str,
        max: u32,
        devices: Vec<yuzora_companion::devices::Device>,
        full: bool,
    ) -> Self {
        let mut value: Value = serde_json::from_str(&init_fixture::json()).unwrap();
        value["policy"]["passkeyBinding"] = json!(policy);
        value["policy"]["maxDevices"] = json!(max);
        value["policy"]["fullSessionEnabled"] = json!(true);
        value["policy"]["fullSessionEnabled"] = json!(full);
        value["devices"] = serde_json::to_value(devices).unwrap();
        Self::from_init(value).await
    }
    async fn from_init(value: Value) -> Self {
        let init: Init = serde_json::from_value(value).unwrap();
        let (output, input) = tokio::io::duplex(262144);
        let (sender, writer) = admin::spawn_writer(output);
        let events = Arc::new(Mutex::new(Vec::new()));
        let collected = events.clone();
        let reader = tokio::spawn(async move {
            let mut input = BufReader::new(input);
            loop {
                let mut line = String::new();
                if input.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                collected
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&line).unwrap());
            }
        });
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let gateway = Gateway::new(&init, sender, clock).unwrap();
        let root = tempfile::tempdir().unwrap();
        let server = Server::bind(&init.listeners.loopback, root.path(), gateway.clone())
            .await
            .unwrap();
        let port = server.port().unwrap();
        let (stop, stopped) = oneshot::channel();
        Self {
            gateway,
            port,
            events,
            _root: root,
            tasks: vec![
                reader,
                tokio::spawn(async move {
                    writer.await.unwrap().unwrap();
                }),
            ],
            server: tokio::spawn(server.run(stopped)),
            _stop: stop,
        }
    }
    fn admin(&self, value: Value) {
        let kind = value["type"].as_str().unwrap();
        self.gateway
            .command(if kind.starts_with("pairing.") {
                CommandBody::Pairing(serde_json::from_value(value).unwrap())
            } else {
                CommandBody::Device(serde_json::from_value(value).unwrap())
            })
            .unwrap();
    }
    async fn event(&self, typ: &str) -> Value {
        for _ in 0..20000 {
            let index = {
                self.events
                    .lock()
                    .unwrap()
                    .iter()
                    .position(|e| e["type"] == typ)
            };
            if let Some(index) = index {
                return self.events.lock().unwrap().remove(index);
            }
            tokio::task::yield_now().await;
        }
        panic!("missing {typ}");
    }
    async fn post(&self, path: &str, body: Value, login: Option<&Login>) -> Reply {
        let credentials = login.map_or(String::new(), |l| {
            format!("Cookie: {}\r\nX-Yzc-Binding: {}\r\n", l.cookie, l.binding)
        });
        self.post_headers(path, body, &credentials).await
    }
    async fn post_headers(&self, path: &str, body: Value, extra: &str) -> Reply {
        let request=format!("POST {path} HTTP/1.1\r\nHost: desktop.tailnet.ts.net:8443\r\nTailscale-User-Login: user@example.com\r\nOrigin: {ORIGIN}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.to_string().len());
        let port = self.port;
        let task = tokio::spawn(async move {
            let mut socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            socket.write_all(request.as_bytes()).await.unwrap();
            let mut text = String::new();
            socket.read_to_string(&mut text).await.unwrap();
            text
        });
        // Prevent auto-advance of a paused clock while real OS loopback IO settles.
        for _ in 0..100000 {
            if task.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(task.is_finished(), "loopback response deadline");
        let text = task.await.unwrap();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        for (name, value) in SECURITY_HEADERS {
            assert!(headers.contains(&format!("{name}: {value}")));
        }
        assert!(headers.contains("cache-control: no-store"));
        Reply {
            status: headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
            body: serde_json::from_str(body).unwrap_or(json!(body)),
            headers: headers.into(),
        }
    }
    async fn approved(&self, target: Option<&str>) -> (String, Value, Value) {
        self.admin(json!({"v":1,"type":"pairing.create","path":"tailscale","addToDevice":target}));
        let event = self.event("pairing.created").await;
        let (id, secret) = event["url"]
            .as_str()
            .unwrap()
            .split_once("#p=")
            .unwrap()
            .1
            .split_once('.')
            .unwrap();
        let claim=self.post("/api/pair/claim",json!({"pairingId":id,"secret":secret,"deviceLabel":"Phone","deviceNonce":B64.encode([7;32])}),None).await;
        assert_eq!(claim.status, 200);
        self.admin(json!({"v":1,"type":"pairing.approve","pairingId":id}));
        let status = self
            .post(
                "/api/pair/status",
                json!({"pairingId":id,"claimToken":claim.body["claimToken"]}),
                None,
            )
            .await;
        assert_eq!(status.status, 200);
        (
            id.into(),
            claim.body["claimToken"].clone(),
            status.body["registrationTicket"].clone(),
        )
    }
    async fn options(&self, target: Option<&str>) -> (String, Value) {
        let (id, _, ticket) = self.approved(target).await;
        let options = self
            .post(
                &format!("{REGISTER}options"),
                json!({"pairingId":id,"registrationTicket":ticket}),
                None,
            )
            .await;
        assert_eq!(options.status, 200, "{}", options.body);
        (id, options.body)
    }
    async fn finish(&self, id: &str, response: Value) -> Reply {
        self.post(
            &format!("{REGISTER}finish"),
            json!({"pairingId":id,"credential":response}),
            None,
        )
        .await
    }
    async fn register(&self, auth: &Authenticator, target: Option<&str>) -> String {
        let (id, options) = self.options(target).await;
        let result = self
            .finish(&id, auth.response(&options, true, 10, "valid"))
            .await;
        assert_eq!(result.status, 200, "{}", result.body);
        let device = result.body["deviceId"].as_str().unwrap();
        assert_eq!(device, options["user"]["id"]);
        device.into()
    }
    async fn assertion(&self, auth: &Authenticator, device: &str, count: u32, case: &str) -> Value {
        let mut options = self.post(&format!("{AUTH}options"), json!({}), None).await;
        assert_eq!(options.status, 200);
        assert!(options.body["allowCredentials"]
            .as_array()
            .is_none_or(Vec::is_empty));
        options.body["user"] = json!({"id":device});
        auth.response(&options.body, false, count, case)
    }
    async fn login(&self, auth: &Authenticator, device: &str, count: u32) -> Login {
        let credential = self.assertion(auth, device, count, "valid").await;
        let reply = self
            .post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None,
            )
            .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        Self::login_reply(reply)
    }
    fn login_reply(reply: Reply) -> Login {
        let cookie = reply
            .headers
            .lines()
            .find_map(|l| l.strip_prefix("set-cookie: "))
            .unwrap();
        assert!(cookie.ends_with("; Secure; HttpOnly; SameSite=Strict; Path=/"));
        assert_eq!(reply.body["scopes"], json!(["view"]));
        Login {
            cookie: cookie.split(';').next().unwrap().into(),
            binding: reply.body["binding"].as_str().unwrap().into(),
            device_id: reply.body["deviceId"].as_str().unwrap().into(),
        }
    }
    fn grant(&self, id: &str) {
        self.admin(json!({"v":1,"type":"device.update","deviceId":id,"grants":{"control":{"until":"2030-01-01T01:00:00Z"},"full":true}}));
    }
    async fn elevate(&self, auth: &Authenticator, login: &Login, scope: &str, count: u32) -> Reply {
        let mut options = self
            .post(
                &format!("{AUTH}elevate/options"),
                json!({"scope":scope}),
                Some(login),
            )
            .await;
        assert_eq!(options.status, 200, "{}", options.body);
        options.body["user"] = json!({"id":login.device_id});
        let credential = auth.response(&options.body, false, count, "valid");
        self.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":scope,"credential":credential}),
            Some(login),
        )
        .await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_algorithms_discoverable_registration_login_fresh_cookie_binding() {
    for algorithm in ["ES256", "EdDSA", "RS256"] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new(algorithm, 7, false);
        let (pair, options) = f.options(None).await;
        assert_eq!(options["authenticatorSelection"]["residentKey"], "required");
        assert_eq!(
            options["authenticatorSelection"]["userVerification"],
            "required"
        );
        assert_eq!(options["user"]["name"], "yuzora-companion");
        assert_eq!(options["user"]["displayName"], "Phone");
        assert_eq!(options["timeout"], 120000);
        assert_eq!(options["attestation"], "none");
        assert!(options["extensions"]
            .get("credentialProtectionPolicy")
            .is_none());
        assert_eq!(
            B64.decode(options["challenge"].as_str().unwrap())
                .unwrap()
                .len(),
            16
        );
        let algorithms: Vec<_> = options["pubKeyCredParams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["alg"].as_i64().unwrap())
            .collect();
        assert_eq!(algorithms, vec![-8, -7, -257]);
        let response = a.response(&options, true, 10, "valid");
        let device = f.finish(&pair, response.clone()).await;
        assert_eq!(device.status, 200);
        assert_eq!(f.finish(&pair, response).await.status, 403);
        let device = device.body["deviceId"].as_str().unwrap();
        let event = f.event("device.registered").await;
        assert_eq!(
            event["device"]["credentials"][0]["stateFormat"],
            "webauthn_rp-0.3.0-static"
        );
        let first = f.login(&a, device, 11).await;
        let credential = f.assertion(&a, device, 12, "valid").await;
        let second = f
            .post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                Some(&first),
            )
            .await;
        assert_eq!(second.status, 200);
        let second = Fixture::login_reply(second);
        assert_ne!(first.cookie, second.cookie);
        assert_ne!(first.binding, second.binding);
        assert_eq!(B64.decode(second.binding).unwrap().len(), 32);
        let device = f
            .gateway
            .identity
            .lock()
            .unwrap()
            .device(device)
            .unwrap()
            .clone();
        assert_eq!(device.credentials[0].sign_count, 12);
    }
}
#[tokio::test(start_paused = true)]
async fn webauthn_registration_negative_bindings_flags_type_replay_and_cancel() {
    for case in [
        "origin",
        "cross",
        "rp",
        "uv",
        "up",
        "challenge",
        "type",
        "bs",
        "cancel",
        "expiry",
    ] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new("ES256", 7, false);
        let (pair, options) = f.options(None).await;
        let response = a.response(&options, true, 10, case);
        if case == "cancel" {
            f.admin(json!({"v":1,"type":"pairing.cancel","pairingId":pair}));
        }
        if case == "expiry" {
            tokio::time::advance(Duration::from_secs(120)).await;
        }
        assert_eq!(f.finish(&pair, response).await.status, 403, "{case}");
        assert_eq!(
            f.finish(&pair, a.response(&options, true, 10, "valid"))
                .await
                .status,
            if case == "challenge" { 200 } else { 403 },
            "single use {case}"
        );
        assert_eq!(
            f.gateway.identity.lock().unwrap().devices().count(),
            usize::from(case == "challenge")
        );
    }
}
#[tokio::test(start_paused = true)]
async fn webauthn_registration_o1_rejects_backup_o2_o3_accept_and_max_devices() {
    for policy in ["o1", "o2", "o3"] {
        let f = Fixture::new(policy, 1).await;
        let a = Authenticator::new("ES256", 7, true);
        let (pair, options) = f.options(None).await;
        let result = f
            .finish(&pair, a.response(&options, true, 10, "valid"))
            .await;
        assert_eq!(result.status, if policy == "o1" { 403 } else { 200 });
        if policy != "o1" {
            let b = Authenticator::new("EdDSA", 8, false);
            let (pair, options) = f.options(None).await;
            assert_eq!(
                f.finish(&pair, b.response(&options, true, 10, "valid"))
                    .await
                    .status,
                403
            );
            assert_eq!(f.gateway.identity.lock().unwrap().devices().count(), 1);
        }
    }
}
#[tokio::test(start_paused = true)]
async fn webauthn_add_to_device_excludes_existing_caps_eight_and_emits_updated() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, true);
    let device = f.register(&a, None).await;
    f.event("device.registered").await;
    for byte in 8..15 {
        tokio::time::advance(Duration::from_secs(1)).await;
        let b = Authenticator::new("EdDSA", byte, false);
        let (pair, options) = f.options(Some(&device)).await;
        assert_eq!(
            options["authenticatorSelection"]["residentKey"],
            "preferred"
        );
        assert_eq!(options["user"]["id"], device);
        assert_eq!(
            options["excludeCredentials"].as_array().unwrap().len(),
            (byte - 7) as usize
        );
        assert_eq!(
            f.finish(&pair, b.response(&options, true, 10, "valid"))
                .await
                .status,
            200
        );
        let event = f.event("device.updated").await;
        assert_eq!(
            event["device"]["credentials"].as_array().unwrap().len(),
            (byte - 6) as usize
        );
    }
    assert_eq!(
        f.gateway
            .identity
            .lock()
            .unwrap()
            .device(&device)
            .unwrap()
            .credentials
            .len(),
        8
    );
    let command = serde_json::from_value(
        json!({"v":1,"type":"pairing.create","path":"tailscale","addToDevice":device}),
    )
    .unwrap();
    assert!(f.gateway.command(CommandBody::Pairing(command)).is_err());
}
#[tokio::test(start_paused = true)]
async fn webauthn_login_user_handle_mismatch_unknown_credential_and_locked() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let b = Authenticator::new("EdDSA", 8, false);
    let first = f.register(&a, None).await;
    let second = f.register(&b, None).await;
    let credential = f.assertion(&a, &second, 11, "valid").await;
    assert_eq!(
        f.post(
            &format!("{AUTH}finish"),
            json!({"credential":credential}),
            None
        )
        .await
        .status,
        403
    );
    let unknown = Authenticator::new("ES256", 9, false);
    let credential = f.assertion(&unknown, &first, 11, "valid").await;
    assert_eq!(
        f.post(
            &format!("{AUTH}finish"),
            json!({"credential":credential}),
            None
        )
        .await
        .status,
        403
    );
    for _ in 0..4 {
        let credential = f.assertion(&a, &first, 11, "signature").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
    }
    assert!(
        !f.gateway
            .identity
            .lock()
            .unwrap()
            .device(&first)
            .unwrap()
            .locked
    );
    let credential = f.assertion(&a, &first, 11, "valid").await;
    assert_eq!(
        f.post(
            &format!("{AUTH}finish"),
            json!({"credential":credential}),
            None
        )
        .await
        .status,
        200
    );
}
#[tokio::test(start_paused = true)]
async fn webauthn_five_failures_lock_clear_sessions_unlock_and_success_resets() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    let login = f.login(&a, &device, 11).await;
    for _ in 0..4 {
        let credential = f.assertion(&a, &device, 12, "origin").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
    }
    f.login(&a, &device, 12).await;
    for i in 0..5 {
        let credential = f.assertion(&a, &device, 13, "origin").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
        assert_eq!(
            f.gateway
                .identity
                .lock()
                .unwrap()
                .device(&device)
                .unwrap()
                .locked,
            i == 4
        );
    }
    assert_eq!(f.gateway.identity.lock().unwrap().session_count(), 0);
    assert_eq!(
        f.post(&format!("{AUTH}logout"), json!({}), Some(&login))
            .await
            .status,
        401
    );
    f.admin(json!({"v":1,"type":"device.unlock","deviceId":device}));
    f.login(&a, &device, 13).await;
}
#[tokio::test(start_paused = true)]
async fn webauthn_sign_count_rollback_immediately_locks() {
    for counter in [0, 9, 10] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new("ES256", 7, false);
        let device = f.register(&a, None).await;
        let credential = f.assertion(&a, &device, counter, "valid").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
        assert!(
            f.gateway
                .identity
                .lock()
                .unwrap()
                .device(&device)
                .unwrap()
                .locked
        );
    }
}
#[tokio::test(start_paused = true)]
async fn webauthn_login_negative_origin_uv_up_rp_type_cross_and_expiry() {
    for case in ["origin", "uv", "up", "rp", "type", "cross", "expiry"] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new("ES256", 7, false);
        let device = f.register(&a, None).await;
        let credential = f.assertion(&a, &device, 11, case).await;
        if case == "expiry" {
            tokio::time::advance(Duration::from_secs(120)).await;
        }
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403,
            "{case}"
        );
        assert_eq!(f.gateway.identity.lock().unwrap().session_count(), 0);
    }
}
#[tokio::test(start_paused = true)]
async fn webauthn_o2_elevation_filters_backup_credentials_and_scopes_independent() {
    let f = Fixture::new("o2", 5).await;
    let synced = Authenticator::new("ES256", 7, true);
    let hardware = Authenticator::new("EdDSA", 8, false);
    let device = f.register(&synced, None).await;
    f.register(&hardware, Some(&device)).await;
    f.grant(&device);
    let login = f.login(&synced, &device, 11).await;
    let mut options = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"control"}),
            Some(&login),
        )
        .await;
    assert_eq!(options.status, 200);
    assert_eq!(
        options.body["allowCredentials"].as_array().unwrap().len(),
        1
    );
    assert_eq!(options.body["allowCredentials"][0]["id"], hardware.id);
    options.body["user"] = json!({"id":device});
    let credential = synced.response(&options.body, false, 12, "valid");
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"control","credential":credential}),
            Some(&login)
        )
        .await
        .status,
        403
    );
    assert_eq!(
        f.elevate(&hardware, &login, "control", 11).await.status,
        200
    );
    let full = f.elevate(&hardware, &login, "full", 12).await;
    assert_eq!(full.status, 200);
    let headers = {
        let mut h = axum::http::HeaderMap::new();
        h.insert("cookie", login.cookie.parse().unwrap());
        h.insert("x-yzc-binding", login.binding.parse().unwrap());
        h
    };
    let ctx = f
        .gateway
        .authenticate(&headers, admin::Scope::View)
        .unwrap();
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Control, |_| ())
        .is_ok());
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Full, |_| ())
        .is_ok());
    tokio::time::advance(Duration::from_secs(120)).await;
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Full, |_| ())
        .is_ok());
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Control, |_| ())
        .is_ok());
    tokio::time::advance(Duration::from_secs(780)).await;
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Full, |_| ())
        .is_err());
    assert!(f
        .gateway
        .with_authorized(&ctx, admin::Scope::Control, |_| ())
        .is_err());
}
#[tokio::test(start_paused = true)]
async fn webauthn_elevation_requires_grant_policy_and_revoke_revalidated() {
    let f = Fixture::new("o3", 5).await;
    let a = Authenticator::new("ES256", 7, true);
    let device = f.register(&a, None).await;
    let login = f.login(&a, &device, 11).await;
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"full"}),
            Some(&login)
        )
        .await
        .status,
        403
    );
    f.grant(&device);
    let login = f.login(&a, &device, 12).await;
    assert_eq!(f.elevate(&a, &login, "full", 13).await.status, 200);
    let mut options = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"control"}),
            Some(&login),
        )
        .await;
    assert_eq!(options.status, 200);
    options.body["user"] = json!({"id":device});
    let credential = a.response(&options.body, false, 14, "valid");
    f.admin(json!({"v":1,"type":"device.revoke","deviceId":device}));
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"control","credential":credential}),
            Some(&login)
        )
        .await
        .status,
        401
    );
}
#[tokio::test(start_paused = true)]
async fn webauthn_per_device_four_sessions_evict_oldest_and_challenges_bounded() {
    let f = Fixture::new("o2", 1).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    let first = f.login(&a, &device, 11).await;
    for count in 12..=15 {
        tokio::time::advance(Duration::from_secs(1)).await;
        f.login(&a, &device, count).await;
    }
    assert_eq!(f.gateway.identity.lock().unwrap().session_count(), 4);
    assert_eq!(
        f.post(&format!("{AUTH}logout"), json!({}), Some(&first))
            .await
            .status,
        401
    );
    for _ in 0..64 {
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(
            f.post(&format!("{AUTH}options"), json!({}), None)
                .await
                .status,
            200
        );
    }
    assert_eq!(
        f.post(&format!("{AUTH}options"), json!({}), None)
            .await
            .status,
        200
    );
    tokio::time::advance(Duration::from_secs(120)).await;
    f.gateway.maintenance();
    assert_eq!(
        f.post(&format!("{AUTH}options"), json!({}), None)
            .await
            .status,
        200
    );
}

#[tokio::test(start_paused = true)]
async fn webauthn_restored_o1_login_rejects_synced_and_full_policy_disabled() {
    let original = Fixture::new("o3", 5).await;
    let synced = Authenticator::new("ES256", 7, true);
    let device = original.register(&synced, None).await;
    original.grant(&device);
    let saved = original
        .gateway
        .identity
        .lock()
        .unwrap()
        .device(&device)
        .unwrap()
        .clone();
    let strict = Fixture::restore("o1", 5, vec![saved.clone()], true).await;
    let credential = strict.assertion(&synced, &device, 11, "valid").await;
    assert_eq!(
        strict
            .post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
        403
    );
    let disabled = Fixture::restore("o3", 5, vec![saved], false).await;
    let login = disabled.login(&synced, &device, 11).await;
    assert_eq!(
        disabled
            .post(
                &format!("{AUTH}elevate/options"),
                json!({"scope":"full"}),
                Some(&login)
            )
            .await
            .status,
        403
    );
    assert_eq!(
        disabled
            .elevate(&synced, &login, "control", 12)
            .await
            .status,
        200
    );
}
#[tokio::test(start_paused = true)]
async fn webauthn_assertion_replay_wrong_scope_per_session_cap_and_null_user_handle() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.grant(&device);
    let response = f.assertion(&a, &device, 11, "valid").await;
    let reply = f
        .post(
            &format!("{AUTH}finish"),
            json!({"credential":response}),
            None,
        )
        .await;
    assert_eq!(reply.status, 200);
    let login = Fixture::login_reply(reply);
    assert_eq!(
        f.post(
            &format!("{AUTH}finish"),
            json!({"credential":response}),
            None
        )
        .await
        .status,
        403
    );
    let mut options = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"full"}),
            Some(&login),
        )
        .await;
    assert_eq!(options.status, 200);
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"control"}),
            Some(&login)
        )
        .await
        .status,
        200
    );
    options.body["user"] = json!({"id":device});
    let response = a.response(&options.body, false, 12, "valid");
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"control","credential":response}),
            Some(&login)
        )
        .await
        .status,
        403
    );
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"full","credential":response}),
            Some(&login)
        )
        .await
        .status,
        403
    );
    let mut options = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"full"}),
            Some(&login),
        )
        .await;
    assert_eq!(options.status, 200);
    options.body["user"] = json!({"id":device});
    let mut response = a.response(&options.body, false, 12, "valid");
    response["response"]["userHandle"] = Value::Null;
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"full","credential":response}),
            Some(&login)
        )
        .await
        .status,
        200
    );
}
#[tokio::test(start_paused = true)]
async fn webauthn_add_finish_rechecks_device_revoke_and_duplicate_credentials() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    let (pair, options) = f.options(Some(&device)).await;
    assert_eq!(
        f.finish(&pair, a.response(&options, true, 10, "valid"))
            .await
            .status,
        403
    );
    let (pair, options) = f.options(Some(&device)).await;
    let b = Authenticator::new("EdDSA", 8, false);
    f.admin(json!({"v":1,"type":"device.revoke","deviceId":device}));
    assert_eq!(
        f.finish(&pair, b.response(&options, true, 10, "valid"))
            .await
            .status,
        403
    );
    assert_eq!(f.gateway.identity.lock().unwrap().devices().count(), 0);
}

#[tokio::test(start_paused = true)]
async fn webauthn_o2_control_rejects_synced_assertion() {
    let f = Fixture::new("o2", 5).await;
    let synced = Authenticator::new("ES256", 7, true);
    let hardware = Authenticator::new("EdDSA", 8, false);
    let device = f.register(&synced, None).await;
    f.register(&hardware, Some(&device)).await;
    f.grant(&device);
    let login = f.login(&synced, &device, 11).await;
    let mut options = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"control"}),
            Some(&login),
        )
        .await;
    assert_eq!(options.status, 200);
    options.body["user"] = json!({"id":device});
    let credential = synced.response(&options.body, false, 12, "valid");
    let finish = f
        .post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"control","credential":credential}),
            Some(&login),
        )
        .await;
    assert_eq!(
        finish.status, 403,
        "a valid synced signature cannot authorize control"
    );
}

#[path = "support/websocket.rs"]
mod websocket;

#[tokio::test(start_paused = true)]
async fn webauthn_unverified_garbage_never_locks_victim_or_attributes_outcome() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    let registered = f.event("auth.outcome").await;
    assert_eq!(registered["deviceId"], device);
    for case in [
        "garbage",
        "signature",
        "handle",
        "challenge",
        "expiry",
        "type",
        "up",
    ] {
        for _ in 0..5 {
            let credential = if case == "garbage" {
                json!({"rawId":a.id})
            } else {
                let credential = f
                    .assertion(
                        &a,
                        if case == "handle" {
                            "AAAAAAAAAAAAAAAAAAAAAA"
                        } else {
                            &device
                        },
                        11,
                        case,
                    )
                    .await;
                if case == "expiry" {
                    tokio::time::advance(Duration::from_secs(120)).await;
                }
                credential
            };
            assert_eq!(
                f.post(
                    &format!("{AUTH}finish"),
                    json!({"credential":credential}),
                    None
                )
                .await
                .status,
                403
            );
            assert!(
                !f.gateway
                    .identity
                    .lock()
                    .unwrap()
                    .device(&device)
                    .unwrap()
                    .locked,
                "{case}"
            );
            let event = f.event("auth.outcome").await;
            assert_eq!(event["outcome"], "rejected");
            assert!(event["deviceId"].is_null());
            tokio::time::advance(Duration::from_millis(200)).await;
        }
    }
    f.login(&a, &device, 11).await;
    assert_eq!(f.event("auth.outcome").await["deviceId"], device);
}

#[tokio::test(start_paused = true)]
async fn webauthn_registration_junk_finish_preserves_real_challenge() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let (pair, options) = f.options(None).await;
    assert_eq!(f.finish(&pair, json!({"rawId":a.id})).await.status, 403);
    assert_eq!(
        f.finish(&pair, a.response(&options, true, 10, "challenge"))
            .await
            .status,
        403
    );
    let valid = a.response(&options, true, 10, "valid");
    assert_eq!(
        f.finish("AAAAAAAAAAAAAAAAAAAAAA", valid.clone())
            .await
            .status,
        403
    );
    assert_eq!(f.finish(&pair, valid).await.status, 200);
}

#[tokio::test(start_paused = true)]
async fn webauthn_elevation_options_replace_cancelled_same_and_other_scope() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.grant(&device);
    let login = f.login(&a, &device, 11).await;
    for (old, new, count) in [
        ("control", "control", 12),
        ("control", "full", 13),
        ("full", "control", 14),
    ] {
        let first = f
            .post(
                &format!("{AUTH}elevate/options"),
                json!({"scope":old}),
                Some(&login),
            )
            .await;
        let mut replacement = f
            .post(
                &format!("{AUTH}elevate/options"),
                json!({"scope":new}),
                Some(&login),
            )
            .await;
        assert_eq!(first.status, 200);
        assert_eq!(replacement.status, 200);
        assert_ne!(first.body["challenge"], replacement.body["challenge"]);
        replacement.body["user"] = json!({"id":device});
        let credential = a.response(&replacement.body, false, count, "valid");
        assert_eq!(
            f.post(
                &format!("{AUTH}elevate/finish"),
                json!({"scope":new,"credential":credential}),
                Some(&login)
            )
            .await
            .status,
            200
        );
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_login_pool_evicts_oldest_reserves_registration_and_elevation() {
    let f = Fixture::new("o2", 1).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.grant(&device);
    let login = f.login(&a, &device, 11).await;
    let mut oldest = f
        .post(&format!("{AUTH}options"), json!({}), None)
        .await
        .body;
    oldest["user"] = json!({"id":device});
    for _ in 0..48 {
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(
            f.post(&format!("{AUTH}options"), json!({}), None)
                .await
                .status,
            200
        );
    }
    // A full anonymous pool cannot starve these authenticated/reserved paths.
    assert_eq!(f.elevate(&a, &login, "control", 12).await.status, 200);
    let b = Authenticator::new("EdDSA", 8, false);
    let (pair, options) = f.options(Some(&device)).await;
    assert_eq!(
        f.finish(&pair, b.response(&options, true, 10, "valid"))
            .await
            .status,
        200
    );
    let credential = a.response(&oldest, false, 13, "valid");
    assert_eq!(
        f.post(
            &format!("{AUTH}finish"),
            json!({"credential":credential}),
            None
        )
        .await
        .status,
        403
    );
    assert!(
        !f.gateway
            .identity
            .lock()
            .unwrap()
            .device(&device)
            .unwrap()
            .locked
    );
    f.login(&a, &device, 13).await;
}

#[tokio::test(start_paused = true)]
async fn webauthn_registration_target_failure_restores_approved_fresh_ticket() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    let (pair, token, ticket) = f.approved(Some(&device)).await;
    f.admin(json!({"v":1,"type":"device.revoke","deviceId":device}));
    assert_eq!(
        f.post(
            &format!("{REGISTER}options"),
            json!({"pairingId":pair,"registrationTicket":ticket}),
            None
        )
        .await
        .status,
        403
    );
    let status = f
        .post(
            "/api/pair/status",
            json!({"pairingId":pair,"claimToken":token}),
            None,
        )
        .await;
    assert_eq!(status.body["state"], "approved");
    assert!(status.body["registrationTicket"].is_string());
    assert_ne!(status.body["registrationTicket"], ticket);
    assert_eq!(
        f.post(
            &format!("{REGISTER}options"),
            json!({"pairingId":pair,"registrationTicket":ticket}),
            None
        )
        .await
        .status,
        403
    );
    tokio::time::advance(Duration::from_secs(600)).await;
    let expired = f
        .post(
            "/api/pair/status",
            json!({"pairingId":pair,"claimToken":token}),
            None,
        )
        .await;
    assert_eq!(expired.body["state"], "expired");
}

#[tokio::test(start_paused = true)]
async fn webauthn_registration_l3_json_cross_checks_backup_eligible_not_backed_up() {
    for algorithm in ["ES256", "EdDSA", "RS256"] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new(algorithm, 7, true);
        let (pair, options) = f.options(None).await;
        let valid = a.response(&options, true, 10, "eligible");
        assert!(valid["response"]["authenticatorData"].is_string());
        assert!(valid["response"]["publicKey"].is_string());
        assert!(valid["response"]["publicKeyAlgorithm"].is_number());
        for field in ["authenticatorData", "publicKey", "publicKeyAlgorithm"] {
            let mut invalid = valid.clone();
            invalid["response"][field] = if field == "publicKeyAlgorithm" {
                json!(0)
            } else {
                json!(B64.encode([0; 32]))
            };
            assert_eq!(
                f.finish(&pair, invalid).await.status,
                403,
                "{algorithm} {field}"
            );
        }
        let reply = f.finish(&pair, valid).await;
        assert_eq!(reply.status, 200, "{algorithm}");
        let id = reply.body["deviceId"].as_str().unwrap();
        let event = f.event("device.registered").await;
        let record = &event["device"]["credentials"][0];
        assert_eq!(record["backupEligible"], true);
        assert_eq!(record["backupState"], false);
        assert_eq!(record["transports"].as_array().unwrap().len(), 6);
        let credential = f.assertion(&a, id, 11, "eligible").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            200
        );
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_locked_device_http_does_not_repeat_snapshot() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.event("auth.outcome").await;
    for _ in 0..5 {
        let credential = f.assertion(&a, &device, 11, "origin").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
        assert!(f.event("auth.outcome").await["deviceId"].is_null());
    }
    assert!(
        f.gateway
            .identity
            .lock()
            .unwrap()
            .device(&device)
            .unwrap()
            .locked
    );
    let count = || {
        f.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "device.updated")
            .count()
    };
    assert_eq!(count(), 1);
    for _ in 0..5 {
        let credential = f.assertion(&a, &device, 11, "origin").await;
        assert_eq!(
            f.post(
                &format!("{AUTH}finish"),
                json!({"credential":credential}),
                None
            )
            .await
            .status,
            403
        );
        f.event("auth.outcome").await;
        assert_eq!(count(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_signature_verified_policy_variants_count_toward_lockout() {
    for case in ["origin", "cross", "top", "rp", "uv", "extension"] {
        let f = Fixture::new("o2", 5).await;
        let a = Authenticator::new("ES256", 7, false);
        let device = f.register(&a, None).await;
        f.grant(&device);
        let login = f.login(&a, &device, 11).await;
        for i in 0..5 {
            // Challenge-bound state reaches post-signature policy checks.
            // Unknown/stale challenge rejection is separately tested below.
            let mut options = f
                .post(
                    &format!("{AUTH}elevate/options"),
                    json!({"scope":"control"}),
                    Some(&login),
                )
                .await;
            assert_eq!(options.status, 200);
            options.body["user"] = json!({"id":device});
            let credential = a.response(&options.body, false, 12, case);
            assert_eq!(
                f.post(
                    &format!("{AUTH}elevate/finish"),
                    json!({"scope":"control","credential":credential}),
                    Some(&login)
                )
                .await
                .status,
                403
            );
            assert_eq!(
                f.gateway
                    .identity
                    .lock()
                    .unwrap()
                    .device(&device)
                    .unwrap()
                    .locked,
                i == 4,
                "{case}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn webauthn_replaced_elevation_old_and_unknown_challenges_preserve_new_ceremony() {
    let f = Fixture::new("o2", 5).await;
    let a = Authenticator::new("ES256", 7, false);
    let device = f.register(&a, None).await;
    f.grant(&device);
    let login = f.login(&a, &device, 11).await;
    let mut old = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"control"}),
            Some(&login),
        )
        .await;
    old.body["user"] = json!({"id":device});
    let stale = a.response(&old.body, false, 12, "valid");
    let mut new = f
        .post(
            &format!("{AUTH}elevate/options"),
            json!({"scope":"full"}),
            Some(&login),
        )
        .await;
    assert_eq!(new.status, 200);
    new.body["user"] = json!({"id":device});
    let unknown = a.response(&new.body, false, 12, "challenge");
    let valid = a.response(&new.body, false, 12, "valid");
    // Neither unknown nor stale challenge, nor a valid challenge under a wrong
    // scope/session, may consume the currently usable Full ceremony.
    let other_login = f.login(&a, &device, 12).await;
    for (credential, scope, session) in
        [(&valid, "control", &login), (&valid, "full", &other_login)]
    {
        assert_eq!(
            f.post(
                &format!("{AUTH}elevate/finish"),
                json!({"scope":scope,"credential":credential}),
                Some(session)
            )
            .await
            .status,
            403
        );
    }
    // Five signed stale/unknown attempts do not lock; direct count invariant is
    // also checked in session_tests against an existing nonzero failure count.
    for _ in 0..5 {
        for credential in [&stale, &unknown] {
            assert_eq!(
                f.post(
                    &format!("{AUTH}elevate/finish"),
                    json!({"scope":"full","credential":credential}),
                    Some(&login)
                )
                .await
                .status,
                403
            );
        }
    }
    assert!(
        !f.gateway
            .identity
            .lock()
            .unwrap()
            .device(&device)
            .unwrap()
            .locked
    );
    let valid = a.response(&new.body, false, 13, "valid");
    assert_eq!(
        f.post(
            &format!("{AUTH}elevate/finish"),
            json!({"scope":"full","credential":valid}),
            Some(&login)
        )
        .await
        .status,
        200
    );
}
