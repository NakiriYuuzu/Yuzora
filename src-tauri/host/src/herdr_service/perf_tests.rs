//! `#[ignore]` micro-benchmarks for the HERDR hot paths.
//! Run: cargo test --release --lib perf_ -- --ignored --nocapture
use super::*;
use std::time::Instant;

fn big_snapshot_line() -> String {
    let panes: Vec<_> = (0..160)
        .map(|i| {
            serde_json::json!({
                "pane_id": format!("w{}:p{i}", i % 8), "terminal_id": format!("term_{i}"),
                "workspace_id": format!("ws_{}", i % 8), "tab_id": format!("tab_{}", i % 40),
                "focused": i == 3, "agent_status": "idle", "revision": i, "title": format!("Shell {i}"),
                "cwd": format!("/Users/someone/projects/repo-{i}/src/deeply/nested"),
                "agent": "pi", "scroll": {"offset": 0, "max": 100}
            })
        })
        .collect();
    let tabs: Vec<_> = (0..40)
        .map(|i| serde_json::json!({"tab_id": format!("tab_{i}"), "workspace_id": format!("ws_{}", i % 8), "label": format!("Tab {i}"), "pane_count": 4, "agent_status": "idle"}))
        .collect();
    let agents: Vec<_> = (0..100)
        .map(|i| serde_json::json!({"pane_id": format!("w{}:p{i}", i % 8), "agent": "pi", "status": "working", "title": format!("Agent {i}")}))
        .collect();
    let layouts: Vec<_> = (0..40)
        .map(|i| serde_json::json!({"tab_id": format!("tab_{i}"), "root": {"type": "split", "ratio": 0.5, "first": {"type": "pane", "pane_id": "p1"}, "second": {"type": "pane", "pane_id": "p2"}}}))
        .collect();
    serde_json::json!({
        "id": "yuzora:herdr:1",
        "result": {"type": "session_snapshot", "snapshot": {
            "version": "0.9.1", "protocol": 22,
            "workspaces": (0..8).map(|i| serde_json::json!({"workspace_id": format!("ws_{i}"), "label": format!("Space {i}")})).collect::<Vec<_>>(),
            "tabs": tabs, "panes": panes, "layouts": layouts, "agents": agents
        }}
    })
    .to_string()
}

#[test]
#[ignore]
fn perf_snapshot_pipeline() {
    let line = big_snapshot_line();
    let iterations = 2000;
    let started = Instant::now();
    for _ in 0..iterations {
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let value = validate_api_response_sized(value, line.len()).unwrap();
        let result = bounded_ipc(parse_snapshot_response(value).unwrap()).unwrap();
        std::hint::black_box(result);
    }
    let per = started.elapsed().as_secs_f64() * 1e6 / iterations as f64;
    println!(
        "PERF snapshot_pipeline bytes={} us/snapshot={per:.1}",
        line.len()
    );
}

fn frame_lines(count: usize, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(count * (payload.len() + 96));
    for seq in 1..=count {
        out.extend_from_slice(
            format!(
                "{{\"type\":\"terminal.frame\",\"seq\":{seq},\"full\":{},\"encoding\":\"ansi\",\"width\":120,\"height\":40,\"bytes\":\"{payload}\"}}\n",
                seq == 1
            )
            .as_bytes(),
        );
    }
    out
}

fn run_reader(input: Vec<u8>, frames: usize) -> f64 {
    let session = Arc::new(ConnectorSession {
        id: "perf".into(),
        mode: HerdrTerminalMode::Observe,
        cols: Mutex::new(120),
        rows: Mutex::new(40),
        child: Mutex::new(None),
        process_tree: Mutex::new(None),
        stdin: Mutex::new(None),
        reader: Mutex::new(None),
        closed: Mutex::new(false),
    });
    let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = emitted.clone();
    let on_event: OnTerminalEvent = Arc::new(move |event| {
        if matches!(event, HerdrTerminalEvent::Frame { .. }) {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    });
    let started = Instant::now();
    connector_reader_loop(
        session,
        std::io::Cursor::new(input),
        None::<std::io::Cursor<Vec<u8>>>,
        on_event,
    );
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(emitted.load(std::sync::atomic::Ordering::Relaxed), frames);
    secs
}

#[test]
#[ignore]
fn perf_connector_frame_reader() {
    for (label, payload, count) in [
        ("64KiB-base64", "QUJD".repeat(16 * 1024), 1500usize),
        ("tiny-4B", "AAA=".to_string(), 100_000usize),
    ] {
        let input = frame_lines(count, &payload);
        let mb = input.len() as f64 / (1024.0 * 1024.0);
        let secs = run_reader(input, count);
        println!(
            "PERF connector_reader {label}: frames={count} MB/s={:.1} us/frame={:.2}",
            mb / secs,
            secs * 1e6 / count as f64
        );
    }
}
