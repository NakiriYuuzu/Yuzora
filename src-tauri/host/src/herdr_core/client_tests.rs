//! The connection layer against the fake core (`testkit`), over real local sockets.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::caps::Refusal;
use super::client::*;
use super::docs::{self, DocError, LayoutState, Op, ReadOnlyReason};
use super::events::{self, Effect};
use super::fixtures;
use super::frame::{CloseReason, INITIAL_CREDIT};
use super::testkit::{
    consume, events_from_recording, eventually, request, AttachScript, Collect, FakeCore, Recorder,
};

// ---------------------------------------------------------------------------------- helpers

fn connect(core: &FakeCore) -> (Session, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let session = Session::connect(&core.endpoint(), SessionConfig::default(), recorder.clone())
        .expect("connect to the fake core");
    (session, recorder)
}

fn layout_with_space(id: &str) -> Value {
    let mut doc = docs::LayoutDoc::empty();
    docs::apply(
        &mut doc,
        &Op::AddSpace {
            id: id.into(),
            name: id.into(),
            root: "/a".into(),
            color: None,
        },
    )
    .unwrap();
    docs::encode(&doc).unwrap()
}

// ---------------------------------------------------------------------------------- connecting

#[test]
fn session_uses_two_connections_and_never_subscribes_on_the_plain_one() {
    let core = FakeCore::start();
    let (session, _) = connect(&core);
    assert_eq!(session.hello().version, fixtures::BUNDLED);
    let stats = core.stats();
    assert_eq!((stats.plain_connections, stats.framed_connections), (1, 1));
    assert!(
        !stats.plain_subscribed,
        "events.subscribe on a plain connection silences it"
    );
    assert_eq!(
        stats
            .calls
            .iter()
            .filter(|m| *m == "events.subscribe")
            .count(),
        1
    );
    // RPC keeps answering on the plain connection while the framed one streams events.
    let list = session.rpc("terminal.list", json!({})).unwrap();
    assert_eq!(list["hidden_finished"], 0);
    // The paths that would break that, or that need framing, are refused before sending.
    for method in ["events.subscribe", "terminal.attach", "hello"] {
        assert_eq!(
            session.rpc(method, json!({})),
            Err(ClientError::Unsupported(method.into()))
        );
    }
    // So is anything the core did not declare; what it declared goes through, errors included.
    assert_eq!(
        session.rpc("no.such.method", json!({})),
        Err(ClientError::Unsupported("no.such.method".into()))
    );
    match session.rpc("machine.list", json!({})) {
        Err(ClientError::Rpc(e)) => assert_eq!(e.kind, "unknown_method"),
        other => panic!("{other:?}"),
    }
    assert!(!core.stats().calls.contains(&"no.such.method".to_string()));
}

#[test]
fn incompatible_cores_are_refused_with_the_reason() {
    let core = FakeCore::start();
    for (version, generation, refusal) in [
        (
            "0.1.5",
            1,
            Refusal::TooOld {
                version: "0.1.5".into(),
            },
        ),
        ("0.2.0", 2, Refusal::Generation { core: 2 }),
    ] {
        core.set_version(version, generation);
        let err = Session::connect(
            &core.endpoint(),
            SessionConfig::default(),
            Arc::new(Recorder::default()),
        )
        .err()
        .expect("refused");
        assert_eq!(err, ClientError::Incompatible(refusal));
        assert_eq!(err.code(), "incompatible");
    }
    // A newer core than the bundled one is used.
    core.set_version("0.2.0", 1);
    let (session, _) = connect(&core);
    assert_eq!(session.hello().version, "0.2.0");
}

#[test]
fn connecting_to_nothing_is_an_error_not_a_start() {
    let core = FakeCore::start();
    let mut endpoint = core.endpoint();
    endpoint.socket = format!("{}-nope", endpoint.socket.display()).into();
    let err = Session::connect(
        &endpoint,
        SessionConfig::default(),
        Arc::new(Recorder::default()),
    )
    .err()
    .expect("no core there");
    assert!(matches!(err, ClientError::Io(_)), "{err:?}");
}

// ---------------------------------------------------------------------------------- attach

#[test]
fn attach_replays_a_recorded_first_paint_and_carries_input_both_ways() {
    let core = FakeCore::start();
    let script = AttachScript::from_recording(&fixtures::recorded("attach-basic.ndjson"), "V");
    let expected = script.stream.clone();
    assert!(expected.len() > 100, "{} recorded bytes", expected.len());
    core.script_attach("t1", script);
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let display = DisplayFacts {
        bg: Some("#101820".into()),
        fg: Some("#e0e0e0".into()),
        cursor: Some("#ffcc00".into()),
        cell: Some([9, 18]),
    };
    let attach = session
        .attach(
            &AttachRequest {
                display: display.clone(),
                history: 100,
                ..request("t1")
            },
            sink.clone(),
        )
        .unwrap();
    assert_eq!(attach.terminal()["address"], "t1");
    consume(&attach, &sink, expected.len());
    assert_eq!(sink.bytes(), expected, "every recorded byte, in order");
    // The request carried the geometry, history and the viewer's colors.
    let params = core.stats().attach_params[0].clone();
    assert_eq!(
        (
            params["cols"].as_u64(),
            params["rows"].as_u64(),
            params["history"].as_u64()
        ),
        (Some(100), Some(30), Some(100))
    );
    assert_eq!(
        params["display"],
        json!({"bg": "#101820", "fg": "#e0e0e0", "cursor": "#ffcc00", "cell": [9, 18]})
    );

    let channel = attach.channel();
    attach.input("hello 世界\r".as_bytes()).unwrap();
    attach.resize(120, 40).unwrap();
    attach.focus(true).unwrap();
    attach.facts(&display).unwrap();
    let stats = core.wait_until("viewer frames", |s| !s.facts.is_empty());
    assert_eq!(stats.input[&channel], "hello 世界\r".as_bytes());
    assert_eq!(stats.resizes, [(channel, 120, 40)]);
    assert_eq!(stats.focus, [(channel, true)]);
    assert_eq!(stats.facts[0].1["bg"], "#101820");
    // Big pastes are split into frames, in order.
    let paste = vec![b'x'; 200 * 1024];
    attach.input(&paste).unwrap();
    core.wait_until("the paste", |s| s.input[&channel].len() >= 12 + paste.len());

    attach.detach();
    attach.detach();
    eventually("the detach answer", || sink.end().is_some());
    assert_eq!(
        sink.end(),
        Some(AttachEnd::Core {
            reason: CloseReason::Detached,
            reattach_after_restart: false
        })
    );
    assert_eq!(
        core.stats().closes_received,
        [channel],
        "one CLOSE for two detaches"
    );
    assert_eq!(attach.input(b"late"), Err(ClientError::Closed));
    assert_eq!(sink.ends.load(Ordering::SeqCst), 1);
}

