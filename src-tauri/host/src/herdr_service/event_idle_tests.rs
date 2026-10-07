use super::{
    write_fake_herdr_event_session, HerdrManager, HerdrSubscriptionEvent, OnSubscriptionEvent,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

struct Fixture {
    _directory: tempfile::TempDir,
    listener: UnixListener,
    manager: Arc<HerdrManager>,
}

struct Subscription {
    manager: Arc<HerdrManager>,
    id: Option<String>,
    peer: UnixStream,
    events: mpsc::Receiver<HerdrSubscriptionEvent>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("events.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let binary = write_fake_herdr_event_session(directory.path(), &socket);
        Self {
            _directory: directory,
            listener,
            manager: Arc::new(HerdrManager::with_binary(binary)),
        }
    }

    fn subscribe(&self) -> Subscription {
        let listener = self.listener.try_clone().unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut peer = loop {
                match listener.accept() {
                    Ok((peer, _)) => break peer,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "owned accept timed out");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("owned accept failed: {error}"),
                }
            };
            peer.set_nonblocking(false).unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            peer.set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(peer.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["method"], "events.subscribe");
            peer.write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
                .unwrap();
            peer
        });
        let (send, events) = mpsc::channel();
        let callback: OnSubscriptionEvent =
            Arc::new(move |event| send.send(event).map_err(|error| error.to_string()));
        let result = self
            .manager
            .events_subscribe(Some("default".into()), vec![], callback);
        let peer = server.join().unwrap();
        let id = result.unwrap();
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            HerdrSubscriptionEvent::Subscribed { .. }
        ));
        Subscription {
            manager: Arc::clone(&self.manager),
            id: Some(id),
            peer,
            events,
        }
    }
}

impl Subscription {
    fn finish_without_release(mut self, kind: &str) -> String {
        let id = self.id.take().unwrap();
        match kind {
            "eof" => {
                self.peer.shutdown(Shutdown::Write).unwrap();
                assert!(matches!(
                    self.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                    HerdrSubscriptionEvent::Disconnected { .. }
                ));
            }
            "protocol" => {
                self.peer.write_all(b"not JSON\n").unwrap();
                assert!(matches!(
                    self.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                    HerdrSubscriptionEvent::Error { .. }
                ));
            }
            "callback" => {
                let (_sender, replacement) = mpsc::channel();
                drop(std::mem::replace(&mut self.events, replacement));
                self.peer.write_all(&event_line("unobserved", "")).unwrap();
            }
            _ => panic!("unknown terminal kind"),
        }
        let mut byte = [0];
        assert_eq!(self.peer.read(&mut byte).unwrap(), 0);
        id
    }

    fn receive_topology(&self, expected: &str) {
        match self.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            HerdrSubscriptionEvent::TopologyChanged {
                workspace_id, kind, ..
            } => {
                assert_eq!(kind, "workspace.renamed");
                assert_eq!(workspace_id.as_deref(), Some(expected));
            }
            event => panic!("unexpected event: {event:?}"),
        }
    }

    fn release(mut self) -> Duration {
        let id = self.id.take().unwrap();
        let started = Instant::now();
        self.manager.events_release(&id).unwrap();
        let elapsed = started.elapsed();
        assert!(
            self.events.try_recv().is_err(),
            "manual release emitted an event"
        );
        let mut byte = [0];
        assert_eq!(self.peer.read(&mut byte).unwrap(), 0);
        elapsed
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.manager.events_release(&id);
        }
    }
}

fn event_line(id: &str, padding: &str) -> Vec<u8> {
    let mut line = serde_json::to_vec(&serde_json::json!({
        "event": "workspace.renamed", "data": {"workspace_id": id}, "padding": padding,
    }))
    .unwrap();
    line.push(b'\n');
    line
}

