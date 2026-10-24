use super::*;
use crate::{
    admin::{self, CommandBody, DeviceCommand, PushDevice, PushInit},
    clock::Clock,
    devices::Device,
    herdr::{HerdrPort, Result as HerdrResult},
    test_support::Fixture,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::{AbortHandle, JoinHandle};
use yuzora_host::herdr_service::{HerdrNamedSession, HerdrSnapshotResult, OnTerminalEvent};

const D1: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const D2: &str = "AAAAAAAAAAAAAAAAAAAAAQ";
const D3: &str = "AAAAAAAAAAAAAAAAAAAAAg";
const FAST: Pace = Pace {
    settle: Duration::from_millis(5),
    first: Duration::from_millis(5),
    longest: Duration::from_millis(40),
};

fn point(first: u8, len: usize) -> String {
    let mut bytes = vec![7u8; len];
    bytes[0] = first;
    URL_SAFE_NO_PAD.encode(bytes)
}
fn vapid() -> String {
    point(0x04, 65)
}
fn device(id: &str, credential: &str, locked: bool) -> Device {
    let mut value: Value =
        serde_json::from_str(include_str!("../tests/fixtures/device.json")).unwrap();
    value["deviceId"] = json!(id);
    value["credentials"][0]["id"] = json!(credential);
    value["locked"] = json!(locked);
    serde_json::from_value(value).unwrap()
}

#[test]
fn push_endpoint_allowlist_table() {
    let long = |len: usize| format!("https://fcm.googleapis.com/{}", "a".repeat(len - 27));
    for accepted in [
        "https://web.push.apple.com/QGkYcR",
        "https://push.apple.com/x",
        "https://api.sandbox.push.apple.com/3/device/abc",
        "https://fcm.googleapis.com/fcm/send/abc:def",
        "https://updates.push.services.mozilla.com/wpush/v2/gAAAA",
        "https://wns2-par02p.notify.windows.com/w/?token=BQYAAAB",
        "https://notify.windows.com/w",
        "https://fcm.googleapis.com",
        "https://fcm.googleapis.com?x=1",
        &long(2048),
    ] {
        assert!(
            endpoint_ok(accepted),
            "{}",
            &accepted[..accepted.len().min(80)]
        );
    }
    for rejected in [
        "http://fcm.googleapis.com/x",
        "HTTPS://fcm.googleapis.com/x",
        "fcm.googleapis.com/x",
        "//fcm.googleapis.com/x",
        "ftp://fcm.googleapis.com/x",
        "wss://fcm.googleapis.com/x",
        "",
        "https://",
        "https:///x",
        // userinfo, in every position
        "https://user@fcm.googleapis.com/x",
        "https://user:pw@fcm.googleapis.com/x",
        "https://fcm.googleapis.com@evil.com/x",
        "https://fcm.googleapis.com:443@evil.com/x",
        "https://evil.com@fcm.googleapis.com/x",
        // fragment
        "https://fcm.googleapis.com/x#frag",
        "https://fcm.googleapis.com#frag",
        // look-alike hosts
        "https://evilpush.apple.com/x",
        "https://evilfcm.googleapis.com/x",
        "https://xnotify.windows.com/x",
        "https://evilpush.apple.com.attacker.tld/x",
        "https://push.apple.com.evil/x",
        "https://push.apple.com.attacker.tld/x",
        "https://fcm.googleapis.com.evil.com/x",
        "https://notify.windows.com.evil/x",
        "https://updates.push.services.mozilla.com.evil.com/x",
        "https://evil.com/fcm.googleapis.com",
        "https://evil.com?.push.apple.com/x",
        "https://evil.com\\.fcm.googleapis.com/x",
        // a parent of a push service is not a push service
        "https://apple.com/x",
        "https://googleapis.com/x",
        "https://services.mozilla.com/x",
        "https://push.services.mozilla.com/x",
        "https://windows.com/x",
        // ports, trailing and doubled dots, case, address literals
        "https://fcm.googleapis.com:443/x",
        "https://fcm.googleapis.com:8443/x",
        "https://fcm.googleapis.com./x",
        "https://.fcm.googleapis.com/x",
        "https://a..fcm.googleapis.com/x",
        "https://FCM.googleapis.com/x",
        "https://[::1]/x",
        "https://127.0.0.1/x",
        "https://localhost/x",
        // characters that are not visible ASCII
        "https://fcm.googleapis.com/x y",
        "https://fcm.googleapis.com/x\ty",
        "https://fcm.googleapis.com/x\ny",
        "https://fcm.googleapis.com/\0",
        "https://fcm.googleapis.com/\u{e9}",
        "https://fcm.googleapis\u{ff0e}com/x",
        &long(2049),
    ] {
        assert!(
            !endpoint_ok(rejected),
            "{:?}",
            &rejected[..rejected.len().min(80)]
        );
    }
}

#[test]
fn push_subscription_params_check_keys_and_shape() {
    let endpoint = "https://fcm.googleapis.com/fcm/send/abc";
    let p256dh = point(0x04, 65);
    let auth = URL_SAFE_NO_PAD.encode([9u8; 16]);
    let params = |endpoint: &str, p256dh: &str, auth: &str| json!({"endpoint":endpoint,"keys":{"p256dh":p256dh,"auth":auth}});
    let ok = subscription(&params(endpoint, &p256dh, &auth)).unwrap();
    assert_eq!(
        (ok.endpoint.as_str(), ok.p256dh.as_str(), ok.auth.as_str()),
        (endpoint, p256dh.as_str(), auth.as_str())
    );

    let swap = |value: &str, from: char, to: &str| {
        let changed = value.replace(from, to);
        assert_ne!(changed, value);
        changed
    };
    let mut non_canonical = p256dh.clone();
    non_canonical.replace_range(86.., "B");
    assert_ne!(non_canonical, p256dh);
    for bad_p256dh in [
        String::new(),
        point(0x04, 64),
        point(0x04, 66),
        point(0x02, 65),
        point(0x03, 65),
        point(0x00, 65),
        format!("{p256dh}="),
        swap(&p256dh, 'A', "+"),
        swap(&p256dh, 'A', "/"),
        swap(&p256dh, 'A', " "),
        non_canonical,
        auth.clone(),
    ] {
        assert_eq!(
            subscription(&params(endpoint, &bad_p256dh, &auth)).err(),
            Some("invalid-params"),
            "{bad_p256dh:?}"
        );
    }
    for bad_auth in [
        String::new(),
        URL_SAFE_NO_PAD.encode([9u8; 15]),
        URL_SAFE_NO_PAD.encode([9u8; 17]),
        format!("{auth}=="),
        swap(&auth, 'Q', "+"),
        p256dh.clone(),
    ] {
        assert_eq!(
            subscription(&params(endpoint, &p256dh, &bad_auth)).err(),
            Some("invalid-params"),
            "{bad_auth:?}"
        );
    }
    assert_eq!(
        subscription(&params("http://fcm.googleapis.com/x", &p256dh, &auth)).err(),
        Some("invalid-params")
    );
    for malformed in [
        Value::Null,
        json!([]),
        json!("endpoint"),
        json!({}),
        json!({"endpoint":endpoint}),
        json!({"keys":{"p256dh":p256dh,"auth":auth}}),
        json!({"endpoint":7,"keys":{"p256dh":p256dh,"auth":auth}}),
        json!({"endpoint":endpoint,"keys":{"p256dh":p256dh}}),
        json!({"endpoint":endpoint,"keys":{"p256dh":p256dh,"auth":auth,"x":1}}),
        json!({"endpoint":endpoint,"keys":{"p256dh":p256dh,"auth":auth},"expirationTime":null}),
        json!({"endpoint":endpoint,"keys":[p256dh, auth]}),
    ] {
        assert_eq!(subscription(&malformed).err(), Some("invalid-params"));
    }
}

#[test]
fn push_backoff_doubles_to_the_longest_and_a_long_healthy_streak_resets_it() {
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    let mut backoff = Backoff::new(Pace::DEFAULT);
    // Never up: keep growing, however late the failure.
    let waits: Vec<_> = (0..6)
        .map(|n| backoff.after(at(n * 1000)).as_secs())
        .collect();
    assert_eq!(waits, [5, 10, 20, 40, 60, 60]);
    // Up for less than the longest wait: keep growing.
    backoff.up(at(10_000));
    assert_eq!(backoff.after(at(10_059)).as_secs(), 60);
    // Up for the longest wait or more: start again, and the failure ends the streak.
    backoff.up(at(11_000));
    assert_eq!(backoff.after(at(11_060)).as_secs(), 5);
    assert_eq!(backoff.after(at(11_061)).as_secs(), 10);
    // Cycles cut by topology changes are one streak: it counts from the first.
    for began in [20_000, 20_030, 20_050] {
        backoff.up(at(began));
    }
    assert_eq!(backoff.after(at(20_070)).as_secs(), 5);
}

fn pane(id: &str, status: &str) -> Value {
    json!({"id":id,"workspaceId":"w","status":status,"label":null})
}
fn snapshot_of(panes: Vec<Value>) -> Value {
    json!({"session":"work","workspaces":[{"id":"w","label":"Space One","status":"unknown"}],
        "tabs":[],"panes":panes,"agents":[]})
}
fn kinds(notices: Vec<Notice>) -> Vec<(String, &'static str)> {
    notices.into_iter().map(|n| (n.pane, n.kind)).collect()
}

#[test]
fn push_transitions_into_blocked_or_done_only() {
    let mut seen = Seen::default();
    // Starting point: a pane that is already blocked announces nothing.
    assert!(seen
        .snapshot(&snapshot_of(vec![
            pane("w:a", "blocked"),
            pane("w:b", "done"),
            pane("w:c", "working")
        ]))
        .is_empty());
    assert!(seen.status("w:a", "blocked").is_none());
    assert!(seen.status("w:b", "done").is_none());
    // Into blocked / done, once each.
    assert_eq!(seen.status("w:c", "blocked").unwrap().kind, "blocked");
    assert!(seen.status("w:c", "blocked").is_none());
    assert_eq!(seen.status("w:c", "done").unwrap().kind, "done");
    assert!(seen.status("w:c", "done").is_none());
    // Out of them, or into anything else, nothing.
    for quiet in ["idle", "working", "unknown"] {
        assert!(seen.status("w:c", quiet).is_none(), "{quiet}");
    }
    assert_eq!(seen.status("w:c", "blocked").unwrap().kind, "blocked");
    // unknown (an unrecognised Herdr status) is not blocked.
    assert_eq!(status_name("surprising"), "unknown");
    assert_eq!(status_name("blocked"), "blocked");
    // An event for a pane no snapshot knows is ignored.
    assert!(seen.status("w:zz", "blocked").is_none());

    // Later snapshots: a changed known pane is a transition, a new pane is a
    // starting point, a vanished pane is forgotten.
    let next = snapshot_of(vec![
        pane("w:a", "working"),
        pane("w:b", "idle"),
        pane("w:c", "blocked"),
        pane("w:new", "blocked"),
        pane("w:x", "done"),
    ]);
    assert!(seen.snapshot(&next).is_empty());
    let next = snapshot_of(vec![
        pane("w:a", "done"),
        pane("w:c", "blocked"),
        pane("w:new", "working"),
        pane("w:x", "blocked"),
    ]);
    assert_eq!(
        {
            let mut found = kinds(seen.snapshot(&next));
            found.sort();
            found
        },
        [("w:a".to_string(), "done"), ("w:x".to_string(), "blocked")]
    );
    assert!(seen.status("w:b", "blocked").is_none());
    seen.panes.remove("w:a");
    assert!(seen.status("w:a", "blocked").is_none());
}

#[test]
fn push_notice_names_the_agent_then_kind_then_label_then_id() {
    let snapshot = json!({"session":"work",
        "workspaces":[{"id":"w","label":"  Space One  ","status":"unknown"},{"id":"v","label":"","status":"unknown"}],
        "panes":[
            {"id":"w:named","workspaceId":"w","status":"blocked","label":"zsh"},
            {"id":"w:kind","workspaceId":"w","status":"blocked","label":"zsh"},
            {"id":"w:label","workspaceId":"v","status":"blocked","label":"zsh"},
            {"id":"w:bare","workspaceId":null,"status":"blocked","label":null},
            {"id":"w:blank","workspaceId":"gone","status":"blocked","label":"   "},
            {"id":"w:long","workspaceId":"w","status":"blocked","label":"x"}],
        "agents":[
            {"paneId":"w:named","name":"Reviewer","agent":"claude","status":"blocked"},
            {"paneId":"w:kind","name":null,"agent":"claude","status":"blocked"},
            {"paneId":"w:blank","name":" ","agent":"","status":"blocked"},
            {"paneId":"w:long","name":"é".repeat(300),"agent":"claude","status":"blocked"}]});
    let mut seen = Seen::default();
    seen.snapshot(&snapshot);
    let titles: HashMap<_, _> = seen
        .panes
        .iter()
        .map(|(id, p)| (id.as_str(), (p.title.clone(), p.space.clone())))
        .collect();
    let space = Some("Space One".to_owned());
    assert_eq!(titles["w:named"], ("Reviewer".into(), space.clone()));
    assert_eq!(titles["w:kind"], ("claude".into(), space.clone()));
    assert_eq!(titles["w:label"], ("zsh".into(), None));
    assert_eq!(titles["w:bare"], ("w:bare".into(), None));
    assert_eq!(titles["w:blank"], ("w:blank".into(), None));
    assert_eq!(titles["w:long"].0.chars().count(), LABEL_CHARS);
    let payload = seen.panes["w:named"]
        .notice("w:named")
        .unwrap()
        .payload("work");
    assert_eq!(
        serde_json::to_value(Event::push_request(vec![D1.into()], payload)).unwrap(),
        json!({"v":1,"type":"push.request","deviceIds":[D1],"payload":{
            "kind":"blocked","session":"work","paneId":"w:named","workspaceId":"w",
            "title":"Reviewer","space":"Space One"}})
    );
    let bare = seen.panes["w:bare"]
        .notice("w:bare")
        .unwrap()
        .payload("work");
    assert_eq!(
        serde_json::to_value(&bare).unwrap(),
        json!({"kind":"blocked","session":"work","paneId":"w:bare","workspaceId":null,
            "title":"w:bare","space":null})
    );
}

#[test]
fn push_endpoint_id_is_the_unpadded_base64url_of_the_sha256_of_the_url() {
    // The desktop computes the same: these vectors are in its tests too.
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
}

#[test]
fn push_subscription_event_shapes_and_no_reserved_names() {
    let subscription = Subscription {
        endpoint: "https://fcm.googleapis.com/x".into(),
        p256dh: vapid(),
        auth: "a".into(),
    };
    assert_eq!(
        serde_json::to_value(Event::push_subscription(D1.into(), subscription, 7)).unwrap(),
        json!({"v":1,"type":"push.subscription","deviceId":D1,"subscription":{
            "endpoint":"https://fcm.googleapis.com/x","p256dh":vapid(),"auth":"a"},"requestId":7})
    );
    // A removal names the one endpoint, by id, and carries no URL.
    assert_eq!(
        serde_json::to_value(Event::push_unsubscription(D1.into(), eid("a"))).unwrap(),
        json!({"v":1,"type":"push.subscription","deviceId":D1,"subscription":null,
            "endpointId":eid("a")})
    );
    assert!(admin::RESERVED_EVENTS.is_empty());
}

#[tokio::test]
async fn push_events_are_ordinary_class_and_cannot_starve_lifecycle_events() {
    // Nothing reads the other end, so queued events keep their slots.
    let (output, _input) = tokio::io::duplex(1);
    let (events, _writer) = admin::spawn_writer(output);
    let accepted = (0..100)
        .filter(|_| events.try_emit(Event::push_unsubscription(D1.into(), eid("a"))))
        .count();
    assert_eq!(accepted, 48);
    assert!(events.try_emit(Event::error("lifecycle")));
}

fn init_with(push: Option<PushInit>, devices: Vec<Device>, sessions: &[&str]) -> admin::Init {
    let mut init = Fixture::init();
    init.session_allowlist = sessions.iter().map(|s| s.to_string()).collect();
    init.devices = devices;
    init.push = push;
    init
}
/// A valid endpoint id; `name` stands for an endpoint URL.
fn eid(name: &str) -> String {
    endpoint_id(&format!("https://fcm.googleapis.com/fcm/send/{name}"))
}
/// A device holding one endpoint of its own.
fn holding(device: &str) -> PushDevice {
    with(device, &[device])
}
fn with(device: &str, endpoints: &[&str]) -> PushDevice {
    PushDevice {
        device_id: device.into(),
        endpoints: endpoints.iter().map(|name| eid(name)).collect(),
    }
}
fn push_init(devices: &[&str]) -> Option<PushInit> {
    Some(PushInit {
        vapid_public_key: vapid(),
        devices: devices.iter().map(|id| holding(id)).collect(),
    })
}
fn stored_set(gateway: &Gateway) -> BTreeMap<String, Vec<String>> {
    gateway
        .push
        .devices
        .lock()
        .unwrap()
        .iter()
        .map(|(id, endpoints)| (id.clone(), endpoints.iter().cloned().collect()))
        .collect()
}
fn gateway(init: &admin::Init) -> Result<Arc<Gateway>, &'static str> {
    let (events, _writer) = admin::spawn_writer(tokio::io::sink());
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    Gateway::new(init, events, clock)
}