#[test]
fn attach_to_an_unknown_terminal_is_the_cores_error() {
    let core = FakeCore::start();
    let (session, _) = connect(&core);
    match session.attach(&request("t99"), Arc::new(Collect::default())) {
        Err(ClientError::Rpc(e)) => assert_eq!(e.kind, "not_found"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("attached to nothing"),
    }
    assert_eq!(
        session.closed(),
        None,
        "an error answer does not end the session"
    );
}

#[test]
fn output_is_merged_in_order_before_it_reaches_the_sink() {
    let core = FakeCore::start();
    // 400 KiB in 500-byte frames: 800 frames.
    let mut script = AttachScript::flood("t1", 400 * 1024);
    script.frame = 500;
    let expected = script.stream.clone();
    core.script_attach("t1", script);
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    consume(&attach, &sink, expected.len());
    assert_eq!(sink.bytes(), expected);
    let chunks = sink.chunk_count();
    assert!(
        chunks < 800 / 2,
        "800 frames arrived as {chunks} deliveries"
    );
    let largest = sink
        .chunks
        .lock()
        .unwrap()
        .iter()
        .map(Vec::len)
        .max()
        .unwrap();
    assert!(largest <= COALESCE_MAX, "a delivery of {largest} bytes");
}

#[test]
fn a_late_pump_still_delivers_in_slices_of_at_most_the_coalesce_limit() {
    let all: Vec<u8> = (0..150 * 1024).map(|i| (i % 251) as u8).collect();
    let mut buf = all.clone();
    let mut lens = Vec::new();
    let mut joined = Vec::new();
    while !buf.is_empty() {
        let slice = super::client::take_delivery(&mut buf);
        lens.push(slice.len());
        joined.extend(slice);
    }
    assert_eq!(lens, [COALESCE_MAX, COALESCE_MAX, 22 * 1024]);
    assert_eq!(joined, all);
}

#[test]
fn a_flood_flows_inside_the_credit_window_with_batched_credit() {
    let core = FakeCore::start();
    let total = 4 << 20;
    core.script_attach("t1", AttachScript::flood("t1", total));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let channel = attach.channel();
    consume(&attach, &sink, total);
    let expected: Vec<u8> = (0..total).map(|i| b'a' + (i % 23) as u8).collect();
    assert!(sink.bytes() == expected, "bytes arrive intact and in order");
    // The leftover owed (< 32 KiB) goes out on the 50 ms timer.
    let stats = core.wait_until("all credit", |s| s.credit_total(channel) == total as u64);
    // The core obeys the credit it gets, so a client that credited bytes it never received
    // would have widened the window: it never went past the 256 KiB it started with.
    assert!(
        stats.max_window[&channel] <= INITIAL_CREDIT,
        "the core was allowed {} bytes in flight",
        stats.max_window[&channel]
    );
    let frames = stats.credits[&channel].len();
    assert!(
        frames <= total / CREDIT_BATCH as usize + 100,
        "{frames} CREDIT frames for {total} bytes"
    );
    assert!(
        stats.credits[&channel]
            .iter()
            .filter(|c| **c as u64 >= CREDIT_BATCH)
            .count()
            >= frames / 2,
        "most CREDIT frames are full batches"
    );
}

#[test]
fn without_credit_a_stream_stops_at_the_window_and_resumes_with_it() {
    let core = FakeCore::start();
    let total = 1 << 20;
    core.script_attach("t1", AttachScript::flood("t1", total));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let channel = attach.channel();
    eventually("the window", || sink.total() == INITIAL_CREDIT as usize);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        sink.total(),
        INITIAL_CREDIT as usize,
        "nothing past the window"
    );
    assert_eq!(core.stats().sent[&channel], INITIAL_CREDIT);
    consume(&attach, &sink, total);
    assert_eq!(sink.total(), total);
}

#[test]
fn credit_below_a_batch_is_sent_by_the_timer_not_at_once() {
    let core = FakeCore::start();
    // 100 bytes, then one full batch: credit can only be reported for bytes the sink got.
    let delivered = 100 + CREDIT_BATCH as usize;
    core.script_attach("t1", AttachScript::flood("t1", delivered));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let channel = attach.channel();
    eventually("the output", || sink.total() == delivered);
    let started = Instant::now();
    attach.credit(100);
    let stats = core.wait_until("the timer's CREDIT", |s| s.credits.contains_key(&channel));
    assert_eq!(stats.credits[&channel], [100]);
    assert!(
        started.elapsed() >= CREDIT_DELAY - Duration::from_millis(15),
        "sent after {:?}",
        started.elapsed()
    );
    // A full batch goes out immediately, as one frame.
    attach.credit(CREDIT_BATCH as usize);
    let stats = core.wait_until("the batch", |s| s.credits[&channel].len() == 2);
    assert_eq!(stats.credits[&channel], [100, CREDIT_BATCH as u32]);
}

