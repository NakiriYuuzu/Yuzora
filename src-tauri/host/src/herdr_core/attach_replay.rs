//! Replays recorded framed attach sessions (`tests/fixtures/herdr-core/v<pin>/attach-*.ndjson`):
//! framed decode, credit accounting, first paint, CLOSE reasons.
//!
//! The recordings keep the real bytes of the first 64 KiB of each stream and only the length
//! of the rest (`Recorded::frames` turns those into zeros). Nothing here asserts on bytes
//! past that point, or on where inside a long stream a repaint landed.

use serde_json::Value;

use super::fixtures::{self, Recorded};
use super::frame::{self, CloseReason, Decoder, Dir, Frame, INITIAL_CREDIT};

/// Stream channel of each `terminal.attach` reply on a connection, in order.
fn attach_channels(frames: &[(Dir, Frame)]) -> Vec<u32> {
    frames
        .iter()
        .filter(|(d, f)| *d == Dir::In && f.channel == 0 && f.kind == frame::DATA)
        .filter_map(|(_, f)| {
            serde_json::from_slice::<Value>(&f.payload)
                .ok()?
                .pointer("/result/channel")?
                .as_u64()
        })
        .map(|c| c as u32)
        .collect()
}

fn data_in(frames: &[(Dir, Frame)], ch: u32) -> Vec<u8> {
    frames
        .iter()
        .filter(|(d, f)| *d == Dir::In && f.channel == ch && f.kind == frame::DATA)
        .flat_map(|(_, f)| f.payload.clone())
        .collect()
}

fn close_in(frames: &[(Dir, Frame)], ch: u32) -> Option<CloseReason> {
    frames
        .iter()
        .find(|(d, f)| *d == Dir::In && f.channel == ch && f.kind == frame::CLOSE)
        .map(|(_, f)| CloseReason::parse(&f.payload))
}