/// Lists that break a limit: init refuses them, `push.devices` keeps the old set.
fn violations() -> Vec<(&'static str, Vec<PushDevice>)> {
    let device = |endpoints: Vec<String>| {
        vec![PushDevice {
            device_id: D1.into(),
            endpoints,
        }]
    };
    let mut non_canonical = eid("a");
    non_canonical.replace_range(42.., "B");
    assert_ne!(non_canonical, eid("a"));
    assert!(eid("abc").contains('_'));
    vec![
        (
            "65 devices",
            (0..65).map(|n| holding(&format!("id-{n}"))).collect(),
        ),
        ("no endpoint", device(Vec::new())),
        ("5 endpoints", vec![with(D1, &["a", "b", "c", "d", "e"])]),
        ("a device twice", vec![holding(D1), holding(D1)]),
        ("empty id", device(vec![String::new()])),
        ("short id", device(vec![eid("a")[..42].to_owned()])),
        ("long id", device(vec![format!("{}A", eid("a"))])),
        ("padded id", device(vec![format!("{}=", &eid("a")[..42])])),
        (
            "url as id",
            device(vec!["https://fcm.googleapis.com/x".into()]),
        ),
        (
            "standard alphabet",
            device(vec![eid("abc").replace('_', "/")]),
        ),
        ("trailing bits", device(vec![non_canonical])),
    ]
}
fn devices_json(list: &[PushDevice]) -> Value {
    list.iter()
        .map(|d| json!({"deviceId":d.device_id,"endpoints":d.endpoints}))
        .collect()
}
/// The most one list may hold: 64 devices with 4 endpoints each.
fn at_limits() -> Vec<PushDevice> {
    (0..64)
        .map(|n| with(&format!("id-{n}"), &["a", "b", "c", "d"]))
        .collect()
}