#[test]
fn the_cores_close_reason_comes_after_the_last_output() {
    let core = FakeCore::start();
    let mut script = AttachScript::flood("t1", 5000);
    script.close = Some(json!({"reason": "exited", "exit_code": 3, "signal": null}));
    core.script_attach("t1", script);
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    eventually("the end", || sink.end().is_some());
    assert_eq!(sink.total(), 5000, "all output before the end");
    assert_eq!(
        sink.end(),
        Some(AttachEnd::Core {
            reason: CloseReason::Ended {
                reason: "exited".into(),
                exit_code: Some(3),
                signal: None
            },
            reattach_after_restart: false
        })
    );
    assert_eq!(attach.focus(true), Err(ClientError::Closed));
    attach.credit(5000);
}

/// Output of a sink blocks until `open` is called.
struct Gated {
    inner: Collect,
    open: (Mutex<bool>, Condvar),
}

impl AttachSink for Gated {
    fn output(&self, bytes: Vec<u8>) {
        let (gate, wake) = &self.open;
        let mut open = gate.lock().unwrap();
        while !*open {
            open = wake.wait(open).unwrap();
        }
        drop(open);
        self.inner.output(bytes);
    }

    fn closed(&self, end: AttachEnd) {
        self.inner.closed(end);
    }
}

#[test]
fn a_stuck_sink_stalls_only_its_own_stream() {
    let core = FakeCore::start();
    let total = 1 << 20;
    core.script_attach("slow", AttachScript::flood("slow", total));
    core.script_attach("fast", AttachScript::flood("fast", total));
    let (session, _) = connect(&core);
    let slow = Arc::new(Gated {
        inner: Collect::default(),
        open: (Mutex::new(false), Condvar::new()),
    });
    let slow_attach = session.attach(&request("slow"), slow.clone()).unwrap();
    let fast = Arc::new(Collect::default());
    let fast_attach = session.attach(&request("fast"), fast.clone()).unwrap();
    // The slow sink never returns, yet the other stream, RPC and events all keep going.
    consume(&fast_attach, &fast, total);
    assert_eq!(fast.total(), total);
    assert!(session.rpc("terminal.list", json!({})).is_ok());
    core.push_event(&json!({"jsonrpc": "2.0", "method": "heartbeat", "params": {"seq": 0}}));
    core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
        "seq": 1, "type": "title", "terminal": "slow", "data": {"title": "still moving"}}}));
    eventually("the event behind the heartbeat", || {
        session
            .terminals()
            .iter()
            .any(|t| t["address"] == "slow" && t["title"] == "still moving")
    });
    assert_eq!(session.closed(), None);
    assert_eq!(slow.inner.total(), 0, "the slow sink is still blocked");
    // Its stream is stopped by the window, not by the others: the sink took nothing, so the
    // client credited nothing, and the core stopped where its first window ended.
    let slow_channel = slow_attach.channel();
    core.wait_until("the slow window", |s| {
        s.sent.get(&slow_channel) == Some(&INITIAL_CREDIT)
    });
    thread::sleep(Duration::from_millis(100));
    let stats = core.stats();
    assert_eq!(stats.sent[&slow_channel], INITIAL_CREDIT);
    assert!(
        !stats.credits.contains_key(&slow_channel),
        "no credit for bytes the sink never took: {:?}",
        stats.credits.get(&slow_channel)
    );
    let (gate, wake) = &slow.open;
    *gate.lock().unwrap() = true;
    wake.notify_all();
    consume(&slow_attach, &slow.inner, total);
    assert_eq!(slow.inner.total(), total);
}

// ---------------------------------------------------------------------------------- events

#[test]
fn events_replay_rebuilds_the_cores_own_list() {
    let rec = fixtures::recorded("events-lifecycle.ndjson");
    let (snapshot, notes) = events_from_recording(&rec);
    let core = FakeCore::start();
    core.script_events(snapshot, notes);
    let (session, recorder) = connect(&core);
    let want: BTreeMap<String, Value> = rec.final_list().unwrap()["terminals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["address"].as_str().unwrap().to_string(),
                events::project(t),
            )
        })
        .collect();
    // The list is updated on the reader thread and effects reach the observer through the
    // worker thread afterwards, so wait for both.
    let seen = |pred: &dyn Fn(&Effect) -> bool| recorder.effects.lock().unwrap().iter().any(pred);
    eventually("the recorded events", || {
        let got: BTreeMap<String, Value> = session
            .terminals()
            .iter()
            .map(|t| {
                (
                    t["address"].as_str().unwrap().to_string(),
                    events::project(t),
                )
            })
            .collect();
        got == want
            && seen(&|e| matches!(e, Effect::StatusChanged(_)))
            && seen(&|e| matches!(e, Effect::DocChanged { name, .. } if name == "yuzora"))
    });
    let effects = recorder.effects.lock().unwrap().clone();
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::NeedResnapshot(_))),
        "{effects:?}"
    );
    assert_eq!(session.closed(), None);
}

#[test]
fn the_events_channel_is_credited_without_the_app() {
    let core = FakeCore::start();
    core.script_events(
        json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "title": ""}]}),
        Vec::new(),
    );
    let (session, _) = connect(&core);
    // 1500 events of 1 KiB: six times the window, and nobody calls credit.
    let events = 1500u64;
    for seq in 1..=events {
        core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
            "seq": seq, "type": "title", "terminal": "t1",
            "data": {"title": format!("{seq}:{}", "x".repeat(1000))}}}));
    }
    eventually("the last event", || {
        session.terminals().first().and_then(|t| {
            t["title"]
                .as_str()
                .map(|s| s.starts_with(&format!("{events}:")))
        }) == Some(true)
    });
    let stats = core.stats();
    let channel = *stats.sent.keys().next().expect("the events channel");
    assert!(
        stats.sent[&channel] > 6 * INITIAL_CREDIT / 2,
        "{} bytes sent",
        stats.sent[&channel]
    );
    assert!(
        stats.max_window[&channel] <= INITIAL_CREDIT,
        "the core was allowed {} bytes in flight",
        stats.max_window[&channel]
    );
    assert!(
        stats.credit_total(channel) > INITIAL_CREDIT,
        "the client credited by itself"
    );
    assert!(
        stats.credit_total(channel) <= stats.sent[&channel],
        "and never more than it was sent"
    );
}

