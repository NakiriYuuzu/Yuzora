//! Integration test against a real `herdr-core` binary. Ignored by default:
//!
//! ```text
//! YUZORA_HC_TEST_BIN=/path/to/herdr-core \
//!   cargo test -p yuzora-host --lib herdr_core::real_core -- --ignored --nocapture
//! ```
//!
//! Everything runs in a scratch directory: `HOME`, every `XDG_*` variable, `HC_SESSION` and
//! `HC_SOCKET` point into it (Windows: `LOCALAPPDATA`, `APPDATA` and `USERPROFILE` too), and
//! the test refuses to go on if the endpoint it computed is not inside. It never connects to,
//! stops or reads a core of the person running it. The binary must be the pinned one (SHA-256
//! from `tests/fixtures/herdr-core/pins.json`) unless `YUZORA_HC_TEST_ALLOW_UNPINNED=1`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::client::{AttachRequest, DisplayFacts, Session, SessionConfig};
use super::docs::{LayoutState, Op};
use super::events::Effect;
use super::fixtures;
use super::paths::{self, Endpoint, Env};
use super::startup::{
    self, LiveProbe, Origin, Outcome, Probe, ProbeResult, StartError, StartRecord, Supervisor,
    SupervisorConfig, SystemClock,
};
use super::status::{AgentState, AgentView};
use super::testkit::{eventually, request, Collect, Recorder};

fn isolate(command: &mut Command, root: &Path) {
    let home = root.join("home");
    command
        .env("HOME", &home)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("XDG_RUNTIME_DIR");
    if cfg!(windows) {
        command
            .env("LOCALAPPDATA", root.join("local"))
            .env("APPDATA", root.join("roaming"))
            .env("USERPROFILE", &home);
    }
}

fn isolated(binary: &Path, endpoint: &Endpoint, args: &[&str], root: &Path) -> Command {
    let mut command = startup::build_command(
        binary,
        endpoint,
        args,
        std::env::vars_os().map(|(name, _)| name),
    );
    isolate(&mut command, root);
    command
}

/// Runs the CLI inside the scratch environment, for the supervisor's start.
struct Isolated {
    root: PathBuf,
}