#[tokio::test]
async fn push_init_checks_the_vapid_key_and_the_device_lists_and_drops_unknown_devices() {
    let devices = || vec![device(D1, "AQ", false), device(D2, "Ag", false)];
    let ok = gateway(&init_with(push_init(&[D2, "unknown", D1]), devices(), &[])).unwrap();
    assert_eq!(
        stored_set(&ok),
        BTreeMap::from([(D1.into(), vec![eid(D1)]), (D2.into(), vec![eid(D2)])])
    );
    assert!(ok.push.available());
    assert!(*ok.push.active.borrow());
    assert_eq!(ok.push.hello(), json!({"vapidPublicKey":vapid()}));

    let unknown = gateway(&init_with(push_init(&["unknown"]), devices(), &[])).unwrap();
    assert!(unknown.push.devices.lock().unwrap().is_empty());
    assert!(!*unknown.push.active.borrow());

    let none = gateway(&init_with(None, devices(), &[])).unwrap();
    assert!(!none.push.available());
    assert_eq!(none.push.hello(), Value::Null);

    let key = |value: String| {
        Some(PushInit {
            vapid_public_key: value,
            devices: Vec::new(),
        })
    };
    let mut bad = vec![
        String::new(),
        point(0x04, 64),
        point(0x04, 66),
        point(0x02, 65),
        format!("{}=", vapid()),
        vapid().replace('A', "+"),
    ];
    assert_ne!(vapid().replace('A', "+"), vapid());
    bad.push(URL_SAFE_NO_PAD.encode([1u8; 16]));
    for value in bad {
        assert!(
            matches!(
                gateway(&init_with(key(value.clone()), devices(), &[])),
                Err("invalid-push")
            ),
            "{value:?}"
        );
    }
    for (name, list) in violations() {
        let push = Some(PushInit {
            vapid_public_key: vapid(),
            devices: list,
        });
        assert!(
            matches!(
                gateway(&init_with(push, devices(), &[])),
                Err("invalid-push")
            ),
            "{name}"
        );
    }
    // Exactly at every limit is fine; four endpoints of a known device are kept.
    let push = Some(PushInit {
        vapid_public_key: vapid(),
        devices: at_limits()
            .into_iter()
            .chain([with(D1, &["a", "b", "c", "d"])])
            .skip(1)
            .collect(),
    });
    let full = gateway(&init_with(push, devices(), &[])).unwrap();
    assert_eq!(stored_set(&full)[D1].len(), 4);
    // Names that cannot be sessions are not watched; duplicates once.
    let sessions = init_with(push_init(&[]), devices(), &["b", "a", "b", "", " x", "-y"]);
    assert_eq!(gateway(&sessions).unwrap().push.sessions, ["a", "b"]);
}