#[test]
fn missed_replaces_the_snapshot_and_reads_documents_again() {
    let core = FakeCore::start();
    core.script_events(
        json!({"seq": 0, "terminals": [{"address": "t1", "state": "live"}]}),
        Vec::new(),
    );
    let (session, recorder) = connect(&core);
    eventually("the first read", || core.stats().doc_gets == 1);
    assert_eq!(session.terminals().len(), 1);
    core.push_event(&json!({"jsonrpc": "2.0", "method": "missed", "params": {
        "seq": 50, "terminals": [
            {"address": "t1", "state": "live", "title": "after"},
            {"address": "t2", "state": "live"}]}}));
    eventually("the new snapshot", || session.terminals().len() == 2);
    eventually("the re-read", || core.stats().doc_gets >= 2);
    assert!(recorder
        .effects
        .lock()
        .unwrap()
        .contains(&Effect::Resnapshot));
    assert_eq!(session.closed(), None);
}

#[test]
fn an_event_gap_ends_the_session_so_the_owner_reconnects() {
    let core = FakeCore::start();
    core.script_events(
        json!({"seq": 5, "terminals": [{"address": "t1", "state": "live"}]}),
        Vec::new(),
    );
    let (session, recorder) = connect(&core);
    core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
        "seq": 9, "type": "title", "terminal": "t1", "data": {"title": "x"}}}));
    eventually("the end", || session.closed().is_some());
    assert!(matches!(session.closed(), Some(ClientError::Resync(_))));
    eventually("the observer", || {
        recorder.closed.lock().unwrap().len() == 1
    });
}

// ---------------------------------------------------------------------------------- layout

#[test]
fn the_layout_is_read_on_connect_and_again_on_every_doc_event() {
    let core = FakeCore::start();
    let (session, recorder) = connect(&core);
    eventually("the first layout", || {
        recorder.layouts.lock().unwrap().len() == 1
    });
    assert_eq!(
        recorder.layouts.lock().unwrap()[0],
        LayoutSnapshot {
            version: 0,
            state: LayoutState::Empty
        }
    );
    // Another client writes: the `doc` event makes the session read it again.
    core.set_doc("yuzora", layout_with_space("s1"));
    eventually("the second layout", || {
        recorder.layouts.lock().unwrap().len() == 2
    });
    let second = recorder.layouts.lock().unwrap()[1].clone();
    assert_eq!(second.version, 1);
    assert!(matches!(&second.state, LayoutState::Doc { doc, .. } if doc.spaces[0].id == "s1"));
    assert_eq!(session.closed(), None);
}

#[test]
fn a_doc_event_for_another_document_reads_nothing_and_keeps_the_session() {
    let core = FakeCore::start();
    let (session, recorder) = connect(&core);
    eventually("the first layout", || {
        recorder.layouts.lock().unwrap().len() == 1
    });
    core.set_doc("yuzora", layout_with_space("s1"));
    eventually("the second layout", || {
        recorder.layouts.lock().unwrap().len() == 2
    });
    let reads = core.stats().doc_gets;
    // Event 1 was the write above; this is event 2, in sequence: the session must not take
    // it for a gap (that would end it), and it is not ours, so nothing is read for it.
    core.set_doc("herdr-app", json!({"theirs": true}));
    let foreign = |e: &Effect| matches!(e, Effect::DocChanged { name, .. } if name == "herdr-app");
    eventually("the foreign event", || {
        recorder.effects.lock().unwrap().iter().any(foreign)
    });
    // Our own document changes right behind it (event 3) and is read once: had the foreign
    // event been read for too, that would be a second read, as it is queued first.
    core.set_doc("yuzora", layout_with_space("s2"));
    eventually("the third layout", || {
        recorder.layouts.lock().unwrap().len() == 3
    });
    assert_eq!(session.closed(), None, "an event in sequence is not a gap");
    assert_eq!(
        core.stats().doc_gets,
        reads + 1,
        "one read, for our own document"
    );
}

#[test]
fn updating_the_layout_rebases_on_a_competing_writer() {
    let core = FakeCore::start();
    let (session, _) = connect(&core);
    let add = |id: &str| Op::AddSpace {
        id: id.into(),
        name: id.into(),
        root: "/a".into(),
        color: None,
    };
    assert_eq!(session.update_layout(&add("s1")).unwrap().attempts, 1);
    // Someone adds s2 between our read and our write.
    let (current, _) = core.doc("yuzora");
    let LayoutState::Doc { mut doc, .. } = docs::decode(&current) else {
        panic!("a layout")
    };
    docs::apply(&mut doc, &add("s2")).unwrap();
    core.competing_write_before_next_set("yuzora", docs::encode(&doc).unwrap());
    let updated = session
        .update_layout(&Op::RenameSpace {
            space: "s1".into(),
            name: "mine".into(),
        })
        .unwrap();
    assert_eq!(updated.attempts, 2);
    let (stored, version) = core.doc("yuzora");
    assert_eq!(version, updated.version);
    let LayoutState::Doc { doc, .. } = docs::decode(&stored) else {
        panic!("a layout")
    };
    let spaces: Vec<(&str, &str)> = doc
        .spaces
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str()))
        .collect();
    assert_eq!(
        spaces,
        [("s1", "mine"), ("s2", "s2")],
        "both writers' work survived"
    );
}