impl startup::Launcher for Isolated {
    fn start(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError> {
        let command = isolated(binary, endpoint, &["server", "start"], &self.root);
        startup::run_command(command, Duration::from_secs(30))
    }

    fn restart(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError> {
        let command = isolated(binary, endpoint, &["server", "restart"], &self.root);
        startup::run_command(command, Duration::from_secs(30))
    }
}

/// Stops the scratch core and removes the scratch directory, whatever happens.
struct Scratch {
    root: PathBuf,
    binary: PathBuf,
    endpoint: Endpoint,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Only ever the scratch session's core.
        if LiveProbe.probe(&self.endpoint) != ProbeResult::NoCore {
            let stop = isolated(
                &self.binary,
                &self.endpoint,
                &["server", "stop"],
                &self.root,
            );
            let _ = startup::run_command(stop, Duration::from_secs(20));
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sha256_hex(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The pinned SHA-256 of this platform's `herdr-core` executable.
fn pinned_sha256() -> Option<String> {
    let target = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ("macos-aarch64", "herdr-core")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        ("windows-x86_64", "herdr-core.exe")
    } else {
        return None;
    };
    let pins = fixtures::json("pins.json");
    pins["bundled"]["targets"][target.0]["files"]
        .as_array()?
        .iter()
        .find(|f| f["path"] == target.1)
        .and_then(|f| f["sha256"].as_str().map(String::from))
}

fn scratch_root() -> PathBuf {
    // Short: a Unix socket path is limited to 103 bytes on macOS.
    let base = if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    };
    base.join(format!("yz-hc-it-{}", std::process::id()))
}

#[test]
#[ignore = "needs a real herdr-core: set YUZORA_HC_TEST_BIN"]
fn real_core_attach_flood_events_and_layout_in_a_scratch_session() {
    let binary = PathBuf::from(
        std::env::var_os("YUZORA_HC_TEST_BIN").expect("YUZORA_HC_TEST_BIN=/path/to/herdr-core"),
    );
    let sha = sha256_hex(&binary);
    println!("binary {} sha256 {sha}", binary.display());
    if std::env::var_os("YUZORA_HC_TEST_ALLOW_UNPINNED").is_none() {
        let pinned = pinned_sha256().expect("a pinned binary for this platform");
        assert_eq!(sha, pinned, "not the pinned herdr-core");
    }

    // The scratch environment, and the endpoint inside it.
    let root = scratch_root();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let session_name = format!("yzit{}", std::process::id());
    let endpoint = paths::resolve(
        paths::Os::current(),
        &Env {
            home: Some(root.join("home").to_string_lossy().into_owned()),
            xdg_state_home: Some(root.join("state").to_string_lossy().into_owned()),
            username: std::env::var("USERNAME").ok(),
            localappdata: Some(root.join("local").to_string_lossy().into_owned()),
            ..Env::default()
        },
        &session_name,
    )
    .expect("an endpoint");
    if cfg!(unix) {
        assert!(
            endpoint.socket.starts_with(&root),
            "{:?} is outside the scratch dir",
            endpoint.socket
        );
    } else {
        assert!(endpoint.session.starts_with("yzit"));
    }
    let scratch = Scratch {
        root: root.clone(),
        binary: binary.clone(),
        endpoint: endpoint.clone(),
    };
    println!("endpoint {}", endpoint.socket.display());
    assert_eq!(
        LiveProbe.probe(&endpoint),
        ProbeResult::NoCore,
        "the session must start empty"
    );

    // Start it through the supervisor: cold start, no waiting.
    let record_path = root.join("start.json");
    let mut cfg = SupervisorConfig::new(binary.clone(), endpoint.clone());
    cfg.record_path = Some(record_path.clone());
    let mut supervisor = Supervisor::with_parts(
        cfg,
        Box::new(LiveProbe),
        Box::new(Isolated { root: root.clone() }),
        Box::new(SystemClock),
    );
    let began = Instant::now();
    let Outcome::Ready(ready) = supervisor.ensure() else {
        panic!("the cold start did not produce a core")
    };
    println!(
        "started in {:?}: core {}",
        began.elapsed(),
        ready.hello.version
    );
    let Origin::StartedByYuzora(record) = ready.origin.clone() else {
        panic!("not recognised as ours: {:?}", ready.origin)
    };
    assert_eq!(
        StartRecord::read(&record_path).unwrap(),
        Some(record.clone())
    );
    assert_eq!(record.session, session_name);
    let Outcome::Ready(again) = supervisor.ensure() else {
        panic!("second ensure")
    };
    assert_eq!(again.origin, ready.origin, "the same pid is still ours");

    // Connect: two connections, events subscribed, layout read.
    let recorder = Arc::new(Recorder::default());
    let session = Session::connect(&endpoint, SessionConfig::default(), recorder.clone())
        .expect("connect to the scratch core");
    eventually("the first layout", || {
        !recorder.layouts.lock().unwrap().is_empty()
    });
    assert_eq!(
        recorder.layouts.lock().unwrap()[0].state,
        LayoutState::Empty
    );

    // A flood through an attach stream, credited as a renderer would.
    let flood = session
        .rpc(
            "terminal.run",
            json!({"name": "flood", "cwd": root.to_string_lossy(),
                "env": {"TERM": "xterm-256color", "COLORTERM": "truecolor"},
                "line": "yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c 3000000; echo; echo END-OF-FLOOD; sleep 600"}),
        )
        .expect("terminal.run");
    let flood_address = flood["address"].as_str().unwrap().to_string();
    let sink = Arc::new(Collect::default());
    let display = DisplayFacts {
        bg: Some("#123456".into()),
        fg: Some("#e0e0e0".into()),
        ..DisplayFacts::default()
    };
    let attach = session
        .attach(
            &AttachRequest {
                display,
                ..request(&flood_address)
            },
            sink.clone(),
        )
        .expect("terminal.attach");
    let mut credited = 0;
    let started = Instant::now();
    eventually("END-OF-FLOOD", || {
        let got = sink.total();
        if got > credited {
            attach.credit(got - credited);
            credited = got;
        }
        sink.chunks
            .lock()
            .unwrap()
            .last()
            .is_some_and(|c| c.windows(12).any(|w| w == b"END-OF-FLOOD"))
    });
    let received = sink.total();
    println!(
        "flood: {received} bytes in {:?} over {} deliveries",
        started.elapsed(),
        sink.chunk_count()
    );
    assert!(received >= 3_000_000, "{received}");

    // Typing, resize and a program speaking OSC 7501.
    let echo = session
        .rpc(
            "terminal.run",
            json!({"name": "echo", "cwd": root.to_string_lossy(),
                "line": "printf '\\033]7501;state=blocked:app=it\\033\\\\'; stty -echo; while IFS= read -r l; do echo \"got:$l\"; done"}),
        )
        .unwrap();
    let echo_address = echo["address"].as_str().unwrap().to_string();
    let echo_sink = Arc::new(Collect::default());
    let echo_attach = session
        .attach(&request(&echo_address), echo_sink.clone())
        .unwrap();
    echo_attach.input("hello 世界\r".as_bytes()).unwrap();
    let mut seen = 0;
    eventually("the echo", || {
        let got = echo_sink.total();
        if got > seen {
            echo_attach.credit(got - seen);
            seen = got;
        }
        String::from_utf8_lossy(&echo_sink.bytes()).contains("got:hello 世界")
    });
    echo_attach.resize(120, 40).unwrap();
    eventually("the new size", || {
        session
            .rpc("terminal.get", json!({"terminal": echo_address}))
            .is_ok_and(|t| t["cols"] == 120 && t["rows"] == 40)
    });
    eventually("the 7501 report", || {
        session.terminals().iter().any(|t| {
            t["address"] == echo_address.as_str()
                && matches!(
                    AgentView::from_terminal(t).state,
                    AgentState::Blocked { .. }
                )
        })
    });
    assert!(recorder
        .effects
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, Effect::StatusChanged(a) if *a == echo_address)));

    // The layout document: write, conflict-free read back, and the doc event for our own write.
    let space = Op::AddSpace {
        id: "s_it".into(),
        name: "scratch".into(),
        root: root.to_string_lossy().into_owned(),
        color: None,
    };
    session.update_layout(&space).expect("AddSpace");
    let tab = Op::AddTab {
        space: "s_it".into(),
        tab: "t_it".into(),
        title: "flood".into(),
        leaf: "n_it".into(),
        terminal: flood_address.clone(),
    };
    let updated = session.update_layout(&tab).expect("AddTab");
    assert_eq!(updated.version, 2);
    let read = session.read_layout().unwrap();
    assert_eq!(read.version, 2);
    eventually("the layout event", || {
        recorder
            .layouts
            .lock()
            .unwrap()
            .last()
            .is_some_and(|l| l.version == 2)
    });

    // Release the viewers; the terminals keep running.
    attach.detach();
    echo_attach.detach();
    eventually("the detaches", || {
        sink.end().is_some() && echo_sink.end().is_some()
    });
    for address in [&flood_address, &echo_address] {
        session
            .rpc("terminal.kill", json!({"terminal": address}))
            .unwrap();
        session
            .rpc("terminal.rm", json!({"terminal": address}))
            .unwrap();
    }
    session.close();

    // Stop the scratch core and check that it is gone.
    let stop = isolated(&binary, &endpoint, &["server", "stop"], &root);
    startup::run_command(stop, Duration::from_secs(20)).expect("server stop");
    eventually("the core to stop", || {
        LiveProbe.probe(&endpoint) == ProbeResult::NoCore
    });
    drop(scratch);
    assert!(!root.exists());
}
