//! Fake sidecars are shell scripts speaking the net NDJSON protocol.
use super::*;
use crate::companion_service::GATEWAY_GIT_ENV;
use std::collections::BTreeSet;

const RUNNING: &str = r#"{"v":1,"type":"running","dnsName":"desktop.tail1234.ts.net","login":"user@example.com","displayName":"User","httpsCapable":true,"magicDns":true}"#;
const CERT_READY: &str =
    r#"{"v":1,"type":"cert","state":"ready","notAfter":"2027-01-01T00:00:00Z","error":null}"#;
const SECRET: &str = "s3cr3t-s3cr3t-s3cr3t-s3cr3t-s3cr3t-s3cr3t";
const FAST: Timing = Timing {
    running: Duration::from_millis(600),
    cert: Duration::from_millis(600),
    serve: Duration::from_millis(500),
    unserve: Duration::from_millis(500),
    logout: Duration::from_millis(600),
    stop: Duration::from_millis(500),
};

struct Harness {
    _root: tempfile::TempDir,
    dir: PathBuf,
    bin: PathBuf,
    manager: Arc<NetManager>,
    events: Arc<Mutex<Vec<Value>>>,
    notices: Arc<Mutex<Vec<String>>>,
}
impl Harness {
    fn new(script: &str) -> Self {
        Self::with_timing(script, FAST)
    }
    fn with_timing(script: &str, timing: Timing) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("net");
        let bin = root.path().join("sidecar");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let notices = Arc::new(Mutex::new(Vec::new()));
        let (sink, seen) = (events.clone(), notices.clone());
        let manager = NetManager::with_timing(
            dir.clone(),
            "9.9.9".into(),
            None,
            Arc::new(move |event| sink.lock().unwrap().push(event)),
            Arc::new(move |notice| {
                seen.lock().unwrap().push(match notice {
                    Notice::Running => "running".to_string(),
                    Notice::LoginRequired => "login-required".to_string(),
                    Notice::Exited { requested } => format!("exited:{requested}"),
                })
            }),
            timing,
        );
        Self {
            _root: root,
            dir,
            bin,
            manager,
            events,
            notices,
        }
    }
    async fn until(&self, what: &str, done: impl Fn(&NetOverview) -> bool) {
        for _ in 0..500 {
            if done(&self.manager.overview()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "timed out waiting for {what}: {:?}",
            self.manager.overview()
        );
    }
    fn file(&self, suffix: &str) -> String {
        std::fs::read_to_string(format!("{}.{suffix}", self.bin.display())).unwrap_or_default()
    }
    /// Where the sidecar is found, as `logout` asks for it.
    fn program(&self) -> impl FnOnce() -> Result<PathBuf, String> + Send {
        let bin = self.bin.clone();
        move || Ok(bin)
    }
    fn notices(&self) -> Vec<String> {
        self.notices.lock().unwrap().clone()
    }
    /// A node this desktop saw run: tsnet has its state, settings its identity.
    fn registered(&self) {
        std::fs::create_dir_all(&self.dir).unwrap();
        std::fs::write(self.dir.join(STATE_FILE), "x").unwrap();
        self.manager.inner.lock().unwrap().identity = Some(NetIdentity {
            dns_name: "desktop.tail1234.ts.net".into(),
            login: "user@example.com".into(),
        });
    }
}

#[tokio::test]
async fn companion_net_start_sends_init_and_follows_the_node() {
    let h = Harness::new(&format!(
        r#"read init; printf '%s\n' "$init" > "$0.init"; env > "$0.env"
echo '{{"v":1,"type":"started","version":"9.9.9"}}'
echo '{{"v":1,"type":"state","backendState":"NeedsLogin"}}'
echo '{{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc"}}'
sleep 0.3
echo '{{"v":1,"type":"state","backendState":"Running"}}'
echo '{RUNNING}'
echo '{{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}}'
sleep 0.1
echo '{CERT_READY}'
while read line; do
  case "$line" in *'"shutdown"'*) echo shutdown > "$0.stopped"; exit 0;; esac
done"#
    ));
    assert_eq!(h.manager.overview().state, State::Stopped);
    assert_eq!(h.manager.overview().mode, None);
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    assert_eq!(h.manager.overview().state, State::Starting);
    assert_eq!(h.manager.overview().mode, Some(Mode::Enable));
    h.until("login url", |o| {
        o.state == State::NeedsLogin
            && o.auth_url.as_deref() == Some("https://login.tailscale.com/a/abc")
    })
    .await;
    h.until("cert pending", |o| {
        o.state == State::Running && o.cert.as_ref().map(|c| c.state) == Some(CertState::Pending)
    })
    .await;
    let running = h.manager.overview();
    assert_eq!(running.auth_url, None);
    assert_eq!(running.dns_name.as_deref(), Some("desktop.tail1234.ts.net"));
    assert_eq!(running.login.as_deref(), Some("user@example.com"));
    assert_eq!(running.https_capable, Some(true));
    assert!(!running.serving && !running.logged_in);
    h.until("cert ready", |o| {
        o.cert.as_ref().map(|c| c.state) == Some(CertState::Ready)
    })
    .await;
    let node = h.manager.wait_running().await.unwrap();
    assert_eq!(node.identity.dns_name, "desktop.tail1234.ts.net");
    assert!(node.https_capable);
    h.manager.wait_cert().await.unwrap();
    assert_eq!(
        h.manager.overview().cert.unwrap().not_after.as_deref(),
        Some("2027-01-01T00:00:00Z")
    );
    // The UI hears about every change; the last one is the current state.
    let overview = serde_json::to_value(h.manager.overview()).unwrap();
    assert_eq!(
        h.events.lock().unwrap().last().unwrap(),
        &json!({"type": "net", "net": overview})
    );
    assert_eq!(overview["state"], "running");
    assert_eq!(overview["cert"]["state"], "ready");
    // tsnet's state file is what says the node is registered.
    std::fs::write(h.dir.join(STATE_FILE), "x").unwrap();
    assert!(h.manager.overview().logged_in);

    let init: Value = serde_json::from_str(&h.file("init")).unwrap();
    assert_eq!(
        init,
        json!({"v": 1, "type": "init", "stateDir": h.dir.to_str().unwrap(), "hostname": "yuzora", "version": "9.9.9"})
    );
    // Only the gateway's allowlist and the proxy variables reach the node.
    for line in h.file("env").lines() {
        let key = line.split('=').next().unwrap();
        assert!(
            ENV_ALLOWLIST.contains(&key)
                || PROXY_ENV.contains(&key)
                || ["PWD", "SHLVL", "_", "OLDPWD"].contains(&key),
            "leaked {key}"
        );
    }

    h.manager.stop().await;
    let stopped = h.manager.overview();
    assert_eq!(stopped.state, State::Stopped);
    assert_eq!(
        (
            stopped.auth_url,
            stopped.cert,
            stopped.serving,
            stopped.error
        ),
        (None, None, false, None)
    );
    // Last seen, so the settings can show the node while it is offline.
    assert_eq!(stopped.dns_name.as_deref(), Some("desktop.tail1234.ts.net"));
    assert_eq!(stopped.mode, None);
    assert_eq!(h.file("stopped").trim(), "shutdown");
    assert_eq!(h.notices(), ["running", "exited:true"]);
}