#[test]
fn a_newer_layout_is_reported_read_only_and_never_written() {
    let core = FakeCore::start();
    core.set_doc(
        "yuzora",
        json!({"schema": 2, "app": "yuzora", "spaces": []}),
    );
    let (session, recorder) = connect(&core);
    eventually("the layout", || {
        !recorder.layouts.lock().unwrap().is_empty()
    });
    assert_eq!(
        recorder.layouts.lock().unwrap()[0].state,
        LayoutState::ReadOnly(ReadOnlyReason::NewerSchema { schema: 2 })
    );
    let err = session
        .update_layout(&Op::AddSpace {
            id: "s1".into(),
            name: "s".into(),
            root: "/a".into(),
            color: None,
        })
        .unwrap_err();
    assert_eq!(
        err,
        DocError::ReadOnly(ReadOnlyReason::NewerSchema { schema: 2 })
    );
    assert_eq!(core.doc("yuzora").1, 1, "untouched");
}

// ---------------------------------------------------------------------------------- failure

#[test]
fn losing_the_core_fails_waiting_calls_and_ends_every_stream() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1 << 20));
    core.stay_silent_on("terminal.kill");
    let (session, recorder) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let session = Arc::new(session);
    let waiting = {
        let session = session.clone();
        thread::spawn(move || {
            session.rpc_with_timeout(
                "terminal.kill",
                json!({"terminal": "t1"}),
                Duration::from_secs(60),
            )
        })
    };
    core.wait_until("the call", |s| s.calls.iter().any(|m| m == "terminal.kill"));
    let started = Instant::now();
    core.drop_connections();
    assert_eq!(waiting.join().unwrap(), Err(ClientError::ConnectionLost));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    eventually("the stream end", || sink.end().is_some());
    assert_eq!(sink.end(), Some(AttachEnd::Session("connection_lost")));
    eventually("the observer", || {
        !recorder.closed.lock().unwrap().is_empty()
    });
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        *recorder.closed.lock().unwrap(),
        [ClientError::ConnectionLost],
        "told once"
    );
    // Later calls fail at once with the same code.
    assert_eq!(
        session.rpc("terminal.list", json!({})),
        Err(ClientError::ConnectionLost)
    );
    assert!(matches!(
        session.attach(&request("t1"), Arc::new(Collect::default())),
        Err(ClientError::ConnectionLost)
    ));
    assert_eq!(attach.input(b"x"), Err(ClientError::Closed));
}

#[test]
fn a_timed_out_call_leaves_the_session_working() {
    let core = FakeCore::start();
    core.stay_silent_on("terminal.kill");
    let (session, _) = connect(&core);
    let started = Instant::now();
    let result = session.rpc_with_timeout(
        "terminal.kill",
        json!({"terminal": "t1"}),
        Duration::from_millis(200),
    );
    assert_eq!(result, Err(ClientError::Timeout));
    assert!(started.elapsed() >= Duration::from_millis(190));
    assert!(session.rpc("terminal.list", json!({})).is_ok());
    assert_eq!(session.closed(), None);
}

// A named pipe client has no half-close: on Windows the pipes end when the session is dropped.
#[cfg(unix)]
#[test]
fn closing_a_session_hangs_up_both_connections_at_once() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1000));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let _attach = session.attach(&request("t1"), sink.clone()).unwrap();
    assert_eq!(core.stats().client_hangups, 0);
    // The session object stays alive, so nothing but `close` itself can release the sockets.
    let started = Instant::now();
    session.close();
    core.wait_until("both hang-ups", |s| s.client_hangups == 2);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the core noticed after {:?}",
        started.elapsed()
    );
    eventually("the stream end", || sink.end().is_some());
}

#[test]
fn closing_and_dropping_end_the_session_once() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1000));
    let (session, recorder) = connect(&core);
    let sink = Arc::new(Collect::default());
    let _attach = session.attach(&request("t1"), sink.clone()).unwrap();
    session.close();
    session.close();
    eventually("the observer", || {
        !recorder.closed.lock().unwrap().is_empty()
    });
    assert_eq!(session.closed(), Some(ClientError::Closed));
    assert_eq!(
        session.rpc("terminal.list", json!({})),
        Err(ClientError::Closed)
    );
    eventually("the stream end", || sink.end().is_some());
    assert_eq!(sink.end(), Some(AttachEnd::Session("closed")));
    drop(session);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(*recorder.closed.lock().unwrap(), [ClientError::Closed]);
}

// What `Session::close` promises on every platform: the session's own threads let go of its
// state at once, so the connections end as soon as the caller drops its handles. (On Unix `close`
// hangs up by itself, see above; a named pipe client can only hang up by being dropped.)
#[test]
fn a_closed_session_keeps_its_connections_only_as_long_as_the_callers_handles() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1000));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    session.close();
    eventually("the stream end", || sink.end().is_some());
    // The readers, the worker, the pump and the waiting calls are gone; two handles are left.
    eventually("the session's threads to let go", || session.holders() == 2);
    drop(attach);
    eventually("the attach's hold to go", || session.holders() == 1);
    drop(session);
    core.wait_until("both hang-ups", |s| s.client_hangups == 2);
}

// ---------------------------------------------------------------------------------- attach vs close