#[tokio::test]
async fn push_devices_command_replaces_the_set_and_is_refused_without_push() {
    let devices = || vec![device(D1, "AQ", false), device(D2, "Ag", false)];
    let command = |list: &[PushDevice]| {
        CommandBody::Push(
            serde_json::from_value(
                json!({"v":1,"type":"push.devices","devices":devices_json(list)}),
            )
            .unwrap(),
        )
    };
    let gateway = gateway(&init_with(push_init(&[D1]), devices(), &[])).unwrap();
    let set = || stored_set(&gateway);
    assert_eq!(set(), BTreeMap::from([(D1.into(), vec![eid(D1)])]));
    // Unknown devices are ignored, and so is a repeated endpoint of a device.
    assert_eq!(
        gateway.command(command(&[with(D2, &["x", "x", "y"]), holding("unknown")])),
        Ok(admin::AdminResult::Applied)
    );
    assert_eq!(
        set(),
        BTreeMap::from([(D2.into(), {
            let mut ids = vec![eid("x"), eid("y")];
            ids.sort();
            ids
        })])
    );
    assert!(*gateway.push.active.borrow());
    assert_eq!(
        gateway.command(command(&[])),
        Ok(admin::AdminResult::Applied)
    );
    assert!(set().is_empty());
    assert!(!*gateway.push.active.borrow());
    // Four endpoints per device is the most.
    assert_eq!(
        gateway.command(command(&[with(D1, &["a", "b", "c", "d"])])),
        Ok(admin::AdminResult::Applied)
    );
    assert_eq!(set()[D1].len(), 4);
    for (name, list) in violations() {
        let refused = gateway.command(command(&list)).unwrap_err();
        assert_eq!(refused.0, axum::http::StatusCode::BAD_REQUEST, "{name}");
        assert_eq!(
            set()[D1].len(),
            4,
            "{name}: a refused command changes nothing"
        );
        assert!(*gateway.push.active.borrow(), "{name}");
    }
    assert_eq!(
        gateway.command(command(&at_limits())),
        Ok(admin::AdminResult::Applied)
    );

    let unavailable = gateway_without_push();
    let refused = unavailable.command(command(&[holding(D1)])).unwrap_err();
    assert_eq!(refused.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(unavailable.push.devices.lock().unwrap().is_empty());
}
fn gateway_without_push() -> Arc<Gateway> {
    gateway(&init_with(None, vec![device(D1, "AQ", false)], &[])).unwrap()
}

const FCM: &str = "https://fcm.googleapis.com/fcm/send/";

#[tokio::test]
async fn push_methods_emit_the_subscription_and_never_touch_the_set_themselves() {
    // `push.subscribe` has its own tests below: it waits for the desktop's ack.
    let (output, input) = tokio::io::duplex(1 << 16);
    let (events, _writer) = admin::spawn_writer(output);
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    let init = init_with(push_init(&[]), vec![device(D1, "AQ", false)], &[]);
    let gateway = Gateway::new(&init, events, clock).unwrap();
    let mut reader = tokio::io::BufReader::new(input);
    let call = |method: &str, params: Value| gateway.push.call(&gateway, D1, method, &params);
    let own = format!("{FCM}abc");

    assert_eq!(
        call("push.status", json!({"endpoint":own})),
        Ok(json!({"available":true,"subscribed":false}))
    );
    // Only that endpoint goes, named by its id: the URL is not sent back.
    assert_eq!(
        call("push.unsubscribe", json!({"endpoint":own})),
        Ok(json!({"subscribed":false}))
    );
    assert_eq!(
        read_event(&mut reader).await,
        json!({"v":1,"type":"push.subscription","deviceId":D1,"subscription":null,
            "endpointId":eid("abc")})
    );
    // Rejected input emits nothing: the next event read is the one after.
    assert_eq!(
        gateway
            .push
            .subscribe(
                &gateway,
                D1,
                &json!({"endpoint":"http://fcm.googleapis.com/x"})
            )
            .err(),
        Some("invalid-params")
    );
    let long = "x".repeat(2049);
    for params in [
        Value::Null,
        json!({}),
        json!({"endpoint":null}),
        json!({"endpoint":7}),
        json!({"endpoint":[own]}),
        json!({"endpoint":long}),
        json!({"endpoint":own,"x":1}),
        json!({"x":1}),
    ] {
        assert_eq!(
            call("push.unsubscribe", params.clone()),
            Err("invalid-params"),
            "{params}"
        );
    }
    for params in [
        json!({"endpoint":7}),
        json!({"endpoint":long}),
        json!({"endpoint":own,"x":1}),
        json!({"x":1}),
        json!([own]),
        json!("endpoint"),
    ] {
        assert_eq!(
            call("push.status", params.clone()),
            Err("invalid-params"),
            "{params}"
        );
    }
    assert_eq!(call("push.other", Value::Null), Err("unknown-method"));
    // The longest endpoint is only hashed.
    let longest = "x".repeat(2048);
    assert_eq!(
        call("push.status", json!({"endpoint":longest})),
        Ok(json!({"available":true,"subscribed":false}))
    );
    // Only the desktop's `push.devices` changes the set.
    assert!(gateway.push.devices.lock().unwrap().is_empty());
    gateway.push.replace(&[holding("abc")], |_| true);
    assert_eq!(stored_set(&gateway).len(), 1);
    gateway.push.replace(&[with(D1, &["abc"])], |_| true);
    assert_eq!(
        call("push.status", json!({"endpoint":own})),
        Ok(json!({"available":true,"subscribed":true}))
    );
    call("push.unsubscribe", json!({"endpoint":own})).unwrap();
    assert_eq!(
        read_event(&mut reader).await["endpointId"],
        json!(eid("abc"))
    );
}
async fn read_event(reader: &mut tokio::io::BufReader<tokio::io::DuplexStream>) -> Value {
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(reader, &mut line)
        .await
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn push_status_is_true_only_for_an_endpoint_stored_for_that_device() {
    let (output, input) = tokio::io::duplex(1 << 16);
    let (events, _writer) = admin::spawn_writer(output);
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    // The phone and the tablet share one synced passkey, so one device D1; the
    // desktop stored the phone's endpoint and one more of the same device.
    let init = init_with(
        Some(PushInit {
            vapid_public_key: vapid(),
            devices: vec![with(D1, &["phone", "tablet"]), with(D2, &["other"])],
        }),
        vec![device(D1, "AQ", false), device(D2, "Ag", false)],
        &[],
    );
    let gateway = Gateway::new(&init, events, clock).unwrap();
    let mut reader = tokio::io::BufReader::new(input);
    let status = |device: &str, endpoint: Value| {
        gateway
            .push
            .call(
                &gateway,
                device,
                "push.status",
                &json!({"endpoint":endpoint}),
            )
            .unwrap()["subscribed"]
            .clone()
    };
    let url = |name: &str| format!("{FCM}{name}");
    assert_eq!(status(D1, json!(url("phone"))), true);
    assert_eq!(status(D1, json!(url("tablet"))), true);
    // A browser the desktop has no subscription of: off, though the device is on.
    assert_eq!(status(D1, json!(url("laptop"))), false);
    assert_eq!(status(D1, Value::Null), false);
    // Another device's endpoint says nothing for this one.
    assert_eq!(status(D1, json!(url("other"))), false);
    assert_eq!(status(D2, json!(url("other"))), true);
    assert_eq!(status(D2, json!(url("phone"))), false);
    // A device the desktop holds nothing for.
    assert_eq!(status(D3, json!(url("phone"))), false);

    // Turning one browser off names its endpoint only; the desktop's next
    // `push.devices` then keeps the other.
    gateway
        .push
        .call(
            &gateway,
            D1,
            "push.unsubscribe",
            &json!({"endpoint":url("phone")}),
        )
        .unwrap();
    assert_eq!(
        read_event(&mut reader).await,
        json!({"v":1,"type":"push.subscription","deviceId":D1,"subscription":null,
            "endpointId":eid("phone")})
    );
    assert_eq!(
        status(D1, json!(url("phone"))),
        true,
        "the set did not move"
    );
    assert!(gateway
        .push
        .replace(&[with(D1, &["tablet"]), with(D2, &["other"])], |_| true));
    assert_eq!(status(D1, json!(url("phone"))), false);
    assert_eq!(status(D1, json!(url("tablet"))), true);
    assert!(*gateway.push.active.borrow());
    // The watchers run for a device with at least one endpoint, not otherwise.
    gateway.push.replace(&[], |_| true);
    assert!(!*gateway.push.active.borrow());
    gateway.push.replace(&[with(D1, &["tablet"])], |_| true);
    assert!(*gateway.push.active.borrow());
}

#[tokio::test]
async fn push_is_unavailable_without_the_init_key() {
    let gateway = gateway_without_push();
    let call = |method: &str, params: Value| gateway.push.call(&gateway, D1, method, &params);
    let (p256dh, auth) = (point(0x04, 65), URL_SAFE_NO_PAD.encode([9u8; 16]));
    let valid = json!({"endpoint":format!("{FCM}x"),"keys":{"p256dh":p256dh,"auth":auth}});
    // Availability is decided before the params are looked at.
    let subscribe = |params: &Value| gateway.push.subscribe(&gateway, D1, params).err();
    assert_eq!(subscribe(&valid), Some("push-unavailable"));
    assert_eq!(subscribe(&Value::Null), Some("push-unavailable"));
    assert_eq!(call("push.unsubscribe", json!({})), Err("push-unavailable"));
    assert_eq!(
        call("push.unsubscribe", json!({"endpoint":format!("{FCM}x")})),
        Err("push-unavailable")
    );
    assert_eq!(
        call("push.status", json!({"endpoint":format!("{FCM}x")})),
        Ok(json!({"available":false,"subscribed":false}))
    );
    assert_eq!(
        call("push.status", json!({})),
        Ok(json!({"available":false,"subscribed":false}))
    );
}

/// A scripted Herdr: a world of panes, and the subscriptions asked of it.
#[derive(Default)]
struct Fake {
    world: Mutex<Value>,
    subscriptions: Mutex<HashMap<String, OnSubscriptionEvent>>,
    log: Mutex<Vec<String>>,
    attempts: Mutex<Vec<std::time::Instant>>,
    snapshots: AtomicUsize,
    fail_subscribe: AtomicUsize,
    next: AtomicUsize,
}
impl Fake {
    fn with(panes: &[(&str, &str)]) -> Arc<Self> {
        let fake = Arc::new(Self::default());
        fake.set(panes);
        fake
    }
    fn set(&self, panes: &[(&str, &str)]) {
        *self.world.lock().unwrap() = json!({
            "workspaces":[{"workspace_id":"w","label":"Space One"}],
            "tabs":[],
            "panes":panes.iter().map(|(id, status)| json!({
                "pane_id":id,"workspace_id":"w","tab_id":"w:t","agent_status":status,"label":format!("label {id}")
            })).collect::<Vec<_>>(),
            "agents":panes.iter().map(|(id, status)| json!({
                "pane_id":id,"name":format!("Agent {id}"),"agent":"claude","agent_status":status
            })).collect::<Vec<_>>(),
            "layouts":[]
        });
    }
    fn active(&self) -> usize {
        self.subscriptions.lock().unwrap().len()
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn subscribes(&self) -> Vec<String> {
        self.log()
            .into_iter()
            .filter(|entry| entry.starts_with("subscribe:"))
            .collect()
    }
    fn released(&self, subscription: &str) -> bool {
        self.log().contains(&format!("release:{subscription}"))
    }
    /// Delivers an event the way the Host reader thread would.
    fn fire(&self, event: HerdrSubscriptionEvent) {
        let sinks: Vec<_> = self
            .subscriptions
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect();
        for sink in sinks {
            let _ = sink(event.clone());
        }
    }
    fn status(&self, pane: &str, status: &str) {
        self.fire(HerdrSubscriptionEvent::AgentStatusChanged {
            subscription_id: "x".into(),
            pane_id: pane.into(),
            workspace_id: "w".into(),
            agent_status: status.into(),
            agent: None,
            display_agent: None,
            title: None,
            state_labels: HashMap::new(),
        });
    }
    fn topology(&self, kind: &str) {
        self.fire(HerdrSubscriptionEvent::TopologyChanged {
            subscription_id: "x".into(),
            kind: kind.into(),
            workspace_id: None,
            tab_id: None,
        });
    }
}
impl HerdrPort for Fake {
    crate::test_support::reject_writes!();
    fn sessions(&self) -> HerdrResult<Vec<HerdrNamedSession>> {
        Ok(vec![HerdrNamedSession {
            name: "work".into(),
            running: true,
            default: true,
            session_dir: "SECRET".into(),
            socket_path: "SECRET".into(),
        }])
    }
    fn snapshot(&self, _: &str) -> HerdrResult<HerdrSnapshotResult> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        Ok(HerdrSnapshotResult {
            protocol: 22,
            version: "0.9.5".into(),
            snapshot: self.world.lock().unwrap().clone(),
        })
    }
    fn observe(&self, _: &str, _: &str, _: u16, _: u16, _: OnTerminalEvent) -> HerdrResult<String> {
        panic!("unexpected observe")
    }
    fn subscribe(
        &self,
        session: &str,
        panes: Vec<String>,
        sink: OnSubscriptionEvent,
    ) -> HerdrResult<String> {
        self.attempts
            .lock()
            .unwrap()
            .push(std::time::Instant::now());
        if self
            .fail_subscribe
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err("herdr-unavailable");
        }
        let id = format!("events-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.log
            .lock()
            .unwrap()
            .push(format!("subscribe:{session}:{}", panes.join(",")));
        self.subscriptions.lock().unwrap().insert(id.clone(), sink);
        Ok(id)
    }
    fn release(&self, resource: Resource) {
        if let Resource::Events(id) = resource {
            self.log.lock().unwrap().push(format!("release:{id}"));
            self.subscriptions.lock().unwrap().remove(&id);
        }
    }
}

/// A gateway on a Herdr port, its admin events logged, and the watcher running.
struct Rig {
    gateway: Arc<Gateway>,
    fake: Arc<Fake>,
    events: Arc<Mutex<Vec<Value>>>,
    stop: watch::Sender<u16>,
    watcher: Option<JoinHandle<()>>,
    background: Vec<AbortHandle>,
}
impl Rig {
    async fn start(fake: Arc<Fake>, push: Option<PushInit>, devices: Vec<Device>) -> Self {
        Self::on(fake.clone(), fake, push, devices).await
    }
    async fn on(
        fake: Arc<Fake>,
        port: Arc<dyn HerdrPort>,
        push: Option<PushInit>,
        devices: Vec<Device>,
    ) -> Self {
        Self::paced(fake, port, push, devices, FAST).await
    }
    async fn paced(
        fake: Arc<Fake>,
        port: Arc<dyn HerdrPort>,
        push: Option<PushInit>,
        devices: Vec<Device>,
        pace: Pace,
    ) -> Self {
        let init = init_with(push, devices, &["work"]);
        let (output, input) = tokio::io::duplex(1 << 20);
        let (events, writer) = admin::spawn_writer(output);
        let log = Arc::new(Mutex::new(Vec::new()));
        let drain = tokio::spawn({
            let log = log.clone();
            async move {
                let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(input));
                while let Ok(Some(line)) = lines.next_line().await {
                    log.lock()
                        .unwrap()
                        .push(serde_json::from_str(&line).unwrap());
                }
            }
        });
        let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
        let gateway = Gateway::with_herdr(&init, events, clock, port).unwrap();
        let (stop, stopping) = watch::channel(0u16);
        Self {
            watcher: Some(tokio::spawn(run_paced(gateway.clone(), stopping, pace))),
            background: vec![writer.abort_handle(), drain.abort_handle()],
            gateway,
            fake,
            events: log,
            stop,
        }
    }
    /// Devices D1 and D2 exist; `set` holds a subscription. Returns once the
    /// first look (two snapshots around the subscription) is done.
    async fn watching(fake: Arc<Fake>, set: &[&str]) -> Self {
        let devices = vec![device(D1, "AQ", false), device(D2, "Ag", false)];
        let rig = Self::start(fake, push_init(set), devices).await;
        rig.subscribed().await;
        until("the first look", || {
            rig.fake.snapshots.load(Ordering::SeqCst) >= 2
        })
        .await;
        rig
    }
    async fn subscribed(&self) {
        until("a subscription", || self.fake.active() == 1).await;
    }
    /// Requests so far, as `kind pane`.
    fn requested(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "push.request")
            .map(|event| {
                format!(
                    "{} {}",
                    event["payload"]["kind"].as_str().unwrap(),
                    event["payload"]["paneId"].as_str().unwrap()
                )
            })
            .collect()
    }
    fn requests(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["type"] == "push.request")
            .cloned()
            .collect()
    }
    /// A watcher handles its events in order, so once the last one of a batch
    /// shows up, nothing earlier is still on its way.
    async fn requested_through(&self, last: &str) -> Vec<String> {
        until("the last push request", || {
            self.requested().iter().any(|request| request == last)
        })
        .await;
        self.requested()
    }
    fn set(&self, ids: &[&str]) {
        let list: Vec<_> = ids.iter().map(|id| holding(id)).collect();
        let command = serde_json::from_value(
            json!({"v":1,"type":"push.devices","devices":devices_json(&list)}),
        );
        self.gateway
            .command(CommandBody::Push(command.unwrap()))
            .unwrap();
    }
}
impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(watcher) = &self.watcher {
            watcher.abort();
        }
        for task in &self.background {
            task.abort();
        }
    }
}
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn push_watcher_baseline_is_silent_and_a_status_pushes_only_on_a_new_blocked_or_done() {
    let fake = Fake::with(&[("w:p", "blocked"), ("w:q", "working"), ("w:r", "idle")]);
    let rig = Rig::watching(fake.clone(), &[D1]).await;
    assert_eq!(fake.subscribes(), ["subscribe:work:w:p,w:q,w:r"]);
    fake.status("w:p", "blocked"); // already blocked at the baseline
    fake.status("w:q", "blocked"); // working -> blocked
    fake.status("w:q", "blocked"); // blocked -> blocked
    fake.status("w:q", "working");
    fake.status("w:q", "idle");
    fake.status("w:q", "done"); // idle -> done
    fake.status("w:r", "surprising"); // unrecognised: not blocked
    fake.status("w:r", "blocked"); // unknown -> blocked
    fake.status("w:r", "done"); // blocked -> done
    fake.status("w:r", "done");
    fake.status("w:p", "done"); // blocked at the baseline -> done
    assert_eq!(
        rig.requested_through("done w:p").await,
        [
            "blocked w:q",
            "done w:q",
            "blocked w:r",
            "done w:r",
            "done w:p"
        ]
    );
    assert_eq!(
        rig.requests()[0],
        json!({"v":1,"type":"push.request","deviceIds":[D1],"payload":{
            "kind":"blocked","session":"work","paneId":"w:q","workspaceId":"w",
            "title":"Agent w:q","space":"Space One"}})
    );
    assert_eq!(fake.subscribes().len(), 1, "no resubscription");
}