#[tokio::test]
async fn companion_net_login_mode_reports_the_node_and_enable_takes_it_over() {
    let script = format!(
        r#"read init; sleep 0.3
echo '{RUNNING}'
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    );
    let login = Harness::new(&script);
    login.manager.start(&login.bin, Mode::Login).unwrap();
    assert_eq!(login.manager.mode(), Some(Mode::Login));
    login.until("running", |o| o.state == State::Running).await;
    assert_eq!(login.notices(), ["running"]);
    login.manager.stop().await;

    // Enable wants the same node: the login in progress becomes the enable.
    let taken = Harness::new(&script);
    taken.manager.start(&taken.bin, Mode::Login).unwrap();
    taken.manager.start(&taken.bin, Mode::Enable).unwrap();
    assert_eq!(taken.manager.mode(), Some(Mode::Enable));
    // The UI is told the sidecar now serves an enable.
    assert_eq!(
        taken.events.lock().unwrap().last().unwrap()["net"]["mode"],
        "enable"
    );
    taken.until("running", |o| o.state == State::Running).await;
    // Whatever the mode, a running node is reported: settings remember it.
    assert_eq!(taken.notices(), ["running"]);
    // A login never takes an enable's node over.
    taken.manager.start(&taken.bin, Mode::Login).unwrap();
    assert_eq!(taken.manager.mode(), Some(Mode::Enable));
    taken.manager.stop().await;
    assert_eq!(taken.manager.mode(), None);
}

#[tokio::test]
async fn companion_net_leaving_reports_no_mode_until_an_enable_starts_it_again() {
    let h = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    let last_mode = || h.events.lock().unwrap().last().unwrap()["net"]["mode"].clone();
    let published = || h.events.lock().unwrap().len();
    // Nothing runs: nothing to leave, nothing to say.
    h.manager.leaving();
    assert_eq!(published(), 0);

    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.until("running", |o| o.state == State::Running).await;
    assert_eq!(last_mode(), "enable");
    // The Companion is taken down: the UI is told the sidecar no longer runs
    // for an enable, while the logic still knows what it is.
    h.manager.leaving();
    assert_eq!(h.manager.overview().mode, None);
    assert_eq!(h.manager.mode(), Some(Mode::Enable));
    assert_eq!(last_mode(), Value::Null);
    let told = published();
    h.manager.leaving();
    assert_eq!(published(), told, "said once");

    // An enable that starts again takes the sidecar back.
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    assert_eq!(h.manager.overview().mode, Some(Mode::Enable));
    assert_eq!(last_mode(), "enable");

    // What was left does not outlive the sidecar: the next start is its own.
    h.manager.leaving();
    h.manager.stop().await;
    assert_eq!(h.manager.overview().mode, None);
    h.manager.start(&h.bin, Mode::Login).unwrap();
    assert_eq!(h.manager.overview().mode, Some(Mode::Login));
    assert_eq!(last_mode(), "login");
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_net_waits_end_with_what_the_node_reports() {
    let needs_login = Harness::new(
        r#"read init; echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc"}'; while read line; do :; done"#,
    );
    needs_login
        .manager
        .start(&needs_login.bin, Mode::Enable)
        .unwrap();
    assert_eq!(
        needs_login.manager.wait_running().await.err().unwrap(),
        "companion-net-login-required"
    );
    needs_login.manager.stop().await;

    let silent = Harness::new("read init; while read line; do :; done");
    silent.manager.start(&silent.bin, Mode::Enable).unwrap();
    assert_eq!(
        silent.manager.wait_running().await.err().unwrap(),
        "companion-net-start-failed"
    );
    silent.manager.stop().await;

    let refused = Harness::new(
        r#"read init; echo '{"v":1,"type":"error","code":"net-start-failed","message":"no"}'; exit 2"#,
    );
    refused.manager.start(&refused.bin, Mode::Enable).unwrap();
    assert_eq!(
        refused.manager.wait_running().await.err().unwrap(),
        "companion-net-start-failed"
    );
    let failed = refused.manager.overview();
    assert_eq!(failed.state, State::Failed);
    assert_eq!(failed.error.as_deref(), Some("companion-net-start-failed"));

    // The node retries a failed certificate itself; the wait is for ready.
    let no_cert = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'
echo '{{"v":1,"type":"cert","state":"error","notAfter":null,"error":"rate limited"}}'
while read line; do :; done"#
    ));
    no_cert.manager.start(&no_cert.bin, Mode::Enable).unwrap();
    no_cert.manager.wait_running().await.unwrap();
    assert_eq!(
        no_cert.manager.wait_cert().await.unwrap_err(),
        "companion-net-cert-failed"
    );
    let cert = no_cert.manager.overview().cert.unwrap();
    assert_eq!(cert.state, CertState::Error);
    assert_eq!(cert.error.as_deref(), Some("rate limited"));
    no_cert.manager.stop().await;
}

