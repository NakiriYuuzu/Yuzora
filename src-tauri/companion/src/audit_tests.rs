use super::*;
use crate::test_support::Fixture;

fn audit(f: &Fixture, rejections: Arc<Mutex<Rejections>>) -> Audit {
    Audit::new(
        &f.gateway,
        "device",
        7,
        Scope::Control,
        "work".into(),
        Some("w:p".into()),
        rejections,
    )
}

#[tokio::test(start_paused = true)]
async fn review_rejected_flood_never_occupies_tightening_deferred_bundles() {
    let f = Fixture::new();
    while f.events.try_emit(Event::error("filler")) {}
    let rejections = Arc::new(Mutex::new(Rejections::default()));
    for _ in 0..1000 {
        audit(&f, rejections.clone()).rejected();
    }
    {
        let deferred = f.gateway.deferred_audit.lock().unwrap();
        assert!(deferred.pending.is_empty());
        let counts = deferred.dropped.unwrap();
        assert_eq!(counts.rejected_omitted, 1000);
        assert_eq!(counts.lost, 0);
        assert_eq!(counts.lost_tightening, 0);
    }
    for _ in 0..64 {
        audit(&f, rejections.clone()).tightened("session-invalid", true);
    }
    // A sixty-fifth stream cannot evict a retained tightening bundle.
    audit(&f, rejections).tightened("session-invalid", true);
    let deferred = f.gateway.deferred_audit.lock().unwrap();
    assert_eq!(deferred.pending.len(), 64);
    let counts = deferred.dropped.unwrap();
    assert_eq!(counts.lost, 2);
    assert_eq!(counts.lost_tightening, 2);
    assert_eq!(counts.rejected_omitted, 1000);
    assert!(deferred.pending_for("device"));
    assert!(deferred
        .pending
        .values()
        .all(|(_, bundle)| bundle.len() == 2));
}

#[tokio::test(start_paused = true)]
async fn review_rejected_summaries_are_count_only_per_peer_and_rate_limited() {
    let mut f = Fixture::new();
    let rejections = Arc::new(Mutex::new(Rejections::default()));
    for _ in 0..100 {
        audit(&f, rejections.clone()).rejected();
    }
    let first = f.event().await;
    assert_eq!(first["action"], "write.rejected");
    assert_eq!(first["summary"], json!({"count":1}));
    assert_eq!(rejections.lock().unwrap().count, 99);
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    audit(&f, rejections).rejected();
    let second = f.event().await;
    assert_eq!(second["peerId"], 7);
    assert_eq!(second["summary"], json!({"count":100}));
    assert!(f.gateway.deferred_audit.lock().unwrap().pending.is_empty());
}

#[tokio::test]
async fn review_bucket_global_cap_and_lifecycle_priority_headroom() {
    let mut f = Fixture::new();
    let reservations: Vec<_> = (0..12)
        .map(|_| f.events.reserve_bucket().unwrap())
        .collect();
    assert!(
        f.events.reserve_bucket().is_none(),
        "global bucket reservation cap must be 12"
    );
    let announcements: Vec<_> = (0..4)
        .map(|_| f.events.reserve_announcement().unwrap())
        .collect();
    assert!(f.events.reserve_announcement().is_none());
    let audit = audit(&f, Arc::new(Mutex::new(Rejections::default())));
    for _ in 0..32 {
        assert!(audit.admit("terminal.input", json!({"bytes":1})).is_ok());
    }
    assert_eq!(
        audit.admit("terminal.input", json!({"bytes":1})),
        Err("audit-unavailable")
    );
    // Ordinary audits cannot consume any of the final sixteen lifecycle slots.
    for _ in 0..4 {
        assert!(f
            .events
            .try_emit(Event::pairing_state("pair".into(), "rejected", "rejected")));
        assert!(f.events.try_emit(Event::device_revoked("device".into())));
        assert!(f
            .events
            .try_emit(Event::session_closed(7, "device".into(), "session-invalid")));
        assert!(f.events.try_emit(audit.state(false, false)));
    }
    assert!(!f.events.try_emit(Event::error("full")));
    drop(reservations);
    drop(announcements);
    for _ in 0..48 {
        f.event().await;
    }
    assert!(
        f.events.reserve_bucket().is_some(),
        "bucket and queue permits must return"
    );
}

#[tokio::test(start_paused = true)]
async fn final_n2_dropped_separates_actual_loss_tightening_and_omitted_rejections() {
    let mut f = Fixture::new();
    let audit = audit(&f, Arc::new(Mutex::new(Rejections::default())));
    for _ in 0..3 {
        audit.rejected();
    }
    assert_eq!(f.event().await["action"], "write.rejected");
    f.gateway.deferred_audit.lock().unwrap().flush(&f.events);
    let counts = f.event().await;
    assert_eq!(counts["type"], "audit.dropped");
    assert_eq!(counts["lost"], 0, "intentional omission is not actual loss");
    assert_eq!(counts["lostTightening"], 0);
    assert_eq!(counts["rejectedOmitted"], 3);
    assert!(counts.get("count").is_none());

    let mut filled = 0;
    while f.events.try_emit(Event::error("filler")) {
        filled += 1;
    }
    audit.stalled("write");
    audit.cancelled(false, "dropped");
    // Replacing a retained tightening bundle loses exactly its two events.
    audit.tightened("session-invalid", true);
    audit.tightened("session-invalid", true);
    for _ in 0..filled {
        f.event().await;
    }
    f.gateway.deferred_audit.lock().unwrap().flush(&f.events);
    assert_eq!(f.event().await["action"], "revoke");
    assert_eq!(f.event().await["open"], false);
    let counts = f.event().await;
    assert_eq!(counts["lost"], 4);
    assert_eq!(counts["lostTightening"], 2);
    assert_eq!(
        counts["rejectedOmitted"], 0,
        "successful flush resets all counters"
    );
    assert!(counts["since"].is_string());
    assert!(f.gateway.deferred_audit.lock().unwrap().dropped.is_none());
}