#[tokio::test]
async fn push_watcher_topology_change_resubscribes_and_a_pane_update_does_not() {
    let fake = Fake::with(&[("w:p", "working"), ("w:q", "working")]);
    let rig = Rig::watching(fake.clone(), &[D1]).await;
    fake.topology("pane.updated");
    fake.fire(HerdrSubscriptionEvent::WorktreeChanged {
        subscription_id: "x".into(),
        kind: "created".into(),
        workspace_id: None,
    });
    fake.status("w:q", "blocked");
    assert_eq!(rig.requested_through("blocked w:q").await, ["blocked w:q"]);
    assert_eq!(fake.subscribes(), ["subscribe:work:w:p,w:q"]);

    // w:p turned blocked and w:r appeared (already blocked) before the look.
    fake.set(&[("w:p", "blocked"), ("w:q", "working"), ("w:r", "blocked")]);
    fake.topology("pane.created");
    until("the second subscription", || {
        fake.subscribes().len() == 2 && fake.released("events-0") && fake.active() == 1
    })
    .await;
    assert_eq!(fake.subscribes()[1], "subscribe:work:w:p,w:q,w:r");
    fake.status("w:r", "done");
    // w:p's change is found by the snapshot; w:q leaving blocked and w:r being
    // new announce nothing.
    assert_eq!(
        rig.requested_through("done w:r").await,
        ["blocked w:q", "blocked w:p", "done w:r"]
    );
}