#[test]
fn owned_manual_release_does_not_emit_disconnect() {
    let fixture = Fixture::new();
    let mut subscription = fixture.subscribe();
    subscription
        .peer
        .write_all(&event_line("ready", ""))
        .unwrap();
    subscription.receive_topology("ready");
    std::thread::sleep(Duration::from_millis(3));
    assert!(subscription.release() < Duration::from_secs(1));
    assert!(fixture
        .manager
        .event_subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn owned_partial_release_keeps_sibling_event_stream_live() {
    let fixture = Fixture::new();
    let mut partial = fixture.subscribe();
    let mut sibling = fixture.subscribe();
    partial
        .peer
        .write_all(b"{\"event\":\"workspace.renamed\"")
        .unwrap();
    sibling
        .peer
        .write_all(&event_line("before", "漢字🙂"))
        .unwrap();
    sibling.receive_topology("before");
    partial.release();
    sibling.peer.write_all(&event_line("after", "")).unwrap();
    sibling.receive_topology("after");
    sibling.release();
    assert!(fixture
        .manager
        .event_subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn owned_peer_eof_releases_socket_before_frontend_release() {
    let fixture = Fixture::new();
    let mut subscription = fixture.subscribe();
    subscription.peer.shutdown(Shutdown::Write).unwrap();
    assert!(matches!(
        subscription
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        HerdrSubscriptionEvent::Disconnected { .. }
    ));
    let mut byte = [0];
    assert_eq!(
        subscription.peer.read(&mut byte).unwrap(),
        0,
        "finished reader retained its socket"
    );
    subscription.release();
}

#[test]
fn owned_protocol_error_releases_socket_before_frontend_release() {
    let fixture = Fixture::new();
    let mut subscription = fixture.subscribe();
    subscription.peer.write_all(b"not JSON\n").unwrap();
    assert!(matches!(
        subscription
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        HerdrSubscriptionEvent::Error { .. }
    ));
    let mut byte = [0];
    assert_eq!(
        subscription.peer.read(&mut byte).unwrap(),
        0,
        "failed reader retained its socket"
    );
    subscription.release();
}

#[test]
fn owned_terminal_records_retire_without_disturbing_sibling() {
    let fixture = Fixture::new();
    let mut sibling = fixture.subscribe();
    for kind in ["eof", "protocol", "callback"] {
        for _ in 0..3 {
            let id = fixture.subscribe().finish_without_release(kind);
            let deadline = Instant::now() + Duration::from_secs(5);
            while fixture
                .manager
                .event_subscriptions
                .lock()
                .unwrap()
                .contains_key(&id)
            {
                assert!(Instant::now() < deadline, "terminal {kind} retained {id}");
                std::thread::sleep(Duration::from_millis(1));
            }
            {
                let records = fixture.manager.event_subscriptions.lock().unwrap();
                assert!(!records.contains_key(&id), "terminal {kind} retained {id}");
                assert_eq!(records.len(), 1);
                assert!(records.contains_key(sibling.id.as_ref().unwrap()));
            }
            fixture.manager.events_release(&id).unwrap();
            sibling
                .peer
                .write_all(&event_line("still-live", ""))
                .unwrap();
            sibling.receive_topology("still-live");
        }
    }
    sibling.release();
    assert!(fixture
        .manager
        .event_subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn owned_unknown_event_does_not_retire_live_subscription() {
    let fixture = Fixture::new();
    let mut subscription = fixture.subscribe();
    subscription
        .peer
        .write_all(b"{\"event\":\"unknown.future.event\"}\n")
        .unwrap();
    subscription
        .peer
        .write_all(&event_line("still-live", ""))
        .unwrap();
    subscription.receive_topology("still-live");
    assert!(fixture
        .manager
        .event_subscriptions
        .lock()
        .unwrap()
        .contains_key(subscription.id.as_ref().unwrap()));
    subscription.release();
    assert!(fixture
        .manager
        .event_subscriptions
        .lock()
        .unwrap()
        .is_empty());
}

#[test]
fn owned_terminal_cleanup_can_race_explicit_release() {
    let fixture = Fixture::new();
    for _ in 0..20 {
        let mut subscription = fixture.subscribe();
        let id = subscription.id.take().unwrap();
        let manager = Arc::clone(&fixture.manager);
        let start = Arc::new(std::sync::Barrier::new(2));
        let release_start = Arc::clone(&start);
        let release = std::thread::spawn(move || {
            release_start.wait();
            manager.events_release(&id).unwrap();
        });
        start.wait();
        let _ = subscription.peer.shutdown(Shutdown::Write);
        release.join().unwrap();
        let mut byte = [0];
        assert_eq!(subscription.peer.read(&mut byte).unwrap(), 0);
        let mut disconnected = 0;
        loop {
            match subscription.events.try_recv() {
                Ok(HerdrSubscriptionEvent::Disconnected { .. }) => disconnected += 1,
                Err(mpsc::TryRecvError::Disconnected) => break,
                other => panic!("unexpected result after reader release: {other:?}"),
            }
        }
        assert!(disconnected <= 1);
        assert!(fixture
            .manager
            .event_subscriptions
            .lock()
            .unwrap()
            .is_empty());
    }
}

#[cfg(target_os = "macos")]
struct Usage {
    cpu_ms: f64,
    switches: i64,
    interrupt_wakeups: u64,
    rss: u64,
    footprint: u64,
}

#[cfg(target_os = "macos")]
fn usage() -> Usage {
    // SAFETY: both system calls receive correctly sized writable records. Fields
    // are read only after success has confirmed that each record is initialized.
    let (cpu, task) = unsafe {
        let mut cpu = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, cpu.as_mut_ptr()), 0);
        let mut task = std::mem::MaybeUninit::<libc::rusage_info_v0>::uninit();
        assert_eq!(
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V0,
                task.as_mut_ptr().cast()
            ),
            0
        );
        (cpu.assume_init(), task.assume_init())
    };
    Usage {
        cpu_ms: (cpu.ru_utime.tv_sec + cpu.ru_stime.tv_sec) as f64 * 1000.0
            + (cpu.ru_utime.tv_usec + cpu.ru_stime.tv_usec) as f64 / 1000.0,
        switches: cpu.ru_nvcsw,
        interrupt_wakeups: task.ri_interrupt_wkups,
        rss: task.ri_resident_size,
        footprint: task.ri_phys_footprint,
    }
}

#[cfg(target_os = "macos")]
fn resources(fixture: &Fixture) -> serde_json::Value {
    let value = usage();
    serde_json::json!({ "rss": value.rss, "footprint": value.footprint, "fds": std::fs::read_dir("/dev/fd").unwrap().count(), "subscriptions": fixture.manager.event_subscriptions.lock().unwrap().len() })
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "manual release-profile owned HERDR event socket measurement"]
fn performance_owned_event_subscription_lifecycle() {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "Use --release"
    );
    let smoke = std::env::var_os("YUZORA_EVENT_IDLE_SMOKE").is_some();
    let fixture = Fixture::new();
    let counts: &[usize] = if smoke { &[1] } else { &[0, 1, 8] };
    for &count in counts {
        let subscriptions: Vec<_> = (0..count).map(|_| fixture.subscribe()).collect();
        for sample in 0..if smoke { 3 } else { 9 } {
            let before = usage();
            let started = Instant::now();
            std::thread::sleep(Duration::from_millis(if smoke { 350 } else { 500 }));
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            let after = usage();
            for subscription in &subscriptions {
                assert!(subscription.events.try_recv().is_err());
            }
            println!(
                "HERDR_IDLE {}",
                serde_json::json!({ "count": count, "sample": sample, "warmup": sample < 2, "wallMs": wall_ms, "cpuMs": after.cpu_ms - before.cpu_ms, "voluntarySwitches": after.switches - before.switches, "interruptWakeups": after.interrupt_wakeups - before.interrupt_wakeups, "resources": resources(&fixture) })
            );
        }
        for subscription in subscriptions {
            subscription.release();
        }
        assert!(fixture
            .manager
            .event_subscriptions
            .lock()
            .unwrap()
            .is_empty());
    }
    if smoke {
        return;
    }

    for scenario in ["single", "unicode", "fragmented", "burst32"] {
        let mut subscription = fixture.subscribe();
        let width = if scenario == "burst32" { 32 } else { 1 };
        let padding = if scenario == "unicode" {
            "漢字🙂".repeat(64)
        } else {
            String::new()
        };
        let messages: Vec<_> = (0..width)
            .map(|i| event_line(&format!("space-{i}"), &padding))
            .collect();
        let payload: Vec<_> = messages.iter().flatten().copied().collect();
        for batch in 0..10 {
            let before = usage();
            let mut latencies = Vec::new();
            for _ in 0..64 {
                let started = Instant::now();
                if scenario == "fragmented" {
                    let middle = payload.len() / 2;
                    subscription.peer.write_all(&payload[..middle]).unwrap();
                    subscription.peer.write_all(&payload[middle..]).unwrap();
                } else {
                    subscription.peer.write_all(&payload).unwrap();
                }
                for i in 0..width {
                    subscription.receive_topology(&format!("space-{i}"));
                }
                latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            let after = usage();
            println!(
                "HERDR_EVENT_LOAD {}",
                serde_json::json!({ "scenario": scenario, "batch": batch, "warmup": batch < 3, "iterations": 64, "eventsPerIteration": width, "cpuMs": after.cpu_ms - before.cpu_ms, "latencyMs": latencies })
            );
        }
        subscription.release();
    }

    let mut latencies = Vec::new();
    let mut samples = Vec::new();
    for cycle in 0..110 {
        let mut subscription = fixture.subscribe();
        subscription
            .peer
            .write_all(&event_line("ready", ""))
            .unwrap();
        subscription.receive_topology("ready");
        std::thread::sleep(Duration::from_millis(3));
        let elapsed = subscription.release();
        assert!(fixture
            .manager
            .event_subscriptions
            .lock()
            .unwrap()
            .is_empty());
        if cycle >= 10 {
            latencies.push(elapsed.as_secs_f64() * 1000.0);
        }
        if cycle >= 10 && (cycle + 1) % 10 == 0 {
            samples.push(serde_json::json!({"cycle": cycle - 9, "resources": resources(&fixture)}));
        }
    }
    println!(
        "HERDR_EVENT_LIFECYCLE {}",
        serde_json::json!({ "warmup": 10, "cycles": 100, "releaseLatencyMs": latencies, "samples": samples })
    );
}

#[test]
fn owned_callback_failure_releases_socket_before_frontend_release() {
    let fixture = Fixture::new();
    let mut subscription = fixture.subscribe();
    let (_send, replacement) = mpsc::channel();
    drop(std::mem::replace(&mut subscription.events, replacement));
    subscription
        .peer
        .write_all(&event_line("unobserved", ""))
        .unwrap();
    let mut byte = [0];
    assert_eq!(subscription.peer.read(&mut byte).unwrap(), 0);
    subscription.release();
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "manual extended release-profile HERDR socket resource soak"]
fn performance_owned_event_subscription_soak() {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "Use --release"
    );
    let fixture = Fixture::new();
    // Allocate and touch measurement storage before warmup. Recording a sample
    // must not grow a retained JSON/latency collection during the resource run.
    let mut latencies = vec![f64::NAN; 1000];
    std::hint::black_box(&mut latencies);
    let mut samples = [(0_usize, 0_u64, 0_u64, 0_usize, 0_usize); 11];
    let mut recorded = 0;
    for cycle in 0..1100 {
        let mut subscription = fixture.subscribe();
        subscription
            .peer
            .write_all(&event_line("ready", ""))
            .unwrap();
        subscription.receive_topology("ready");
        std::thread::sleep(Duration::from_millis(3));
        let elapsed = subscription.release();
        assert!(fixture
            .manager
            .event_subscriptions
            .lock()
            .unwrap()
            .is_empty());
        if cycle >= 100 {
            latencies[cycle - 100] = elapsed.as_secs_f64() * 1000.0;
        }
        if cycle + 1 >= 100 && (cycle + 1 - 100) % 100 == 0 {
            let value = usage();
            samples[recorded] = (
                cycle + 1 - 100,
                value.rss,
                value.footprint,
                std::fs::read_dir("/dev/fd").unwrap().count(),
                fixture.manager.event_subscriptions.lock().unwrap().len(),
            );
            recorded += 1;
        }
    }
    assert_eq!(recorded, samples.len());
    assert!(samples
        .iter()
        .all(|sample| sample.3 == samples[0].3 && sample.4 == 0));
    println!(
        "HERDR_EVENT_SOAK {}",
        serde_json::json!({ "warmup": 100, "cycles": 1000, "releaseLatencyMs": latencies, "samples": samples.map(|(cycle, rss, footprint, fds, subscriptions)| serde_json::json!({ "cycle": cycle, "rss": rss, "footprint": footprint, "fds": fds, "subscriptions": subscriptions })) })
    );
}