fn conns(rec: &Recorded) -> Vec<String> {
    let mut v: Vec<String> = rec
        .lines
        .iter()
        .filter(|l| l["mode"] == "framed")
        .filter_map(|l| l["conn"].as_str().map(String::from))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Stream DATA in before / after the viewer's first CREDIT on `ch`.
fn split_at_first_credit(frames: &[(Dir, Frame)], ch: u32) -> (Vec<u8>, Vec<u8>) {
    let at = frames
        .iter()
        .position(|(d, f)| *d == Dir::Out && f.channel == ch && f.kind == frame::CREDIT)
        .unwrap_or(frames.len());
    (data_in(&frames[..at], ch), data_in(&frames[at..], ch))
}

fn without_credits(frames: &[(Dir, Frame)]) -> Vec<(Dir, Frame)> {
    frames
        .iter()
        .filter(|(d, f)| !(*d == Dir::Out && f.kind == frame::CREDIT))
        .cloned()
        .collect()
}

#[test]
fn every_recorded_stream_respects_the_credit_window_and_re_encodes() {
    let mut checked = 0;
    for name in [
        "attach-basic.ndjson",
        "attach-history.ndjson",
        "attach-observe.ndjson",
        "attach-close.ndjson",
        "attach-credit.ndjson",
        "attach-credit-only.ndjson",
        "attach-kitty.ndjson",
        "attach-kitty-reply.ndjson",
        "events-keep.ndjson",
    ] {
        let rec = fixtures::recorded(name);
        for conn in conns(&rec) {
            let frames = rec.frames(&conn);
            // Decoding the re-encoded recording in odd pieces yields the same frames.
            let wire: Vec<u8> = frames
                .iter()
                .filter(|(d, _)| *d == Dir::In)
                .flat_map(|(_, f)| f.encode())
                .collect();
            let mut decoder = Decoder::default();
            let mut back = Vec::new();
            for chunk in wire.chunks(1021) {
                back.extend(decoder.push(chunk).unwrap());
            }
            let ins: Vec<Frame> = frames
                .iter()
                .filter(|(d, _)| *d == Dir::In)
                .map(|(_, f)| f.clone())
                .collect();
            assert_eq!(back, ins, "{name}/{conn}");
            for ch in attach_channels(&frames) {
                let stats = frame::check_window(&frames, ch)
                    .unwrap_or_else(|e| panic!("{name}/{conn}/ch{ch}: {e}"));
                assert!(stats.max_outstanding <= INITIAL_CREDIT);
                checked += 1;
            }
        }
    }
    assert!(checked >= 10, "{checked} streams checked");
}

#[test]
fn first_paint_carries_the_screen_and_the_display_query_never_reaches_the_viewer() {
    let rec = fixtures::recorded("attach-basic.ndjson");
    let frames = rec.frames("V");
    let ch = attach_channels(&frames)[0];
    // First paint: stream DATA before the viewer's first typed DATA.
    let first_out = frames
        .iter()
        .position(|(d, f)| *d == Dir::Out && f.channel == ch && f.kind == frame::DATA)
        .unwrap();
    let paint: Vec<u8> = frames[..first_out]
        .iter()
        .filter(|(d, f)| *d == Dir::In && f.channel == ch && f.kind == frame::DATA)
        .flat_map(|(_, f)| f.payload.clone())
        .collect();
    assert_eq!(
        frame::count_paint_markers(&paint),
        1,
        "one paint marker (CSI ?9001l on macOS)"
    );
    // The screen text the core read back for the same terminal is inside the paint, as UTF-8.
    let note = rec
        .notes()
        .into_iter()
        .find(|n| n.get("terminal_read").is_some())
        .unwrap();
    let screen = note["terminal_read"].as_str().unwrap().trim_end();
    assert!(screen.contains("中文 繁體"), "{screen}");
    assert!(
        paint.windows(screen.len()).any(|w| w == screen.as_bytes()),
        "first paint holds {screen:?}"
    );
    // The display query was answered by the core: it never reached the viewer.
    let all = data_in(&frames, ch);
    assert!(!all.windows(6).any(|w| w == b"\x1b]11;?"));
    // Detach: our empty CLOSE is answered by an empty CLOSE.
    assert_eq!(close_in(&frames, ch), Some(CloseReason::Detached));
}

#[test]
fn close_reasons_exited_and_killed() {
    let rec = fixtures::recorded("attach-close.ndjson");
    let x = rec.frames("X");
    match close_in(&x, attach_channels(&x)[0]) {
        Some(CloseReason::Ended {
            reason, exit_code, ..
        }) => assert_eq!((reason.as_str(), exit_code), ("exited", Some(3))),
        other => panic!("{other:?}"),
    }
    let k = rec.frames("K");
    match close_in(&k, attach_channels(&k)[0]) {
        Some(CloseReason::Ended { reason, .. }) => assert_eq!(reason, "killed"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn observer_input_never_reaches_the_program() {
    let rec = fixtures::recorded("attach-observe.ndjson");
    let a = rec.frames("A");
    let stream = data_in(&a, attach_channels(&a)[0]);
    assert!(stream.windows(9).any(|w| w == b"got:typer"));
    assert!(!stream.windows(18).any(|w| w == b"got:observer-typed"));
}

#[test]
fn unread_viewer_stops_at_the_window_and_its_backlog_is_replaced_by_a_repaint() {
    // S (observe) sent no CREDIT and its socket went unread for ~1.5 s while P drained the
    // flood: the >4 MiB backlog was replaced by a repaint (review S8-R5). Where in the stream
    // that repaint landed varies between recordings and is not asserted: only lengths are.
    let rec = fixtures::recorded("attach-credit.ndjson");
    let s = rec.frames("S");
    let ch = attach_channels(&s)[0];
    let (before, after) = split_at_first_credit(&s, ch);
    assert_eq!(
        before.len() as u64,
        INITIAL_CREDIT,
        "a viewer that never credits gets exactly the initial window"
    );
    // The first paint is always in the verbatim first 64 KiB.
    assert!(frame::count_paint_markers(&before[..64 * 1024]) >= 1);
    let total = (before.len() + after.len()) as u64;
    let flood = 6u64 << 20;
    assert!(
        total < flood / 2,
        "repaint instead of the {flood}-byte backlog: got {total}"
    );
    let p = rec.frames("P");
    let pch = attach_channels(&p)[0];
    assert!(
        data_in(&p, pch).len() as u64 > flood,
        "a crediting viewer gets every byte"
    );
    // The recorded streams only fit the window because of the CREDIT frames we sent.
    for (frames, ch) in [(&p, pch), (&s, ch)] {
        assert!(
            frame::check_window(&without_credits(frames), ch).is_err(),
            "stream accepted without its credits"
        );
    }
}

#[test]
fn credit_starved_viewer_gets_a_repaint_right_after_its_late_credit() {
    // R kept reading its socket (gap <= a few ms) but sent no CREDIT through a 6 MiB flood.
    let rec = fixtures::recorded("attach-credit-only.ndjson");
    let note = rec
        .notes()
        .into_iter()
        .find_map(|n| n.get("credit_only").cloned())
        .expect("credit_only note");
    assert!(
        note["max_socket_read_gap_ms"].as_u64().unwrap() < 100,
        "{note}"
    );
    let r = rec.frames("R");
    let ch = attach_channels(&r)[0];
    let (before, after) = split_at_first_credit(&r, ch);
    assert_eq!(before.len() as u64, INITIAL_CREDIT);
    assert_eq!(
        frame::count_paint_markers(&before[..64 * 1024]),
        1,
        "only the first paint before the credit"
    );
    // The first 16 KiB after the credit were kept verbatim: they start with the repaint.
    assert_eq!(
        frame::count_paint_markers(&after[..16 * 1024]),
        1,
        "repaint after the credit"
    );
    assert!(
        ((before.len() + after.len()) as u64) < (6u64 << 20) / 2,
        "backlog dropped"
    );
    assert_eq!(note["post_credit_paint_markers"], 1);
    assert_eq!(note["screen_has_end"], true);
    assert!(frame::check_window(&without_credits(&r), ch).is_err());
}

#[test]
fn kitty_queries_are_left_to_viewers_and_only_typing_viewers_answers_reach_the_program() {
    // The core does not answer Kitty `a=q`; it forwards it. Observe viewers' DATA is dropped,
    // so their answer never reaches the program; two typing viewers answer twice (review
    // S8-R3).
    const QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q";
    const REPLY: &[u8] = b"\x1b_Gi=31;OK\x1b\\";
    const REPLY_HEX: &str = "1b5f47693d33313b4f4b1b5c";
    let rec = fixtures::recorded("attach-kitty-reply.ndjson");
    for (a, b, replies) in [("A1", "O1", 1), ("A2", "B2", 2)] {
        for v in [a, b] {
            let f = rec.frames(v);
            let ch = attach_channels(&f)[0];
            assert!(
                data_in(&f, ch).windows(QUERY.len()).any(|w| w == QUERY),
                "{v} saw the query"
            );
            let sent: Vec<u8> = f
                .iter()
                .filter(|(d, x)| *d == Dir::Out && x.channel == ch && x.kind == frame::DATA)
                .flat_map(|(_, x)| x.payload.clone())
                .collect();
            assert!(sent.ends_with(REPLY), "{v} answered");
        }
        let f = rec.frames(a);
        let out = String::from_utf8_lossy(&data_in(&f, attach_channels(&f)[0])).into_owned();
        let line = out
            .lines()
            .find(|l| l.contains("got-reply:["))
            .expect("program report");
        assert_eq!(line.matches(REPLY_HEX).count(), replies, "{a}/{b}: {line}");
    }
}