const NEEDS_APPROVAL: &str = r#"echo '{"v":1,"type":"state","backendState":"NeedsMachineAuth"}'"#;

#[tokio::test]
async fn companion_net_a_node_waiting_for_approval_says_so_and_the_enable_fails_at_once() {
    // Signed in; an admin has to approve the machine. No sign-in URL comes.
    let login = Harness::new(&format!(
        r#"read init
echo '{{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc"}}'
{NEEDS_APPROVAL}
sleep 0.3
echo '{{"v":1,"type":"state","backendState":"Running"}}'
echo '{RUNNING}'
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    login.manager.start(&login.bin, Mode::Login).unwrap();
    login
        .until("approval", |o| o.state == State::NeedsApproval)
        .await;
    let waiting = login.manager.overview();
    assert_eq!(
        (
            waiting.mode,
            waiting.auth_url.as_deref(),
            waiting.error.as_deref()
        ),
        (Some(Mode::Login), None, None)
    );
    let wire = serde_json::to_value(&waiting).unwrap();
    assert_eq!(
        (&wire["state"], &wire["authUrl"]),
        (&json!("needsApproval"), &Value::Null)
    );
    // The login URL was used up: the node is not asked to log in again, so
    // settings are not told it has to (it never was registered either).
    assert_eq!(login.notices(), Vec::<String>::new());
    // The admin approves it: the node runs, as for any sign-in.
    login.until("running", |o| o.state == State::Running).await;
    assert_eq!(login.manager.overview().auth_url, None);
    assert_eq!(login.notices(), ["running"]);
    login.manager.stop().await;

    // An enable on such a node says why at once, not "could not start" 20 s later.
    let enable = Harness::new(&format!(
        r#"read init; {NEEDS_APPROVAL}; while read line; do :; done"#
    ));
    enable.manager.start(&enable.bin, Mode::Enable).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        enable.manager.wait_running().await.err().unwrap(),
        "companion-net-approval-required"
    );
    assert!(
        started.elapsed() < FAST.running / 2,
        "{:?}",
        started.elapsed()
    );
    enable.manager.stop().await;
}

const NEEDS_APPROVAL_NAMED: &str = r#"echo '{"v":1,"type":"state","backendState":"NeedsMachineAuth","dnsName":"yuzora-1.tail1234.ts.net"}'"#;

#[tokio::test]
async fn companion_net_a_node_waiting_for_approval_names_the_machine_it_reported() {
    // The first sign-in: no identity yet, and the machine is named "yuzora-1"
    // because "yuzora" is taken. Then it is approved, and then it runs.
    let h = Harness::new(&format!(
        r#"read init
echo '{{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc"}}'
{NEEDS_APPROVAL_NAMED}
sleep 0.4
echo '{{"v":1,"type":"state","backendState":"Running"}}'
echo '{RUNNING}'
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    h.manager.start(&h.bin, Mode::Login).unwrap();
    h.until("approval", |o| o.state == State::NeedsApproval)
        .await;
    let waiting = h.manager.overview();
    assert_eq!(
        waiting.dns_name.as_deref(),
        Some("yuzora-1.tail1234.ts.net")
    );
    // The name is the pending machine's: nothing is registered or remembered.
    assert_eq!((waiting.logged_in, waiting.login.as_deref()), (false, None));
    assert_eq!(h.manager.identity(), None);
    assert_eq!(h.notices(), Vec::<String>::new());
    let wire = serde_json::to_value(&waiting).unwrap();
    assert_eq!(wire["dnsName"], "yuzora-1.tail1234.ts.net");
    assert_eq!(wire["loggedIn"], false);
    // The identity is the one `running` brings, and the name follows it.
    h.until("running", |o| o.state == State::Running).await;
    let running = h.manager.overview();
    assert_eq!(running.dns_name.as_deref(), Some("desktop.tail1234.ts.net"));
    assert_eq!(h.notices(), ["running"]);
    h.manager.stop().await;

    // A node that does not say its name leaves it unknown, and the name does
    // not outlive the waiting.
    let unnamed = Harness::new(&format!(
        r#"read init; {NEEDS_APPROVAL_NAMED}
sleep 0.3
{NEEDS_APPROVAL}
sleep 0.3
{NEEDS_APPROVAL_NAMED}
sleep 0.3
echo '{{"v":1,"type":"state","backendState":"Starting"}}'
while read line; do :; done"#
    ));
    unnamed.manager.start(&unnamed.bin, Mode::Login).unwrap();
    let named = |o: &NetOverview| o.dns_name.as_deref() == Some("yuzora-1.tail1234.ts.net");
    unnamed.until("named", named).await;
    unnamed
        .until("name dropped", |o| {
            o.state == State::NeedsApproval && o.dns_name.is_none()
        })
        .await;
    unnamed.until("named again", named).await;
    unnamed
        .until("not waiting any more", |o| o.state == State::Starting)
        .await;
    assert_eq!(unnamed.manager.overview().dns_name, None);
    unnamed.manager.stop().await;

    // A remembered node whose machine is no longer approved keeps its own name
    // when the node does not say one, and takes the node's when it does.
    let revoked = Harness::new(&format!(
        "read init; {NEEDS_APPROVAL_NAMED}; while read line; do :; done"
    ));
    revoked.registered();
    assert_eq!(
        revoked.manager.overview().dns_name.as_deref(),
        Some("desktop.tail1234.ts.net")
    );
    revoked.manager.start(&revoked.bin, Mode::Enable).unwrap();
    revoked
        .until("pending name", |o| {
            o.state == State::NeedsApproval
                && o.dns_name.as_deref() == Some("yuzora-1.tail1234.ts.net")
        })
        .await;
    assert!(revoked.manager.overview().logged_in);
    assert_eq!(
        revoked.manager.identity().unwrap().dns_name,
        "desktop.tail1234.ts.net"
    );
    revoked.manager.stop().await;
}

/// The node runs, and then stops being usable before its certificate is ready.
async fn cert_wait_after_running(script: &str) -> (String, Duration) {
    // The certificate would be waited for far longer than the test lasts.
    let h = Harness::with_timing(
        &format!(
            r#"read init; echo '{RUNNING}'
echo '{{"v":1,"type":"cert","state":"pending","notAfter":null,"error":null}}'
sleep 0.2
{script}
while read line; do :; done"#
        ),
        Timing {
            running: Duration::from_secs(10),
            cert: Duration::from_secs(20),
            ..FAST
        },
    );
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.manager.wait_running().await.unwrap();
    let started = std::time::Instant::now();
    let outcome = h.manager.wait_cert().await.unwrap_err();
    h.manager.stop().await;
    (outcome, started.elapsed())
}

#[tokio::test]
async fn companion_net_the_certificate_wait_ends_when_the_node_needs_a_login_or_an_approval() {
    let (code, took) = cert_wait_after_running(NEEDS_LOGIN).await;
    assert_eq!(code, "companion-net-login-required");
    assert!(took < Duration::from_secs(5), "{took:?}");
    let (code, took) = cert_wait_after_running(NEEDS_APPROVAL).await;
    assert_eq!(code, "companion-net-approval-required");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[tokio::test]
async fn companion_net_ignores_a_running_event_without_a_name() {
    let h = Harness::new(&format!(
        r#"read init
echo '{{"v":1,"type":"running","dnsName":"","login":"user@example.com","httpsCapable":true}}'
echo '{{"v":1,"type":"peer.rejected","reason":"tagged","node":"phone"}}'
echo '{CERT_READY}'
while read line; do :; done"#
    ));
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.until("cert", |o| o.cert.is_some()).await;
    let seen = h.manager.overview();
    assert_eq!(seen.state, State::Starting);
    assert_eq!(seen.dns_name, None);
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_net_serve_hands_the_secret_to_the_node_only() {
    let h = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'; echo '{CERT_READY}'
while read line; do
  case "$line" in
    *'"serve"'*) printf '%s\n' "$line" > "$0.serve"; sleep 0.1; echo '{{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}}';;
    *'"unserve"'*) echo x >> "$0.unserves"; sleep 0.1; echo '{{"v":1,"type":"unserved"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    assert_eq!(
        h.manager
            .serve("http://127.0.0.1:47110", SECRET, "user@example.com")
            .await
            .unwrap_err(),
        "companion-net-serve-failed",
        "no node, nothing to serve"
    );
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.manager.wait_running().await.unwrap();
    h.manager.wait_cert().await.unwrap();
    // Nothing is served yet: nothing to close.
    h.manager.unserve().await;
    assert_eq!(h.file("unserves"), "");
    h.manager
        .serve("http://127.0.0.1:47110", SECRET, "user@example.com")
        .await
        .unwrap();
    assert!(h.manager.overview().serving);
    let frame: Value = serde_json::from_str(&h.file("serve")).unwrap();
    assert_eq!(
        frame,
        json!({
            "v": 1,
            "type": "serve",
            "target": "http://127.0.0.1:47110",
            "proxySecret": SECRET,
            "requiredLogin": "user@example.com",
        })
    );
    // The call returns once the node has closed the listener.
    h.manager.unserve().await;
    assert!(!h.manager.overview().serving);
    assert_eq!(h.file("unserves").lines().count(), 1);
    h.manager.stop().await;
    let emitted = serde_json::to_string(&*h.events.lock().unwrap()).unwrap();
    assert!(
        !emitted.contains(SECRET),
        "the proxy secret reached an event"
    );
}

#[tokio::test]
async fn companion_net_unserve_says_whether_the_node_stopped_forwarding() {
    let serving = |on_unserve: &str| {
        format!(
            r#"read init; echo '{RUNNING}'; echo '{CERT_READY}'
while read line; do
  case "$line" in
    *'"serve"'*) echo '{{"v":1,"type":"serving","url":"https://desktop.tail1234.ts.net/"}}';;
    *'"unserve"'*) {on_unserve};;
    *'"shutdown"'*) exit 0;;
  esac
done"#
        )
    };
    // Only the bound of `unserve` is under test; the rest may take its time.
    let patient = Timing {
        running: Duration::from_secs(10),
        cert: Duration::from_secs(10),
        serve: Duration::from_secs(10),
        ..FAST
    };
    let start = |h: &Harness| {
        let manager = h.manager.clone();
        let bin = h.bin.clone();
        async move {
            manager.start(&bin, Mode::Enable).unwrap();
            manager.wait_cert().await.unwrap();
            manager
                .serve("http://127.0.0.1:47110", SECRET, "user@example.com")
                .await
                .unwrap();
        }
    };

    // Nothing was served: nothing forwards, and nothing is asked.
    let idle = Harness::with_timing(&serving(r#"echo '{"v":1,"type":"unserved"}'"#), patient);
    assert!(idle.manager.unserve().await);

    // The node says it closed the listener.
    let answers = Harness::with_timing(&serving(r#"echo '{"v":1,"type":"unserved"}'"#), patient);
    start(&answers).await;
    assert!(answers.manager.unserve().await);
    assert!(!answers.manager.overview().serving);
    assert!(
        answers.manager.unserve().await,
        "and again, with nothing left"
    );
    answers.manager.stop().await;

    // It does not say so in time: the caller cannot tell, and must act.
    let silent = Harness::with_timing(&serving(":"), patient);
    start(&silent).await;
    let started = std::time::Instant::now();
    assert!(!silent.manager.unserve().await);
    assert!(started.elapsed() >= FAST.unserve, "{:?}", started.elapsed());
    assert!(silent.manager.overview().serving);
    silent.manager.stop().await;

    // It is gone before it could say so: nothing forwards any more.
    let leaves = Harness::with_timing(&serving("exit 0"), patient);
    start(&leaves).await;
    assert!(leaves.manager.unserve().await);
    // And a node that was served and is gone is not waited for again.
    let started = std::time::Instant::now();
    assert!(leaves.manager.unserve().await);
    assert!(
        started.elapsed() < FAST.unserve / 2,
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn companion_net_serve_fails_when_the_node_refuses_or_stays_silent() {
    let refuses = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'; echo '{CERT_READY}'
while read line; do
  case "$line" in
    *'"serve"'*) echo '{{"v":1,"type":"error","code":"net-listen-failed","message":"in use"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    refuses.manager.start(&refuses.bin, Mode::Enable).unwrap();
    refuses.manager.wait_cert().await.unwrap();
    assert_eq!(
        refuses
            .manager
            .serve("http://127.0.0.1:47110", SECRET, "user@example.com")
            .await
            .unwrap_err(),
        "companion-net-serve-failed"
    );
    assert!(!refuses.manager.overview().serving);
    refuses.manager.stop().await;

    let silent = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'; echo '{CERT_READY}'; while read line; do :; done"#
    ));
    silent.manager.start(&silent.bin, Mode::Enable).unwrap();
    silent.manager.wait_cert().await.unwrap();
    assert_eq!(
        silent
            .manager
            .serve("http://127.0.0.1:47110", SECRET, "user@example.com")
            .await
            .unwrap_err(),
        "companion-net-serve-failed"
    );
    silent.manager.stop().await;
}

#[tokio::test]
async fn companion_net_exit_without_a_request_is_a_failure() {
    let h = Harness::new(&format!(r#"read init; echo '{RUNNING}'; exit 1"#));
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.until("failed", |o| o.state == State::Failed).await;
    assert_eq!(
        h.manager.overview().error.as_deref(),
        Some("companion-net-crashed")
    );
    assert_eq!(h.manager.mode(), None);
    assert_eq!(h.notices(), ["running", "exited:false"]);
    // A node that left a reason says it.
    let fatal = Harness::new(
        r#"read init; echo '{"v":1,"type":"error","code":"net-version-mismatch","message":"x"}'; exit 2"#,
    );
    fatal.manager.start(&fatal.bin, Mode::Enable).unwrap();
    fatal.until("failed", |o| o.state == State::Failed).await;
    assert_eq!(
        fatal.manager.overview().error.as_deref(),
        Some("companion-net-start-failed")
    );
    // No restart by itself, but the next start works.
    assert!(h.manager.start(&h.bin, Mode::Enable).is_ok());
    h.manager.stop().await;
}

#[tokio::test]
async fn companion_net_stop_kills_a_node_that_ignores_shutdown() {
    let h = Harness::new(&format!(
        r#"trap '' TERM; read init; echo '{RUNNING}'; while :; do sleep 1; done"#
    ));
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    h.until("running", |o| o.state == State::Running).await;
    let started = std::time::Instant::now();
    h.manager.stop().await;
    assert_eq!(h.manager.overview().state, State::Stopped);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(h.notices(), ["running", "exited:true"]);
}

#[cfg(unix)]
#[tokio::test]
async fn companion_net_sidecar_spawns_through_the_spawn_lock() {
    let h = Harness::new(
        r#"read init
while read line; do
  case "$line" in *'"shutdown"'*) exit 0;; esac
done"#,
    );
    let (started, spawns) =
        crate::process_kill::trace_spawns(|| h.manager.start(&h.bin, Mode::Enable));
    started.unwrap();
    assert_eq!(spawns, [h.bin.display().to_string()]);
    h.manager.stop().await;
    assert_eq!(h.manager.overview().state, State::Stopped);
}

#[tokio::test]
async fn companion_net_missing_binary_fails_to_start() {
    let h = Harness::new("exit 0");
    assert!(h
        .manager
        .start(Path::new("/nonexistent/yuzora-companion-net"), Mode::Enable)
        .unwrap_err()
        .starts_with("companion-net-start-failed"));
    assert_eq!(h.manager.overview().state, State::Stopped);
    assert_eq!(h.manager.mode(), None);
}

#[tokio::test]
async fn companion_net_logout_deletes_the_state_once_the_node_logged_out() {
    let h = Harness::new(&format!(
        r#"read init; echo started > "$0.started"; echo '{RUNNING}'
while read line; do
  case "$line" in
    *'"logout"'*) echo logout > "$0.logout"; echo '{{"v":1,"type":"logged-out"}}'; exit 0;;
  esac
done"#
    ));
    h.registered();
    std::fs::write(h.dir.join("tsnet.log"), "x").unwrap();
    h.manager.logout(h.program()).await.unwrap();
    assert_eq!(h.file("logout").trim(), "logout");
    assert!(!h.dir.exists());
    // The UI is told what the sidecar runs for.
    assert!(
        h.events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["net"]["mode"] == "logout"),
        "no event said logout"
    );
    let after = h.manager.overview();
    assert_eq!(after.state, State::Stopped);
    assert_eq!(
        (after.logged_in, after.dns_name, after.login),
        (false, None, None)
    );
    // It ran to be logged out, and said so.
    assert_eq!(h.notices(), ["running", "exited:true"]);

    // A node that never logged in has nothing to log out: only leftovers go.
    let never =
        Harness::new(r#"read init; echo started > "$0.started"; while read line; do :; done"#);
    std::fs::create_dir_all(&never.dir).unwrap();
    std::fs::write(never.dir.join("tsnet.log"), "x").unwrap();
    never.manager.logout(never.program()).await.unwrap();
    assert!(!never.dir.exists());
    assert_eq!(never.file("started"), "", "no sidecar was started");
}

#[tokio::test]
async fn companion_net_logout_that_fails_keeps_the_state() {
    let fails = Harness::new(&format!(
        r#"read init; echo '{RUNNING}'
while read line; do
  case "$line" in
    *'"logout"'*) echo '{{"v":1,"type":"error","code":"net-logout-failed","message":"no"}}';;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    fails.registered();
    assert_eq!(
        fails.manager.logout(fails.program()).await.unwrap_err(),
        "companion-net-logout-failed"
    );
    assert!(fails.dir.join(STATE_FILE).exists());
    assert_eq!(fails.manager.mode(), None, "the node is stopped again");

    // A node that never gets online cannot be asked.
    let offline = Harness::new("read init; while read line; do :; done");
    offline.registered();
    assert_eq!(
        offline.manager.logout(offline.program()).await.unwrap_err(),
        "companion-net-logout-failed"
    );
    assert!(offline.dir.join(STATE_FILE).exists());
    assert_eq!(offline.manager.mode(), None);
}

/// tsnet writes its state file as soon as it starts, before any login.
const WRITES_STATE_FILE: &str = r#"echo '{}' > "$(dirname "$0")/net/tailscaled.state""#;
const NEEDS_LOGIN: &str = r#"echo '{"v":1,"type":"state","backendState":"NeedsLogin"}'
echo '{"v":1,"type":"login-required","authUrl":"https://login.tailscale.com/a/abc"}'"#;

#[tokio::test]
async fn companion_net_a_cancelled_first_login_is_not_logged_in() {
    let h = Harness::new(&format!(
        r#"read init; echo started >> "$0.starts"
{WRITES_STATE_FILE}
{NEEDS_LOGIN}
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    h.manager.start(&h.bin, Mode::Login).unwrap();
    h.until("login url", |o| o.auth_url.is_some()).await;
    // tsnet has its state file by now, and the node is not registered.
    assert!(h.dir.join(STATE_FILE).exists());
    let waiting = h.manager.overview();
    assert_eq!(
        (waiting.mode, waiting.logged_in),
        (Some(Mode::Login), false)
    );
    assert_eq!(serde_json::to_value(&waiting).unwrap()["mode"], "login");

    // Cancel: the card must offer to sign in again, and sign out must not fail.
    h.manager.stop().await;
    let cancelled = h.manager.overview();
    assert_eq!(
        (cancelled.state, cancelled.mode, cancelled.logged_in),
        (State::Stopped, None, false)
    );
    assert_eq!(
        serde_json::to_value(&cancelled).unwrap()["mode"],
        Value::Null
    );
    assert!(
        h.dir.join(STATE_FILE).exists(),
        "the leftover is still there"
    );
    h.manager.logout(h.program()).await.unwrap();
    assert!(!h.dir.exists());
    assert_eq!(
        h.file("starts").lines().count(),
        1,
        "no sidecar to sign out"
    );
    assert_eq!(h.notices(), ["exited:true"], "it never ran");
}

#[tokio::test]
async fn companion_net_a_node_whose_key_expired_is_not_logged_in_any_more() {
    let h = Harness::new(&format!(
        r#"read init; echo started >> "$0.starts"
{NEEDS_LOGIN}
while read line; do
  case "$line" in
    *'"logout"'*) echo logout > "$0.logout";;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    h.registered();
    let before = h.manager.overview();
    assert!(before.logged_in);
    assert_eq!(before.login.as_deref(), Some("user@example.com"));

    // It was running before; now it asks for a login.
    h.manager.start(&h.bin, Mode::Enable).unwrap();
    assert_eq!(
        h.manager.wait_running().await.err().unwrap(),
        "companion-net-login-required"
    );
    let expired = h.manager.overview();
    assert_eq!(
        (expired.logged_in, expired.dns_name, expired.login),
        (false, None, None)
    );
    assert!(h.manager.identity().is_none());
    // Settings are told once, although the node says it twice.
    for _ in 0..500 {
        if !h.notices().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.manager.stop().await;
    assert_eq!(h.notices(), ["login-required", "exited:true"]);
    assert!(!h.manager.overview().logged_in);
}

#[tokio::test]
async fn companion_net_sign_out_of_a_node_that_asks_for_a_login_deletes_locally() {
    let h = Harness::new(&format!(
        r#"read init; echo started >> "$0.starts"
{NEEDS_LOGIN}
while read line; do
  case "$line" in
    *'"logout"'*) echo logout > "$0.logout";;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    // Seen running once, and its key has expired since.
    h.registered();
    h.manager.logout(h.program()).await.unwrap();
    assert!(!h.dir.exists());
    assert_eq!(h.file("logout"), "", "there was no login to end");
    assert_eq!(h.manager.mode(), None);
    assert_eq!(h.manager.overview().state, State::Stopped);
}

#[tokio::test]
async fn companion_net_sign_out_of_a_machine_that_waits_for_approval_deletes_locally() {
    // A remembered node whose machine is no longer approved: it cannot log out,
    // and is not waited for. No logout is sent.
    let revoked = Harness::new(&format!(
        r#"read init; echo started >> "$0.starts"
{NEEDS_APPROVAL}
while read line; do
  case "$line" in
    *'"logout"'*) echo logout > "$0.logout";;
    *'"shutdown"'*) exit 0;;
  esac
done"#
    ));
    revoked.registered();
    let started = std::time::Instant::now();
    revoked.manager.logout(revoked.program()).await.unwrap();
    assert!(
        started.elapsed() < FAST.running / 2,
        "waited for a node that never runs: {:?}",
        started.elapsed()
    );
    assert!(!revoked.dir.exists());
    assert_eq!(revoked.file("logout"), "", "there was no login to end");
    assert_eq!(revoked.file("starts").lines().count(), 1);
    assert_eq!(revoked.manager.mode(), None);
    let after = revoked.manager.overview();
    assert_eq!(
        (after.state, after.logged_in, after.dns_name, after.login),
        (State::Stopped, false, None, None)
    );

    // A first registration still waiting for approval: its login node is
    // stopped first, and the sidecar to ask with is not even looked for.
    let first = Harness::new(&format!(
        r#"read init; {WRITES_STATE_FILE}
{NEEDS_APPROVAL_NAMED}
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    first.manager.start(&first.bin, Mode::Login).unwrap();
    first
        .until("approval", |o| o.state == State::NeedsApproval)
        .await;
    assert!(first.dir.join(STATE_FILE).exists());
    first
        .manager
        .logout(|| Err("the sidecar must not be looked for".into()))
        .await
        .unwrap();
    assert!(!first.dir.exists());
    let after = first.manager.overview();
    assert_eq!(
        (after.state, after.mode, after.logged_in, after.dns_name),
        (State::Stopped, None, false, None)
    );
}

/// What the test process's log sink wrote for lines holding `tag`.
fn logged(tag: &str) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!("yuzora-test-logs-{}", std::process::id()));
    let mut lines = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        lines.extend(text.lines().filter(|l| l.contains(tag)).map(str::to_owned));
    }
    lines
}

#[tokio::test]
async fn companion_net_stderr_is_logged_without_proxy_credentials() {
    let tag = format!("tag{:016x}", rand::random::<u64>());
    let password = format!("pw{:016x}", rand::random::<u64>());
    let h = Harness::new(&format!(
        r#"read init
echo 'tshttpproxy: {tag} using proxy "http://alice:{password}@127.0.0.1:3128" for URL: "https://controlplane.tailscale.com/"' >&2
while read line; do :; done"#
    ));
    h.manager.start(&h.bin, Mode::Login).unwrap();
    let mut lines = Vec::new();
    for _ in 0..300 {
        lines = logged(&tag);
        if !lines.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.manager.stop().await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("companion_net_stderr"), "{}", lines[0]);
    assert!(
        lines[0].contains("<redacted>@127.0.0.1:3128"),
        "{}",
        lines[0]
    );
    assert!(
        logged(&password).is_empty(),
        "a proxy password reached the log"
    );
}

/// A sidecar's stderr, `window` apart in what the log kept.
struct Relayed {
    writer: tokio::io::DuplexStream,
    logged: Arc<Mutex<Vec<String>>>,
    done: tokio::task::JoinHandle<()>,
}
impl Relayed {
    fn new(limit: usize, window: Duration) -> Self {
        let (writer, reader) = tokio::io::duplex(256 * 1024);
        let logged = Arc::new(Mutex::new(Vec::new()));
        let sink = logged.clone();
        let done = tokio::spawn(relay_stderr(reader, limit, window, move |message| {
            sink.lock().unwrap().push(message.to_owned())
        }));
        Self {
            writer,
            logged,
            done,
        }
    }
    async fn say(&mut self, lines: impl IntoIterator<Item = String>) {
        use tokio::io::AsyncWriteExt;
        for line in lines {
            self.writer
                .write_all(format!("{line}\n").as_bytes())
                .await
                .unwrap();
        }
    }
    fn logged(&self) -> Vec<String> {
        self.logged.lock().unwrap().clone()
    }
    async fn until(&self, count: usize) {
        for _ in 0..500 {
            if self.logged().len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {count} lines: {:?}", self.logged());
    }
    /// The sidecar ends: the pipe closes.
    async fn end(self) -> Vec<String> {
        drop(self.writer);
        self.done.await.unwrap();
        let logged = self.logged.lock().unwrap().clone();
        logged
    }
}

#[tokio::test]
async fn companion_net_stderr_survives_an_oversized_record_and_invalid_utf8() {
    let mut relay = Relayed::new(10, Duration::from_secs(60));
    use tokio::io::AsyncWriteExt;
    relay
        .writer
        .write_all(&vec![b'y'; STDERR_RECORD_BYTES * 3])
        .await
        .unwrap();
    relay
        .writer
        .write_all(b"\nbad \xff\nafter\n")
        .await
        .unwrap();
    relay.until(3).await;
    let logged = relay.end().await;
    assert_eq!(logged[0], "y".repeat(500));
    assert_eq!(logged[1], "bad \u{fffd}");
    assert_eq!(logged[2], "after");
}

#[tokio::test]
async fn companion_net_stderr_passes_a_limited_number_of_lines_per_window_and_counts_the_rest() {
    let window = Duration::from_millis(400);
    let mut relay = Relayed::new(3, window);
    relay.say((0..10).map(|n| format!("line {n}"))).await;
    // The window's lines go through at once; the rest wait for its end.
    relay.until(3).await;
    assert_eq!(relay.logged(), ["line 0", "line 1", "line 2"]);
    // The summary comes when the window ends, without another line to prompt it.
    relay.until(4).await;
    assert_eq!(
        relay.logged(),
        ["line 0", "line 1", "line 2", "… 7 similar lines suppressed"]
    );
    // The next window passes lines again, and its own count.
    relay.say((10..12).map(|n| format!("line {n}"))).await;
    relay.until(6).await;
    assert_eq!(relay.logged()[4..], ["line 10", "line 11"]);
    // A window that dropped nothing says nothing more.
    tokio::time::sleep(window + Duration::from_millis(200)).await;
    assert_eq!(relay.end().await.len(), 6);
}

#[tokio::test]
async fn companion_net_stderr_reports_what_it_dropped_when_the_sidecar_ends() {
    let mut relay = Relayed::new(2, Duration::from_secs(60));
    relay.say((0..5).map(|n| format!("line {n}"))).await;
    assert_eq!(
        relay.end().await,
        ["line 0", "line 1", "… 3 similar lines suppressed"]
    );
    // Within the limit, nothing is added.
    let mut quiet = Relayed::new(2, Duration::from_secs(60));
    quiet.say((0..2).map(|n| format!("line {n}"))).await;
    assert_eq!(quiet.end().await, ["line 0", "line 1"]);
}

#[tokio::test]
async fn companion_net_stderr_masks_credentials_before_it_cuts_the_line() {
    let mut relay = Relayed::new(5, Duration::from_secs(60));
    // The user name and password start at the 481st character: a cut at 500
    // characters of the raw line lands inside them and leaves a piece unmasked.
    let line = format!(
        "{}http://alice:{}@proxy:3128/",
        "x".repeat(473),
        "p".repeat(30)
    );
    relay.say([line, "y".repeat(2000)]).await;
    let logged = relay.end().await;
    assert!(
        !logged[0].contains("alice") && !logged[0].contains("pppp"),
        "{}",
        logged[0]
    );
    assert!(logged[0].contains("<redacted>@proxy"), "{}", logged[0]);
    assert!(logged[0].chars().count() <= 500);
    assert_eq!(logged[1].chars().count(), 500);
}

#[tokio::test]
async fn companion_net_a_sidecar_that_floods_stderr_cannot_flood_the_log() {
    let tag = format!("flood{:016x}", rand::random::<u64>());
    let h = Harness::new(&format!(
        r#"read init
i=0
while [ $i -lt 437 ]; do echo "{tag} line $i" >&2; i=$((i+1)); done
while read line; do case "$line" in *'"shutdown"'*) exit 0;; esac; done"#
    ));
    h.manager.start(&h.bin, Mode::Login).unwrap();
    // 60 lines per minute are logged ...
    for _ in 0..500 {
        if logged(&tag).len() >= 60 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // ... and one line says what the rest was, at the latest when the sidecar ends.
    h.manager.stop().await;
    for _ in 0..500 {
        if !logged("377 similar lines suppressed").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let lines = logged(&tag);
    assert_eq!(lines.len(), 60, "{} lines", lines.len());
    assert!(lines[0].contains("line 0") && lines[59].contains("line 59"));
    assert_eq!(logged("377 similar lines suppressed").len(), 1);
    assert!(logged("377 similar lines suppressed")[0].contains("companion_net_stderr"));
}

#[test]
fn companion_net_env_adds_only_the_proxy_variables_to_the_gateway_allowlist() {
    let env = sidecar_env(|_| Some("x".into()));
    let keys: BTreeSet<_> = env.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys.len(), ENV_ALLOWLIST.len() + PROXY_ENV.len());
    for key in ENV_ALLOWLIST {
        assert!(keys.contains(key), "{key}");
    }
    for key in PROXY_ENV {
        assert!(keys.contains(key), "{key}");
        assert!(
            !ENV_ALLOWLIST.contains(key),
            "{key} would reach the gateway"
        );
    }
    for key in ["HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "ALL_PROXY"] {
        assert!(PROXY_ENV.contains(&key) && PROXY_ENV.contains(&key.to_lowercase().as_str()));
    }
    // The gateway runs git for a phone, the node does not: its extra
    // variables are set at the gateway's spawn, and are not inherited.
    for (key, _) in GATEWAY_GIT_ENV {
        assert!(!keys.contains(key), "{key} would reach the node");
        assert!(!ENV_ALLOWLIST.contains(key), "{key} would be inherited");
    }
    // What the desktop does not have, the node does not get.
    assert!(sidecar_env(|_| None).is_empty());
}

#[test]
fn companion_net_errors_map_to_the_codes_the_ui_knows() {
    for (code, mapped) in [
        ("net-not-ready", "companion-net-serve-failed"),
        ("net-serve-invalid", "companion-net-serve-failed"),
        ("net-listen-failed", "companion-net-serve-failed"),
        ("net-logout-failed", "companion-net-logout-failed"),
        ("net-start-failed", "companion-net-start-failed"),
        ("net-init-invalid", "companion-net-start-failed"),
        ("net-version-mismatch", "companion-net-start-failed"),
        ("something-else", "companion-net-start-failed"),
    ] {
        assert_eq!(error_code(code), mapped, "{code}");
    }
}