#[test]
fn a_session_closed_while_an_attach_answer_is_handled_still_ends_that_attach() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1000));
    let (session, recorder) = connect(&core);
    let session = Arc::new(session);
    // The answer is taken out of the table and its hook is running (on the reader thread)
    // when `close` lands: `close` finds no attach to end yet.
    session.pause_attach_hook(Duration::from_millis(400));
    let sink = Arc::new(Collect::default());
    let attaching = {
        let (session, sink) = (session.clone(), sink.clone());
        thread::spawn(move || session.attach(&request("t1"), sink))
    };
    core.wait_until("the attach request", |s| !s.attach_params.is_empty());
    thread::sleep(Duration::from_millis(100));
    session.close();
    let attach = attaching
        .join()
        .unwrap()
        .expect("the answer was in before the close");
    eventually("the observer", || {
        !recorder.closed.lock().unwrap().is_empty()
    });
    // The stream is over, for the sink and for the caller, instead of open on a dead session.
    eventually("the stream end", || sink.end().is_some());
    assert_eq!(sink.end(), Some(AttachEnd::Session("closed")));
    assert_eq!(sink.ends.load(Ordering::SeqCst), 1);
    assert_eq!(sink.total(), 0);
    assert_eq!(attach.input(b"after-close"), Err(ClientError::Closed));
    assert_eq!(attach.focus(true), Err(ClientError::Closed));
    assert!(
        core.stats().input.is_empty(),
        "nothing was written to the core after the close"
    );
    // And the attach does not keep the session alive: once the caller lets go, the reader and
    // worker threads and the pump are all that held it, and they end.
    drop(attach);
    eventually("the session's state is released", || session.holders() == 1);
}

#[test]
fn an_attach_answered_after_its_timeout_is_closed_again_at_the_core() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 1 << 20));
    // Answers five timeouts late: a starved test thread cannot make the answer look in time.
    core.delay_reply("terminal.attach", Duration::from_millis(1000));
    let config = SessionConfig {
        rpc_timeout: Duration::from_millis(200),
        ..SessionConfig::default()
    };
    let session = Session::connect(&core.endpoint(), config, Arc::new(Recorder::default()))
        .expect("connect to the fake core");
    let sink = Arc::new(Collect::default());
    let started = Instant::now();
    let result = session.attach(&request("t1"), sink.clone());
    assert_eq!(result.err(), Some(ClientError::Timeout));
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "gave up at the timeout, not at the answer: {:?}",
        started.elapsed()
    );
    // The core answers late and starts streaming into a viewer nobody owns: the session
    // releases it by itself, once.
    let stats = core.wait_until("the late viewer's CLOSE", |s| !s.closes_received.is_empty());
    let channel = stats.closes_received[0];
    thread::sleep(Duration::from_millis(100));
    let stats = core.stats();
    assert_eq!(stats.closes_received, [channel], "one CLOSE");
    assert!(stats.sent[&channel] <= INITIAL_CREDIT);
    assert_eq!(
        (sink.total(), sink.end()),
        (0, None),
        "the sink heard nothing"
    );
    assert_eq!(session.closed(), None);
    // The session works on: the next attach is a viewer like any other. (The 200 ms timeout is
    // short for a loaded machine, so a slow answer is tried again; anything but a timeout is
    // the session being broken.)
    core.delay_reply("terminal.attach", Duration::ZERO);
    let mut attached = None;
    for _ in 0..50 {
        let second = Arc::new(Collect::default());
        match session.attach(&request("t1"), second.clone()) {
            Ok(attach) => {
                attached = Some((attach, second));
                break;
            }
            Err(ClientError::Timeout) => {}
            Err(other) => panic!("the session stopped working: {other:?}"),
        }
    }
    let (attach, second) = attached.expect("an attach that was answered in time");
    assert_ne!(attach.channel(), channel);
    eventually("the second stream", || {
        second.total() == INITIAL_CREDIT as usize
    });
    assert_eq!(session.closed(), None);
}

#[test]
fn attaches_the_core_never_answers_do_not_pile_up_state_until_the_session_ends() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 10));
    core.stay_silent_on("terminal.attach");
    // Generous for the connect (its `events.subscribe` is answered on time); the attaches
    // below all wait it out at once.
    let config = SessionConfig {
        rpc_timeout: Duration::from_millis(250),
        ..SessionConfig::default()
    };
    let session = Session::connect(&core.endpoint(), config, Arc::new(Recorder::default()))
        .expect("connect to the fake core");
    let before = session.holders();
    let sinks: Vec<Arc<Collect>> = (0..20).map(|_| Arc::new(Collect::default())).collect();
    thread::scope(|scope| {
        for sink in &sinks {
            let session = &session;
            scope.spawn(move || {
                let attached = session.attach(&request("t1"), sink.clone());
                assert_eq!(attached.err(), Some(ClientError::Timeout));
            });
        }
    });
    // The requests stay registered, so a late answer can still be closed, but nothing of the
    // calls does: not the session (every thread that holds it keeps it from being released)
    // and not the caller's sinks.
    assert_eq!(
        session.holders(),
        before,
        "each timeout kept the session alive"
    );
    for sink in &sinks {
        assert_eq!(
            Arc::strong_count(sink),
            1,
            "a timed-out attach kept its sink"
        );
    }
    assert_eq!(session.late_attaches(), sinks.len());
    session.close();
    eventually("the session's state is released", || session.holders() == 1);
    assert_eq!(session.late_attaches(), 0);
}

// ---------------------------------------------------------------------------------- credit

#[test]
fn credit_reported_twice_or_beyond_what_was_delivered_cannot_widen_the_window() {
    let core = FakeCore::start();
    core.script_attach("t1", AttachScript::flood("t1", 100));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let channel = attach.channel();
    eventually("the output", || sink.total() == 100);
    // The renderer reports the same 100 bytes again and again, and then a million.
    for _ in 0..50 {
        attach.credit(100);
    }
    attach.credit(1 << 20);
    attach.credit(1 << 20);
    core.wait_until("the credit", |s| s.credit_total(channel) >= 100);
    thread::sleep(Duration::from_millis(200));
    let stats = core.stats();
    assert_eq!(
        stats.credit_total(channel),
        100,
        "credit only for the bytes the sink was handed, once"
    );
    assert!(stats.max_window[&channel] <= INITIAL_CREDIT);
}

