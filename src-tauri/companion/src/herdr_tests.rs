use super::*;
use std::sync::Mutex;

pub(crate) fn snapshot_fixture() -> Value {
    json!({"workspaces":[{"workspace_id":"w1","label":"Work","cwd":"SECRET"}],"tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"Tab"}],"panes":[{"pane_id":"w1:p1","terminal_id":"term_1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"working","cwd":"SECRET","tokens":{"token":"SECRET"}}],"agents":[{"pane_id":"w1:p1","name":"Agent","agent":"test","agent_status":"other","tokens":{"key":"SECRET"}}],"layouts":[{"panes":[{"pane_id":"w1:p1","rect":{"width":120,"height":40}}]}]})
}
struct Fake {
    calls: Mutex<Vec<String>>,
}
impl HerdrPort for Fake {
    crate::test_support::reject_writes!();
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>> {
        self.calls.lock().unwrap().push("list".into());
        Ok([("running", true), ("stopped", false), ("secret", true)]
            .into_iter()
            .map(|(name, running)| HerdrNamedSession {
                name: name.into(),
                running,
                default: false,
                session_dir: "SECRET".into(),
                socket_path: "SECRET".into(),
            })
            .collect())
    }
    fn snapshot(&self, session: &str) -> Result<HerdrSnapshotResult> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("snapshot:{session}"));
        Ok(HerdrSnapshotResult {
            version: "0.9.1".into(),
            protocol: 22,
            snapshot: snapshot_fixture(),
        })
    }
    fn observe(&self, _: &str, _: &str, _: u16, _: u16, _: OnTerminalEvent) -> Result<String> {
        panic!("unexpected attach")
    }
    fn subscribe(&self, _: &str, _: OnSubscriptionEvent) -> Result<String> {
        panic!("unexpected subscribe")
    }
    fn release(&self, _: Resource) {
        panic!("unexpected release")
    }
}
#[test]
fn bridge_allowlist_running_only_no_autostart_or_stopped_snapshot() {
    let fake = Arc::new(Fake {
        calls: Mutex::new(Vec::new()),
    });
    let reads = Reads::new(fake.clone(), &["running".into(), "stopped".into()]);
    assert_eq!(
        reads.sessions().unwrap(),
        json!({"sessions":[{"name":"running","default":false}]})
    );
    assert_eq!(reads.snapshot("stopped"), Err("session-stopped"));
    assert_eq!(reads.snapshot("secret"), Err("session-not-allowed"));
    assert_eq!(*fake.calls.lock().unwrap(), vec!["list", "list"]);
    assert!(reads.snapshot("running").is_ok());
}
#[test]
fn bridge_snapshot_minimal_normalized_and_layout_not_pty() {
    let raw = snapshot_fixture();
    let normalized = normalize_snapshot("work", &raw).unwrap();
    assert!(!normalized.to_string().contains("SECRET"));
    assert!(normalized.get("protocol").is_none());
    assert_eq!(normalized["agents"][0]["status"], "unknown");
    assert_eq!(
        normalized["panes"][0]["layout"],
        json!({"cols":120,"rows":40})
    );
    assert_eq!(
        observe_geometry(&raw, "term_1").unwrap(),
        ("w1:p1".into(), 120, 40)
    );
    assert_eq!(
        observe_geometry(&raw, "w1:p1").unwrap(),
        ("w1:p1".into(), 120, 40)
    );
    assert_eq!(observe_geometry(&raw, "missing"), Err("target-not-found"));
    let mut absent = raw;
    absent["layouts"] = json!([]);
    assert_eq!(
        observe_geometry(&absent, "w1:p1"),
        Err("geometry-unavailable")
    );
}
#[test]
fn bridge_snapshot_payload_too_large() {
    let mut raw = snapshot_fixture();
    raw["workspaces"][0]["label"] = json!("x".repeat(SNAPSHOT_BYTES));
    assert_eq!(normalize_snapshot("work", &raw), Err("payload-too-large"));
}
#[test]
fn bridge_stream_reasons_pinned_to_091() {
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/herdr-0.9.1-closed.json")).unwrap();
    for item in fixture.as_array().unwrap() {
        assert_eq!(
            closed_reason(item["reason"].as_str()),
            item["mapped"].as_str().unwrap()
        );
    }
    assert_eq!(closed_reason(None), "stream-closed");
    assert_eq!(closed_reason(Some("untrusted secret")), "stream-closed");
}

#[test]
fn review_local_herdr_manager_method_allowlist_tripwire() {
    // Text tripwire, not Rust name resolution; exact call-set still needs review.
    let source: String = include_str!("herdr.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let methods: std::collections::BTreeSet<_> = source
        .split("self.0.")
        .skip(1)
        .map(|call| call.split('(').next().unwrap())
        .collect();
    assert_eq!(
        methods,
        std::collections::BTreeSet::from([
            "list_sessions",
            "snapshot",
            "open_terminal",
            "events_subscribe",
            "open_native_client",
            "terminal_input",
            "terminal_scroll",
            "terminal_resize",
            "feature",
            "terminal_release",
            "events_release",
        ])
    );
}