#[tokio::test]
async fn push_watcher_finds_what_changed_before_the_subscription_took_effect() {
    // Once the first subscription exists, w:q is blocked and w:new has appeared.
    struct Racing(Arc<Fake>);
    impl HerdrPort for Racing {
        crate::test_support::reject_writes!();
        fn sessions(&self) -> HerdrResult<Vec<HerdrNamedSession>> {
            self.0.sessions()
        }
        fn snapshot(&self, session: &str) -> HerdrResult<HerdrSnapshotResult> {
            self.0.snapshot(session)
        }
        fn observe(
            &self,
            _: &str,
            _: &str,
            _: u16,
            _: u16,
            _: OnTerminalEvent,
        ) -> HerdrResult<String> {
            panic!("unexpected observe")
        }
        fn subscribe(
            &self,
            session: &str,
            panes: Vec<String>,
            sink: OnSubscriptionEvent,
        ) -> HerdrResult<String> {
            let id = self.0.subscribe(session, panes, sink);
            if self.0.subscribes().len() == 1 {
                self.0
                    .set(&[("w:p", "working"), ("w:q", "blocked"), ("w:new", "working")]);
            }
            id
        }
        fn release(&self, resource: Resource) {
            self.0.release(resource)
        }
    }
    let fake = Fake::with(&[("w:p", "working"), ("w:q", "working")]);
    let rig = Rig::on(
        fake.clone(),
        Arc::new(Racing(fake.clone())),
        push_init(&[D1]),
        vec![device(D1, "AQ", false)],
    )
    .await;
    // The second look shows another pane set, so it subscribes again, now to all.
    until("the second subscription", || fake.subscribes().len() == 2).await;
    assert_eq!(
        fake.subscribes(),
        ["subscribe:work:w:p,w:q", "subscribe:work:w:new,w:p,w:q"]
    );
    assert_eq!(rig.requested_through("blocked w:q").await, ["blocked w:q"]);
    until("the first subscription's release", || {
        fake.released("events-0")
    })
    .await;
    assert_eq!(rig.requested(), ["blocked w:q"], "announced once");
}

#[tokio::test]
async fn push_watcher_subscription_failures_back_off_and_recover() {
    let fake = Fake::with(&[("w:p", "working")]);
    fake.fail_subscribe.store(3, Ordering::SeqCst);
    let rig = Rig::start(
        fake.clone(),
        push_init(&[D1]),
        vec![device(D1, "AQ", false)],
    )
    .await;
    rig.subscribed().await;
    let attempts = fake.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 4);
    let gaps: Vec<_> = attempts.windows(2).map(|w| w[1] - w[0]).collect();
    // Waits of 5, 10 and 20 ms: each at least as long as asked.
    assert!(gaps[0] >= Duration::from_millis(5), "{gaps:?}");
    assert!(gaps[1] >= Duration::from_millis(10), "{gaps:?}");
    assert!(gaps[2] >= Duration::from_millis(20), "{gaps:?}");
    fake.set(&[("w:p", "blocked")]);
    fake.status("w:p", "blocked");
    assert_eq!(rig.requested_through("blocked w:p").await, ["blocked w:p"]);

    // A subscription that ends (the Host reader died) is taken up again.
    fake.fire(HerdrSubscriptionEvent::Disconnected {
        subscription_id: "x".into(),
        reason: Some("socket closed".into()),
    });
    until("the retry", || {
        fake.subscribes().len() == 2 && fake.released("events-0") && fake.active() == 1
    })
    .await;
    // What was learned survives: blocked -> blocked is still nothing.
    fake.status("w:p", "blocked");
    fake.status("w:p", "done");
    assert_eq!(
        rig.requested_through("done w:p").await,
        ["blocked w:p", "done w:p"]
    );
}