#[test]
fn reporting_every_delivery_twice_keeps_a_stream_inside_its_window() {
    let core = FakeCore::start();
    let total = 2 << 20;
    core.script_attach("t1", AttachScript::flood("t1", total));
    let (session, _) = connect(&core);
    let sink = Arc::new(Collect::default());
    let attach = session.attach(&request("t1"), sink.clone()).unwrap();
    let channel = attach.channel();
    let mut credited = 0;
    eventually("the stream", || {
        let got = sink.total();
        if got > credited {
            attach.credit(got - credited);
            attach.credit(got - credited);
            credited = got;
        }
        got >= total
    });
    core.wait_until("all credit", |s| s.credit_total(channel) >= total as u64);
    thread::sleep(Duration::from_millis(100));
    let stats = core.stats();
    assert_eq!(stats.credit_total(channel), total as u64);
    assert!(
        stats.max_window[&channel] <= INITIAL_CREDIT,
        "the core was allowed {} bytes in flight",
        stats.max_window[&channel]
    );
}

// ---------------------------------------------------------------------------------- stream end

#[test]
fn a_streams_end_says_from_the_keep_flag_whether_the_terminal_comes_back() {
    let core = FakeCore::start();
    // (terminal, kept, close reason, comes back)
    let cases = [
        ("kept-stop", true, "server_stop", true),
        ("plain-stop", false, "server_stop", false),
        ("kept-lost", true, "holder_lost", true),
        ("plain-lost", false, "holder_lost", false),
        ("kept-signaled", true, "signaled", true),
        ("kept-exited", true, "exited", false),
    ];
    for (name, keep, reason, _) in cases {
        let mut script = AttachScript::flood(name, 10);
        script.terminal["keep"] = json!(keep);
        script.close = Some(json!({"reason": reason, "exit_code": null, "signal": null}));
        core.script_attach(name, script);
    }
    let (session, _) = connect(&core);
    for (name, _, reason, comes_back) in cases {
        let sink = Arc::new(Collect::default());
        let _attach = session.attach(&request(name), sink.clone()).unwrap();
        eventually("the end", || sink.end().is_some());
        assert_eq!(
            sink.end(),
            Some(AttachEnd::Core {
                reason: CloseReason::Ended {
                    reason: reason.into(),
                    exit_code: None,
                    signal: None
                },
                reattach_after_restart: comes_back
            }),
            "{name}"
        );
    }
}

#[test]
fn a_keep_event_before_the_close_counts_more_than_the_record_the_attach_returned() {
    let core = FakeCore::start();
    for (name, keep) in [("turned-on", false), ("turned-off", true)] {
        let mut script = AttachScript::flood(name, 10);
        script.terminal["keep"] = json!(keep);
        core.script_attach(name, script);
    }
    let (session, recorder) = connect(&core);
    let on = Arc::new(Collect::default());
    let off = Arc::new(Collect::default());
    let on_attach = session.attach(&request("turned-on"), on.clone()).unwrap();
    let off_attach = session.attach(&request("turned-off"), off.clone()).unwrap();
    for (seq, terminal, keep) in [(1, "turned-on", true), (2, "turned-off", false)] {
        core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
            "seq": seq, "type": "keep", "terminal": terminal, "data": {"keep": keep}}}));
    }
    eventually("both keep events", || {
        recorder
            .effects
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, Effect::Updated(_)))
            .count()
            == 2
    });
    let reason = json!({"reason": "server_stop", "exit_code": null, "signal": null});
    core.close_channel(on_attach.channel(), &reason);
    core.close_channel(off_attach.channel(), &reason);
    eventually("both ends", || on.end().is_some() && off.end().is_some());
    let comes_back = |sink: &Collect| match sink.end() {
        Some(AttachEnd::Core {
            reattach_after_restart,
            ..
        }) => reattach_after_restart,
        other => panic!("{other:?}"),
    };
    assert!(comes_back(&on), "kept since the event");
    assert!(!comes_back(&off), "no longer kept since the event");
}

// ---------------------------------------------------------------------------------- events

#[test]
fn title_turn_size_foreground_and_keep_reach_the_observer() {
    let core = FakeCore::start();
    core.script_events(
        json!({"seq": 0, "terminals": [{"address": "t1", "state": "live"}]}),
        Vec::new(),
    );
    let (session, recorder) = connect(&core);
    let events = [
        ("title", json!({"title": "build"})),
        ("turn", json!({"turn": 2, "state": "done"})),
        ("size", json!({"cols": 90, "rows": 25})),
        (
            "foreground",
            json!({"pid": 5, "name": "vim", "argv": ["vim"], "cwd": "/a"}),
        ),
        ("keep", json!({"keep": true})),
    ];
    for (i, (kind, data)) in events.iter().enumerate() {
        core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
            "seq": i + 1, "type": kind, "terminal": "t1", "data": data}}));
    }
    eventually("five updates", || {
        recorder.effects.lock().unwrap().len() >= events.len()
    });
    assert_eq!(
        *recorder.effects.lock().unwrap(),
        vec![Effect::Updated("t1".into()); events.len()]
    );
    // The record the observer is told to read already has them.
    let record = session.terminals().remove(0);
    assert_eq!(
        (
            record["title"].as_str(),
            record["cols"].as_u64(),
            record["keep"].as_bool()
        ),
        (Some("build"), Some(90), Some(true))
    );
    assert_eq!(session.closed(), None);
}