#[tokio::test]
async fn push_watcher_ends_a_subscription_whose_events_overflow() {
    let fake = Fake::with(&[("w:p", "working")]);
    let rig = Rig::watching(fake.clone(), &[D1]).await;
    let sink = fake
        .subscriptions
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .clone();
    // The watcher cannot run during this loop (single-threaded test runtime),
    // and the Host's reader thread must never wait: a full buffer is an error.
    let exited = || HerdrSubscriptionEvent::PaneExited {
        subscription_id: "x".into(),
        pane_id: "w:gone".into(),
        workspace_id: "w".into(),
    };
    let refused = (0..EVENTS + 10).filter(|_| sink(exited()).is_err()).count();
    assert_eq!(refused, 10);
    // That ends the Host's reader, which drops the sink and closes the channel.
    fake.subscriptions.lock().unwrap().clear();
    drop(sink);
    until("a new subscription", || {
        fake.subscribes().len() == 2 && fake.active() == 1
    })
    .await;
    assert!(fake.released("events-0"));
    drop(rig);
}

#[tokio::test]
async fn push_watcher_stops_when_the_set_empties_and_starts_over_with_a_new_baseline() {
    let fake = Fake::with(&[("w:p", "working"), ("w:q", "working")]);
    let rig = Rig::watching(fake.clone(), &[D1]).await;
    rig.set(&[]);
    until("the subscription to be released", || fake.active() == 0).await;
    let snapshots = fake.snapshots.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(fake.snapshots.load(Ordering::SeqCst), snapshots);
    assert_eq!(fake.log().len(), 2, "subscribe, release; nothing since");

    // Blocked while nobody listened: the new starting point, not news.
    fake.set(&[("w:p", "working"), ("w:q", "blocked")]);
    rig.set(&[D2]);
    rig.subscribed().await;
    fake.status("w:p", "blocked");
    assert_eq!(rig.requested_through("blocked w:p").await, ["blocked w:p"]);
    assert_eq!(rig.requests()[0]["deviceIds"], json!([D2]));
}

#[tokio::test]
async fn push_watcher_stops_on_shutdown_and_hands_the_subscription_back() {
    let fake = Fake::with(&[("w:p", "working")]);
    let mut rig = Rig::watching(fake.clone(), &[D1]).await;
    rig.stop.send_replace(1001);
    tokio::time::timeout(Duration::from_secs(3), rig.watcher.take().unwrap())
        .await
        .unwrap()
        .unwrap();
    until("the release", || fake.active() == 0).await;
}

#[tokio::test]
async fn push_the_listener_runs_the_watcher_and_stops_it_with_the_server() {
    let fake = Fake::with(&[("w:p", "working")]);
    let init = init_with(push_init(&[D1]), vec![device(D1, "AQ", false)], &["work"]);
    let (events, _writer) = admin::spawn_writer(tokio::io::sink());
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    let gateway = Gateway::with_herdr(&init, events, clock, fake.clone()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let server = crate::http::Server::bind(&init.listeners.loopback, root.path(), gateway)
        .await
        .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.run(stopped));
    until("a subscription", || fake.active() == 1).await;
    stop.send(()).unwrap();
    // `true` would mean the policy expired.
    assert!(!tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap());
    until("the release", || fake.active() == 0).await;
}

#[tokio::test]
async fn push_watcher_never_runs_without_push() {
    let fake = Fake::with(&[("w:p", "blocked")]);
    let mut rig = Rig::start(fake.clone(), None, vec![device(D1, "AQ", false)]).await;
    tokio::time::timeout(Duration::from_secs(3), rig.watcher.take().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fake.snapshots.load(Ordering::SeqCst), 0);
    assert!(fake.log().is_empty());
}

#[tokio::test]
async fn push_requests_leave_out_revoked_and_locked_devices() {
    let fake = Fake::with(&[("w:p", "working")]);
    let devices = vec![
        device(D1, "AQ", false),
        device(D2, "Ag", false),
        device(D3, "Aw", true),
    ];
    let rig = Rig::start(fake.clone(), push_init(&[D1, D2, D3]), devices).await;
    rig.subscribed().await;
    fake.status("w:p", "blocked");
    rig.requested_through("blocked w:p").await;
    assert_eq!(
        rig.requests()[0]["deviceIds"],
        json!([D1, D2]),
        "D3 is locked"
    );

    let revoke = |id: &str| {
        rig.gateway
            .command(CommandBody::Device(DeviceCommand::Revoke {
                v: 1,
                device_id: id.into(),
            }))
            .unwrap()
    };
    let set = || {
        rig.gateway
            .push
            .devices
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    let subscribed = |device: &str, name: &str| {
        let params = json!({"endpoint":format!("{FCM}{name}")});
        rig.gateway
            .push
            .call(&rig.gateway, device, "push.status", &params)
            .unwrap()["subscribed"]
            .clone()
    };
    assert_eq!(subscribed(D2, D2), true);
    revoke(D2);
    assert_eq!(set(), [D1, D3], "revoking removes the device from the set");
    assert_eq!(
        subscribed(D2, D2),
        false,
        "and every endpoint it held with it"
    );
    fake.status("w:p", "done");
    rig.requested_through("done w:p").await;
    assert_eq!(rig.requests()[1]["deviceIds"], json!([D1]));

    // Only a locked device is left: the watcher runs, nobody is told.
    revoke(D1);
    assert_eq!(set(), [D3]);
    assert!(targets(&rig.gateway).is_empty());
    fake.status("w:p", "blocked");
    fake.status("w:p", "done");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.requests().len(), 2);
    // The desktop's list cannot bring a revoked device back.
    rig.set(&[D1, D2, D3]);
    assert_eq!(set(), [D3]);
}

#[tokio::test]
async fn push_request_dropped_on_a_full_admin_queue_is_counted_as_lost() {
    // Nothing reads stdout yet, so the ordinary-class slots stay taken.
    let (output, input) = tokio::io::duplex(1);
    let (events, _writer) = admin::spawn_writer(output);
    let init = init_with(push_init(&[D1]), vec![device(D1, "AQ", false)], &["work"]);
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    let gateway = Gateway::with_herdr(&init, events.clone(), clock, Fake::with(&[])).unwrap();
    while events.try_emit(Event::push_unsubscription("filler".into(), eid("a"))) {}
    let notice = Notice {
        kind: "blocked",
        pane: "w:p".into(),
        workspace: None,
        title: "Agent".into(),
        space: None,
    };
    notify(&gateway, "work", [notice]);
    // Reading frees the slots; the count then goes out with the next flush.
    let lines = Arc::new(Mutex::new(Vec::<Value>::new()));
    let reader = tokio::spawn({
        let lines = lines.clone();
        async move {
            let mut input = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(input));
            while let Ok(Some(line)) = input.next_line().await {
                lines
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str(&line).unwrap());
            }
        }
    });
    until("audit.dropped", || {
        gateway.maintenance();
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "audit.dropped")
    })
    .await;
    let lines = lines.lock().unwrap();
    let dropped = lines
        .iter()
        .find(|event| event["type"] == "audit.dropped")
        .unwrap();
    assert_eq!(
        (dropped["lost"].as_u64(), dropped["lostTightening"].as_u64()),
        (Some(1), Some(0))
    );
    assert!(lines.iter().all(|event| event["type"] != "push.request"));
    reader.abort();
}

/// Waits in virtual time, which runs on by itself while everything is idle.
async fn patiently(what: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3600), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}
fn disconnected() -> HerdrSubscriptionEvent {
    HerdrSubscriptionEvent::Disconnected {
        subscription_id: "x".into(),
        reason: Some("socket closed".into()),
    }
}
/// A watcher that failed five times is waiting out the longest delay when it
/// comes up. It then stays up for ten minutes, a topology change every 20 s
/// cutting that into 31 subscriptions, none of them as long as that delay.
async fn long_healthy_stretch_in_short_cycles() -> Rig {
    let fake = Fake::with(&[("w:p", "working")]);
    fake.fail_subscribe.store(5, Ordering::SeqCst);
    let rig = Rig::paced(
        fake.clone(),
        fake.clone(),
        push_init(&[D1]),
        vec![device(D1, "AQ", false)],
        Pace::DEFAULT,
    )
    .await;
    // 5 + 10 + 20 + 40 + 60 seconds of waiting.
    patiently("the sixth try", || fake.active() == 1).await;
    for look in 2..=31 {
        tokio::time::sleep(Duration::from_secs(20)).await;
        fake.topology("pane.created");
        patiently("the new subscription", || fake.subscribes().len() == look).await;
    }
    tokio::time::sleep(Duration::from_secs(10)).await;
    rig
}

#[tokio::test(start_paused = true)]
async fn push_watcher_retries_quickly_when_a_long_healthy_stretch_in_short_cycles_ends_in_a_drop() {
    let rig = long_healthy_stretch_in_short_cycles().await;
    let fake = rig.fake.clone();
    fake.fire(disconnected());
    let dropped = tokio::time::Instant::now();
    patiently("the retry", || fake.subscribes().len() == 32).await;
    let waited = dropped.elapsed();
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(10),
        "waited {waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn push_watcher_retries_quickly_when_a_long_healthy_stretch_ends_in_a_failed_look() {
    let rig = long_healthy_stretch_in_short_cycles().await;
    let fake = rig.fake.clone();
    let tries = fake.attempts.lock().unwrap().len();
    // Herdr is busy when the watcher looks again after the next change.
    fake.fail_subscribe.store(1, Ordering::SeqCst);
    fake.topology("pane.created");
    patiently("the failed try", || {
        fake.attempts.lock().unwrap().len() == tries + 1
    })
    .await;
    let failed = tokio::time::Instant::now();
    patiently("the retry", || {
        fake.attempts.lock().unwrap().len() == tries + 2
    })
    .await;
    let waited = failed.elapsed();
    assert!(
        waited >= Duration::from_secs(4) && waited < Duration::from_secs(10),
        "waited {waited:?}"
    );
    // A failure ends the streak: the next one doubles the wait again.
    fake.fail_subscribe.store(1, Ordering::SeqCst);
    fake.topology("pane.created");
    patiently("the second failed try", || {
        fake.attempts.lock().unwrap().len() == tries + 4
    })
    .await;
    patiently("recovered", || fake.active() == 1).await;
}

/// A gateway with push whose admin events can be read, and a valid `push.subscribe` params object.
fn subscribing() -> (
    Arc<Gateway>,
    tokio::io::BufReader<tokio::io::DuplexStream>,
    Value,
) {
    let (output, input) = tokio::io::duplex(1 << 16);
    let (events, _writer) = admin::spawn_writer(output);
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    let init = init_with(push_init(&[]), vec![device(D1, "AQ", false)], &[]);
    let params = json!({"endpoint":format!("{FCM}abc"),"keys":{
        "p256dh":vapid(),"auth":URL_SAFE_NO_PAD.encode([9u8; 16])}});
    (
        Gateway::new(&init, events, clock).unwrap(),
        tokio::io::BufReader::new(input),
        params,
    )
}
fn ack(request: u64, stored: bool) -> CommandBody {
    CommandBody::PushAck(
        serde_json::from_value(
            json!({"v":1,"type":"push.ack","requestId":request,"stored":stored}),
        )
        .unwrap(),
    )
}

#[tokio::test(start_paused = true)]
async fn push_subscribe_replies_on_only_after_the_desktop_acks_that_it_stored_it() {
    let (gateway, mut reader, params) = subscribing();
    let mut reply = gateway.push.subscribe(&gateway, D1, &params).ok().unwrap();
    let event = read_event(&mut reader).await;
    assert_eq!(event["type"], "push.subscription");
    let request = event["requestId"].as_u64().unwrap();
    // Queued is not stored: the phone hears nothing yet.
    assert!(tokio::time::timeout(Duration::from_secs(1), &mut reply)
        .await
        .is_err());
    // Another request's ack does not release it.
    gateway.command(ack(request + 1, true)).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(1), &mut reply)
        .await
        .is_err());
    assert_eq!(
        gateway.command(ack(request, true)),
        Ok(admin::AdminResult::Applied)
    );
    assert_eq!(reply.await, Ok(json!({"subscribed":true})));
    assert!(gateway.push.acks.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn push_subscribe_fails_when_the_desktop_could_not_store_it() {
    let (gateway, mut reader, params) = subscribing();
    let reply = gateway.push.subscribe(&gateway, D1, &params).ok().unwrap();
    let request = read_event(&mut reader).await["requestId"].as_u64().unwrap();
    gateway.command(ack(request, false)).unwrap();
    assert_eq!(reply.await, Err("push-store-failed"));
}

#[tokio::test(start_paused = true)]
async fn push_subscribe_gives_up_without_an_ack_and_ignores_a_late_one() {
    let (gateway, mut reader, params) = subscribing();
    let reply = gateway.push.subscribe(&gateway, D1, &params).ok().unwrap();
    let request = read_event(&mut reader).await["requestId"].as_u64().unwrap();
    // Nobody answers: bounded, and a failure the phone can show.
    assert_eq!(reply.await, Err("push-unconfirmed"));
    assert!(gateway.push.acks.lock().unwrap().is_empty());
    assert_eq!(
        gateway.command(ack(request, true)),
        Ok(admin::AdminResult::Applied)
    );
    // Two phones never share a request id.
    let again = gateway.push.subscribe(&gateway, D1, &params).ok().unwrap();
    assert_ne!(
        read_event(&mut reader).await["requestId"].as_u64().unwrap(),
        request
    );
    drop(again);
}

#[tokio::test]
async fn push_subscribe_with_a_full_admin_queue_is_busy_and_leaves_nothing_waiting() {
    let (output, _input) = tokio::io::duplex(1);
    let (events, _writer) = admin::spawn_writer(output);
    let clock = Clock::at(serde_json::from_str("\"2030-01-01T00:00:00Z\"").unwrap());
    let init = init_with(push_init(&[]), vec![device(D1, "AQ", false)], &[]);
    let gateway = Gateway::new(&init, events.clone(), clock).unwrap();
    while events.try_emit(Event::push_unsubscription(D1.into(), eid("a"))) {}
    let params = json!({"endpoint":format!("{FCM}abc"),"keys":{
        "p256dh":vapid(),"auth":URL_SAFE_NO_PAD.encode([9u8; 16])}});
    assert_eq!(
        gateway.push.subscribe(&gateway, D1, &params).err(),
        Some("busy")
    );
    assert!(gateway.push.acks.lock().unwrap().is_empty());
}