#[test]
fn a_labels_change_updates_the_store_and_reaches_the_observer() {
    let core = FakeCore::start();
    core.script_events(
        json!({"seq": 0, "terminals": [{"address": "t1", "state": "live", "labels": {}}]}),
        Vec::new(),
    );
    let (session, recorder) = connect(&core);
    core.push_event(&json!({"jsonrpc": "2.0", "method": "event", "params": {
        "seq": 1, "type": "labels", "terminal": "t1", "data": {"labels": {"space": "a"}}}}));
    eventually("the labels update", || {
        !recorder.effects.lock().unwrap().is_empty()
    });
    assert_eq!(
        *recorder.effects.lock().unwrap(),
        vec![Effect::Updated("t1".into())]
    );
    // The record the observer is told to read already has them.
    assert_eq!(session.terminals()[0]["labels"], json!({"space": "a"}));
    assert_eq!(session.closed(), None);
}

// ---------------------------------------------------------------------------------- rpc deny-list

#[test]
fn methods_that_stop_the_core_run_programs_or_touch_files_are_never_sent() {
    let core = FakeCore::start();
    // Families upstream may grow: declared by a newer core, they are still out.
    core.declare("file.delete");
    core.declare("machine.future");
    let (session, _) = connect(&core);
    let denied = [
        "server.stop",
        "server.restart",
        "machine.exec",
        "file.read",
        "file.write",
        "file.edit",
        // The user's own machine list: dialling out over ssh, renaming, removing.
        "machine.add",
        "machine.connect",
        "machine.rename",
        "machine.rm",
        "file.delete",
        "machine.future",
    ];
    // The core declares every one of them, so it is the deny-list that refuses them, not a
    // missing declaration.
    for method in denied {
        assert!(session.hello().has(method), "{method} is declared");
    }
    for method in denied {
        let err = session.rpc(method, json!({})).unwrap_err();
        assert_eq!(err, ClientError::Unsupported(method.into()), "{method}");
        assert_eq!(err.code(), "unsupported");
        assert_eq!(
            session.rpc_with_timeout(method, json!({}), Duration::from_secs(1)),
            Err(ClientError::Unsupported(method.into())),
            "{method}, with a timeout"
        );
    }
    let calls = core.stats().calls;
    assert!(
        !calls.iter().any(|c| denied.contains(&c.as_str())),
        "none of them was sent: {calls:?}"
    );
    // What a viewer needs is untouched, and so are the session's own document calls.
    assert!(session.rpc("terminal.list", json!({})).is_ok());
    assert!(session.rpc("server.status", json!({})).is_ok());
    assert_eq!(session.read_layout().unwrap().version, 0);
    assert!(session
        .update_layout(&Op::AddSpace {
            id: "s1".into(),
            name: "s1".into(),
            root: "/a".into(),
            color: None,
        })
        .is_ok());
    // Reading the machine list is allowed: the fake core does not implement it, which is
    // what proves the request went out.
    match session.rpc("machine.list", json!({})) {
        Err(ClientError::Rpc(e)) => assert_eq!(e.kind, "unknown_method"),
        other => panic!("{other:?}"),
    }
    assert!(core.stats().calls.contains(&"machine.list".to_string()));
}

#[test]
fn a_request_that_names_another_machine_is_never_sent() {
    let core = FakeCore::start();
    let (session, _) = connect(&core);
    let named = [
        ("terminal.run", json!({"on": "box", "cwd": "/a"})),
        ("doc.get", json!({"name": "yuzora", "on": "box"})),
        (
            "doc.set",
            json!({"name": "yuzora", "value": {}, "version": 0, "on": "box"}),
        ),
        ("terminal.list", json!({"on": "box"})),
        // However it is spelled, a request that says `on` is not about this core alone.
        ("doc.get", json!({"name": "yuzora", "on": null})),
    ];
    for (method, params) in &named {
        assert!(session.hello().has(method), "{method} is declared");
        assert_eq!(
            session.rpc(method, params.clone()),
            Err(ClientError::Unsupported((*method).into())),
            "{method} {params}"
        );
        assert_eq!(
            session.rpc_with_timeout(method, params.clone(), Duration::from_secs(1)),
            Err(ClientError::Unsupported((*method).into())),
            "{method} {params}, with a timeout"
        );
    }
    // (`doc.get` is left out: the session itself reads the layout document after connecting.)
    let calls = core.stats().calls;
    assert!(
        !calls
            .iter()
            .any(|c| ["terminal.run", "doc.set", "terminal.list"].contains(&c.as_str())),
        "none of them was sent: {calls:?}"
    );
    // The same methods without `on` are what the app sends, and they go through. (The fake
    // core has no `terminal.run`: its error is what shows the request was made.)
    assert!(session
        .rpc(
            "terminal.run",
            json!({"cwd": "/a", "env": {"TERM": "xterm-256color"}})
        )
        .is_err_and(|e| matches!(e, ClientError::Rpc(ref rpc) if rpc.kind == "unknown_method")));
    assert!(session.rpc("doc.get", json!({"name": "yuzora"})).is_ok());
    assert!(session.rpc("terminal.list", json!({"all": true})).is_ok());
    assert!(core.stats().calls.contains(&"terminal.run".to_string()));
}

// ---------------------------------------------------------------------------------- layout json

#[test]
fn a_layout_snapshot_serializes_with_one_kind_and_a_separate_reason() {
    let cases = [
        (
            LayoutState::ReadOnly(ReadOnlyReason::NewerSchema { schema: 2 }),
            r#"{"version":3,"state":{"kind":"readOnly","reason":"newerSchema","schema":2}}"#,
        ),
        (
            LayoutState::ReadOnly(ReadOnlyReason::Invalid {
                message: "no schema".into(),
            }),
            r#"{"version":3,"state":{"kind":"readOnly","reason":"invalid","message":"no schema"}}"#,
        ),
        (
            LayoutState::Empty,
            r#"{"version":3,"state":{"kind":"empty"}}"#,
        ),
    ];
    for (state, text) in cases {
        let snapshot = LayoutSnapshot { version: 3, state };
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(serialized, text);
        assert_eq!(serialized.matches("\"kind\"").count(), 1, "{serialized}");
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            serde_json::from_str::<Value>(text).unwrap()
        );
    }
}
