//! Service-level tests. The stateful ones run against `fake_core` (Unix sockets) through the
//! same `Session` and `LiveProbe` a real core would meet; none of them touches a home
//! directory, `HC_*`, or a real core.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tauri::http::{HeaderMap, HeaderValue};
use tauri::ipc::InvokeBody;

use super::service::{Backoff, Signal, TERMINAL_ENV};
use super::*;

#[test]
fn herdr_core_signal_stops_sleeps_and_tells_old_session_ends_from_new_ones() {
    let signal = Signal::new();
    assert!(
        !signal.sleep(Duration::from_millis(20)),
        "a plain timeout is not a stop"
    );
    let waiter = {
        let signal = signal.clone();
        std::thread::spawn(move || signal.sleep(Duration::from_secs(30)))
    };
    std::thread::sleep(Duration::from_millis(30));
    signal.stop();
    assert!(waiter.join().unwrap(), "a stop wakes a sleeper at once");
    assert!(signal.stopped() && signal.sleep(Duration::from_secs(30)));

    let signal = Signal::new();
    signal.session_closed(3);
    // The end of epoch 3 says nothing about epoch 4.
    let waiter = {
        let signal = signal.clone();
        std::thread::spawn(move || signal.wait_session_end(4))
    };
    std::thread::sleep(Duration::from_millis(30));
    assert!(!waiter.is_finished());
    signal.session_closed(4);
    assert!(
        !waiter.join().unwrap(),
        "woken by the session, not by a stop"
    );
}

#[test]
fn herdr_core_backoff_doubles_to_a_cap_and_starts_over() {
    let mut backoff = Backoff::new();
    let waits: Vec<u64> = (0..7).map(|_| backoff.take().as_secs()).collect();
    assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30]);
    backoff.reset();
    assert_eq!(backoff.take().as_secs(), 1);
}

#[test]
fn herdr_core_input_request_takes_the_attach_id_from_a_header_and_the_bytes_from_the_body() {
    let mut headers = HeaderMap::new();
    headers.insert(ATTACH_ID_HEADER, HeaderValue::from_static("a7"));
    let body = InvokeBody::Raw(vec![0x1b, b'[', b'A', 0xff]);
    assert_eq!(
        parse_input_request(&headers, &body).unwrap(),
        ("a7".to_string(), vec![0x1b, b'[', b'A', 0xff])
    );
    assert_eq!(
        parse_input_request(&headers, &InvokeBody::Raw(vec![]))
            .unwrap()
            .1,
        Vec::<u8>::new()
    );

    let missing = parse_input_request(&HeaderMap::new(), &body).unwrap_err();
    assert_eq!(missing.code, "invalid_input");
    let mut empty = HeaderMap::new();
    empty.insert(ATTACH_ID_HEADER, HeaderValue::from_static(""));
    assert_eq!(
        parse_input_request(&empty, &body).unwrap_err().code,
        "invalid_input"
    );
    // Anything else as the body is refused.
    for not_bytes in [json!("hi"), json!({"0": 104}), json!(104), json!(null)] {
        assert_eq!(
            parse_input_request(&headers, &InvokeBody::Json(not_bytes))
                .unwrap_err()
                .code,
            "invalid_input"
        );
    }
}

#[test]
fn herdr_core_input_request_accepts_the_json_array_a_postmessage_fallback_sends() {
    // Once the custom protocol fails, Tauri sends a `Uint8Array` as `InvokeBody::Json(array)`.
    let mut headers = HeaderMap::new();
    headers.insert(ATTACH_ID_HEADER, HeaderValue::from_static("a7"));
    let parsed = |value: serde_json::Value| parse_input_request(&headers, &InvokeBody::Json(value));
    assert_eq!(
        parsed(json!([104, 105, 0, 27, 255])).unwrap(),
        ("a7".to_string(), vec![104, 105, 0, 27, 255])
    );
    assert_eq!(parsed(json!([])).unwrap().1, Vec::<u8>::new());
    let every_byte: Vec<u8> = (0..=255).collect();
    assert_eq!(parsed(json!(every_byte)).unwrap().1, every_byte);

    // One bad value refuses the whole input: nothing is half-typed.
    for bad in [
        json!([104, 256]),
        json!([-1]),
        json!([65536]),
        json!([1.5]),
        json!(["a"]),
        json!([null]),
        json!([true]),
        json!([[1]]),
    ] {
        assert_eq!(
            parsed(bad.clone()).unwrap_err().code,
            "invalid_input",
            "{bad}"
        );
    }
    // The attach id is still required on this path.
    assert_eq!(
        parse_input_request(&HeaderMap::new(), &InvokeBody::Json(json!([1])))
            .unwrap_err()
            .code,
        "invalid_input"
    );
}

#[test]
fn herdr_core_the_json_input_fallback_is_logged_once() {
    let flag = AtomicBool::new(false);
    assert!(first_time(&flag));
    assert!(!first_time(&flag));
    assert!(!first_time(&flag));
    // Accepting input on the fallback path sets the real flag; it never goes back.
    let mut headers = HeaderMap::new();
    headers.insert(ATTACH_ID_HEADER, HeaderValue::from_static("a1"));
    parse_input_request(&headers, &InvokeBody::Json(json!([1]))).unwrap();
    assert!(JSON_INPUT_LOGGED.load(Ordering::Relaxed));
    parse_input_request(&headers, &InvokeBody::Json(json!([2]))).unwrap();
    assert!(JSON_INPUT_LOGGED.load(Ordering::Relaxed));
}

#[test]
fn herdr_core_blocking_adapter_reports_a_panicking_operation_as_an_internal_error() {
    let core = HerdrCore::new(Deps::production(|_| {}));
    let state = HerdrCoreState(Arc::new(core));
    let ok = tauri::async_runtime::block_on(blocking(&state, |_| Ok(7)));
    assert_eq!(ok, Ok(7));
    let error = tauri::async_runtime::block_on(blocking(&state, |_| -> Result<(), CoreError> {
        panic!("herdr-core-adapter-panic");
    }))
    .unwrap_err();
    assert_eq!(error.code, "internal");
    assert!(error.message.contains("herdr-core-adapter-panic"));
}

#[test]
fn herdr_core_every_command_is_registered_in_lib() {
    let lib = include_str!("../lib.rs");
    let run_source = lib.split("#[cfg(test)]").next().unwrap();
    let service = include_str!("../herdr_core_service.rs");
    let mut commands: Vec<&str> = service
        .lines()
        .map(str::trim)
        .filter_map(|line| {
            line.strip_prefix("pub async fn herdr_core_")
                .or_else(|| line.strip_prefix("pub fn herdr_core_"))
        })
        .map(|rest| &rest[..rest.find('(').unwrap()])
        .collect();
    commands.sort_unstable();
    assert_eq!(
        commands,
        [
            "attach",
            "credit",
            "detach",
            "events_release",
            "events_subscribe",
            "facts",
            "focus",
            "input",
            "layout_apply",
            "layout_get",
            "release_all",
            "resize",
            "set_enabled",
            "set_session",
            "status",
            "terminal_close",
            "terminal_list",
            "terminal_run",
        ],
        "a command was added or removed: update lib.rs and this list"
    );
    for name in commands {
        let registration = format!("herdr_core_service::herdr_core_{name},");
        assert!(
            run_source.lines().any(|line| line.trim() == registration),
            "missing command registration: {registration}"
        );
    }
}

#[test]
fn herdr_core_is_wired_into_startup_and_exit() {
    let lib = include_str!("../lib.rs");
    let run_source = lib.split("#[cfg(test)]").next().unwrap();
    assert!(run_source.contains("pub mod herdr_core_service;"));
    // The switch comes from the preferences, so they are managed first.
    let preferences = run_source
        .find("app.manage(runtime_preferences::RuntimePreferencesState(")
        .unwrap();
    let launch = run_source
        .find("herdr_core_service::HerdrCoreState::launch(")
        .expect("startup must build the herdr-core service");
    assert!(preferences < launch);
    assert!(
        launch < run_source.find(".invoke_handler(").unwrap(),
        "the service exists before any command can run"
    );
    // The exit step sits between the child-process cleanup and the database shutdown.
    let child_cleanup = run_source
        .find("git_service::kill_all_processes()")
        .unwrap();
    let herdr_core_release = run_source
        .find("app.state::<herdr_core_service::HerdrCoreState>()")
        .expect("exit must release the herdr-core viewers");
    let database = run_source
        .rfind("shutdown_database_runtime_on_dedicated_thread(database_profiles)")
        .unwrap();
    assert!(child_cleanup < herdr_core_release && herdr_core_release < database);
    let exit_call = &run_source[herdr_core_release..database];
    assert!(exit_call.contains(".release_all_within(herdr_core_service::EXIT_RELEASE_DEADLINE)"));
    assert!(
        !exit_call.contains("terminal_close") && !exit_call.contains("kill"),
        "exit releases viewers and the connection; it never stops the core"
    );
    assert!(EXIT_RELEASE_DEADLINE <= Duration::from_secs(2));
    // A reloaded page is cleaned up by the page load hook, on the builder like the other hooks.
    let hook = run_source
        .find(".on_page_load(herdr_core_service::on_page_load)")
        .expect("the main WebView's page load must release herdr-core viewers");
    assert!(hook < run_source.find(".invoke_handler(").unwrap());
    assert!(hook > run_source.find("tauri::Builder::default()").unwrap());
}

#[test]
fn herdr_core_only_the_main_webviews_page_start_counts_as_a_reload() {
    // The unit the hook decides on: nothing here needs a window.
    let core = HerdrCore::new(Deps::production(|_| {}));
    let state = HerdrCoreState(Arc::new(core));
    for (label, event) in [
        ("main", PageLoadEvent::Finished),
        ("preview-child", PageLoadEvent::Started),
        ("preview-child", PageLoadEvent::Finished),
        ("", PageLoadEvent::Started),
        ("Main", PageLoadEvent::Started),
    ] {
        assert!(!page_load_releases(label, event), "{label} {event:?}");
    }
    assert!(page_load_releases("main", PageLoadEvent::Started));
    // A page that loads before the service exists (or with the switch off) has nothing to
    // release and must not panic.
    page_loaded("main", PageLoadEvent::Started, None);
    page_loaded("main", PageLoadEvent::Started, Some(&state));
}

#[test]
fn herdr_core_set_session_is_refused_in_a_debug_build_and_changes_nothing() {
    use crate::runtime_preferences::{RuntimePreferences, RuntimePreferencesState};

    // The preferences file is shared with the installed app, whose session this would move.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime-preferences.json");
    let stored = r#"{"wslEnabled":true,"herdrCoreSession":"yuzora"}"#;
    std::fs::write(&file, stored).unwrap();
    let preferences = RuntimePreferencesState::default();
    let core = HerdrCore::new(Deps::production(|_| {}));
    let status = core.status_snapshot();

    for session in [HerdrCoreSession::Default, HerdrCoreSession::Yuzora] {
        let refused =
            change_session_for(dir.path(), &preferences, &core, session, true).unwrap_err();
        assert_eq!(refused.code, "dev_session_fixed");
        assert!(refused.detail.is_none());
    }
    assert_eq!(std::fs::read_to_string(&file).unwrap(), stored);
    assert_eq!(
        *preferences.0.lock().unwrap(),
        RuntimePreferences::default()
    );
    assert_eq!(
        core.status_snapshot(),
        status,
        "the service was not touched"
    );

    // An installed build still can, and a failed write is the other error.
    let changed = change_session_for(
        dir.path(),
        &preferences,
        &core,
        HerdrCoreSession::Default,
        false,
    )
    .unwrap();
    assert!(!changed.enabled);
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(raw.contains("\"herdrCoreSession\": \"default\""), "{raw}");
    assert!(raw.contains("\"wslEnabled\": true"), "{raw}");
    let unwritable = tempfile::NamedTempFile::new().unwrap();
    let blocked = change_session_for(
        &unwritable.path().join("not-a-dir"),
        &preferences,
        &core,
        HerdrCoreSession::Yuzora,
        false,
    )
    .unwrap_err();
    assert_eq!(blocked.code, "preferences_unwritable");
}

#[test]
fn herdr_core_the_commands_act_as_the_build_they_run_in() {
    use crate::runtime_preferences::{load_from, DEV_BUILD};

    // `change_session` and `change_enabled` are what the commands run, and they ask for this
    // build's own `DEV_BUILD`: the tests above hand `change_session_for` both values, so only
    // these two can show that a debug build leaves the installed app's fields alone and an
    // installed build leaves the debug build's.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("runtime-preferences.json");
    let stored =
        r#"{"herdrCoreEnabled":true,"herdrCoreEnabledDev":true,"herdrCoreSession":"yuzora"}"#;
    std::fs::write(&file, stored).unwrap();
    let preferences = RuntimePreferencesState::default();
    let core = HerdrCore::new(Deps::production(|_| {}));

    let result = change_session(dir.path(), &preferences, &core, HerdrCoreSession::Default);
    let raw = std::fs::read_to_string(&file).unwrap();
    if DEV_BUILD {
        assert_eq!(result.unwrap_err().code, "dev_session_fixed");
        assert_eq!(raw, stored, "a debug build must not write the session");
    } else {
        assert!(result.is_ok());
        assert!(raw.contains("\"herdrCoreSession\": \"default\""), "{raw}");
    }

    // Switching off (nothing may start here): only this build's field turns off.
    std::fs::write(&file, stored).unwrap();
    let status = change_enabled(dir.path(), &preferences, &core, false).unwrap();
    assert!(!status.enabled);
    let on_disk = load_from(dir.path());
    assert_eq!(
        (on_disk.herdr_core_enabled, on_disk.herdr_core_enabled_dev),
        if DEV_BUILD {
            (true, false)
        } else {
            (false, true)
        },
        "the other build's switch is not this one's to change"
    );

    // And the commands hand over to those two without a say in it.
    let source = include_str!("../herdr_core_service.rs");
    for (command, helper) in [
        ("pub async fn herdr_core_set_enabled", "change_enabled("),
        ("pub async fn herdr_core_set_session", "change_session("),
    ] {
        let body = source[source.find(command).unwrap()..]
            .split("\n}\n")
            .next()
            .unwrap();
        assert!(body.contains(helper), "{command} must call {helper}");
        assert!(
            !body.contains("DEV_BUILD") && !body.contains("_for("),
            "{command} must leave the build to {helper}"
        );
    }
}

#[test]
fn herdr_core_terminals_are_started_as_yuzora_like_the_herdr_native_client() {
    assert!(TERMINAL_ENV.contains(&("TERM_PROGRAM", "yuzora")));
    assert_eq!(TERMINAL_ENV.len(), 3);
    let host = include_str!("../../host/src/herdr_native_client.rs");
    assert!(
        host.contains(r#"command.env("TERM_PROGRAM", "yuzora");"#),
        "the identity this one follows changed"
    );
}

#[cfg(unix)]
mod scenarios {
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::thread;
    use std::time::Instant;

    use yuzora_host::herdr_core::caps::{self, Hello};
    use yuzora_host::herdr_core::client::{AttachRequest, DisplayFacts};
    use yuzora_host::herdr_core::docs::Op;
    use yuzora_host::herdr_core::paths::Endpoint;
    use yuzora_host::herdr_core::startup::{
        CoreInfo, Launcher, Probe, ProbeFailure, ProbeResult, StartError, StartRecord, Supervisor,
        SystemClock,
    };

    use super::super::bundled_start::{ProcessFacts, ProcessLookup};
    use super::super::events::{AttachEndView, EffectView};
    use super::super::fake_core::FakeCore;
    use super::super::service::{AttachOut, AttachSend, EventSink, GuardedLauncher, Paths, Signal};
    use super::super::status::{CoreSource, CoreState};
    use super::*;

    const WAIT: Duration = Duration::from_secs(15);
    /// Calls that end a terminal or a core: Yuzora's exit and switch-off never make them.
    const ENDS_SOMETHING: [&str; 4] = [
        "terminal.kill",
        "terminal.rm",
        "server.stop",
        "server.restart",
    ];

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[derive(Clone, Default)]
    struct Emitted(Arc<Mutex<Vec<CoreStatus>>>);

    impl Emitted {
        fn all(&self) -> Vec<CoreStatus> {
            self.0.lock().unwrap().clone()
        }

        fn last(&self) -> CoreStatus {
            self.all().pop().expect("a status was emitted")
        }

        /// The first status at or after `from` that satisfies `matches`, and the index after it.
        fn wait_for(
            &self,
            from: usize,
            matches: impl Fn(&CoreStatus) -> bool,
        ) -> (usize, CoreStatus) {
            let mut found = None;
            wait_until("a status", || {
                let all = self.all();
                found = all
                    .iter()
                    .enumerate()
                    .skip(from)
                    .find(|(_, status)| matches(status))
                    .map(|(at, status)| (at + 1, status.clone()));
                found.is_some()
            });
            found.unwrap()
        }

        fn wait_state(&self, state: CoreState) -> CoreStatus {
            self.wait_for(0, |status| status.state == state).1
        }
    }

    struct FakeProcesses(Option<ProcessFacts>);

    impl ProcessLookup for FakeProcesses {
        fn lookup(&self, pid: u32) -> Option<ProcessFacts> {
            self.0.clone().filter(|_| pid == 4242)
        }
    }

    fn endpoint_at(socket: &Path, session: &str) -> Endpoint {
        Endpoint {
            session: session.into(),
            socket: socket.to_path_buf(),
            holder_socket: socket.with_extension("holder"),
            state: socket.parent().unwrap().to_path_buf(),
        }
    }

    struct Fixture {
        core: HerdrCore,
        emitted: Emitted,
        dir: tempfile::TempDir,
        socket: PathBuf,
    }

    impl Fixture {
        /// The production service, except that the endpoint is `socket` and the "bundled
        /// binary" does not exist, so a test can never start a real core.
        fn new(socket: Option<&Path>, processes: FakeProcesses) -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let socket = socket.map_or_else(|| dir.path().join("none.sock"), Path::to_path_buf);
            let emitted = Emitted::default();
            let sink = emitted.clone();
            let mut deps =
                Deps::production(move |status| sink.0.lock().unwrap().push(status.clone()));
            let at = socket.clone();
            deps.endpoint = Box::new(move |session| Ok(endpoint_at(&at, session)));
            let binary = dir.path().join("no-such-herdr-core");
            deps.bundled_binary = Box::new(move |_| binary.clone());
            deps.processes = Box::new(processes);
            let core = HerdrCore::new(deps);
            core.configure_paths(dir.path().join("herdr-core"), None);
            Fixture {
                core,
                emitted,
                dir,
                socket,
            }
        }

        fn against(fake: &FakeCore) -> Fixture {
            Fixture::new(Some(&fake.socket), FakeProcesses(None))
        }

        fn ready(fake: &FakeCore) -> Fixture {
            let fixture = Fixture::against(fake);
            fixture.core.apply_config(true, "yuzora");
            fixture.emitted.wait_state(CoreState::Ready);
            fixture
        }
    }

    type Output = Arc<Mutex<Vec<AttachOut>>>;

    fn collector() -> (AttachSend, Output) {
        let out: Output = Arc::default();
        let sink = out.clone();
        (
            Arc::new(move |item| {
                sink.lock().unwrap().push(item);
                true
            }),
            out,
        )
    }

    fn bytes_of(out: &Output) -> Vec<u8> {
        out.lock()
            .unwrap()
            .iter()
            .filter_map(|item| match item {
                AttachOut::Bytes(bytes) => Some(bytes.clone()),
                AttachOut::Message(_) => None,
            })
            .flatten()
            .collect()
    }

    fn closed_end(out: &Output) -> Option<AttachEndView> {
        out.lock().unwrap().iter().find_map(|item| match item {
            AttachOut::Message(AttachMessage::Closed { end }) => Some(end.clone()),
            AttachOut::Bytes(_) => None,
        })
    }

    fn request(terminal: &str) -> AttachRequest {
        AttachRequest {
            terminal: terminal.into(),
            cols: 80,
            rows: 24,
            history: 0,
            display: DisplayFacts::default(),
        }
    }

    fn event_collector() -> (EventSink, Arc<Mutex<Vec<CoreEvent>>>) {
        let events: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
        let sink = events.clone();
        (
            Arc::new(move |event| {
                sink.lock().unwrap().push(event.clone());
                true
            }),
            events,
        )
    }

    // ------------------------------------------------------------------ switch off

    struct CountingProbe {
        results: Mutex<VecDeque<ProbeResult>>,
        calls: Arc<AtomicUsize>,
    }

    impl Probe for CountingProbe {
        fn probe(&self, _: &Endpoint) -> ProbeResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut results = self.results.lock().unwrap();
            if results.len() > 1 {
                results.pop_front().unwrap()
            } else {
                results.front().cloned().unwrap_or(ProbeResult::NoCore)
            }
        }
    }

    struct CountingLauncher(Arc<AtomicUsize>);

    impl Launcher for CountingLauncher {
        fn start(&self, _: &Path, _: &Endpoint) -> Result<(), StartError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn restart(&self, binary: &Path, endpoint: &Endpoint) -> Result<(), StartError> {
            self.start(binary, endpoint)
        }
    }

    /// A fixture whose supervisor probes `results` and counts probes and core starts.
    fn counted(
        results: Vec<ProbeResult>,
    ) -> (
        Fixture,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        counted_with(results, Duration::from_millis(300))
    }

    /// [`counted`], with the time a started core gets to answer.
    fn counted_with(
        results: Vec<ProbeResult>,
        start_timeout: Duration,
    ) -> (
        Fixture,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let mut fixture = Fixture::new(None, FakeProcesses(None));
        let (probes, starts, endpoints) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let sink = fixture.emitted.clone();
        let mut deps = Deps::production(move |status| sink.0.lock().unwrap().push(status.clone()));
        let socket = fixture.socket.clone();
        let counter = endpoints.clone();
        deps.endpoint = Box::new(move |session| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(endpoint_at(&socket, session))
        });
        // A real file, so the launcher guard lets the (counting) launcher run.
        let binary = fixture.dir.path().join("herdr-core");
        std::fs::write(&binary, b"x").unwrap();
        deps.bundled_binary = Box::new(move |_| binary.clone());
        let (probe_calls, start_calls) = (probes.clone(), starts.clone());
        deps.supervisor = Box::new(move |mut config, signal| {
            config.start_timeout = start_timeout;
            Supervisor::with_parts(
                config,
                Box::new(CountingProbe {
                    results: Mutex::new(results.clone().into()),
                    calls: probe_calls.clone(),
                }),
                Box::new(GuardedLauncher {
                    inner: CountingLauncher(start_calls.clone()),
                    signal,
                }),
                Box::new(SystemClock),
            )
        });
        fixture.core = HerdrCore::new(deps);
        fixture
            .core
            .configure_paths(fixture.dir.path().join("herdr-core-data"), None);
        (fixture, probes, starts, endpoints)
    }

    fn every_data_command_is_refused(core: &HerdrCore) {
        let op: Op =
            serde_json::from_value(json!({"op": "renameSpace", "space": "s", "name": "n"}))
                .unwrap();
        let (send, _) = collector();
        let (sink, _) = event_collector();
        let codes = [
            core.terminal_list(false).err(),
            core.terminal_run(Some("/".into())).err(),
            core.terminal_close("t1").err(),
            core.layout_get().err(),
            core.layout_apply(&op).err(),
            core.attach(&request("t1"), send).err(),
            core.lane("a1", LaneCommand::Input(b"x".to_vec())).err(),
            core.lane("a1", LaneCommand::Credit(1)).err(),
            core.detach("a1").err(),
            core.release_all().err(),
            core.subscribe(sink).err(),
            core.unsubscribe("s1").err(),
        ];
        for (at, error) in codes.into_iter().enumerate() {
            let error = error.unwrap_or_else(|| panic!("command {at} was not refused"));
            assert_eq!(error.code, "disabled", "command {at}");
        }
    }

    #[test]
    fn herdr_core_switched_off_refuses_every_data_command_and_touches_nothing() {
        let (fixture, probes, starts, endpoints) = counted(vec![ProbeResult::NoCore]);
        fixture.core.apply_config(false, "yuzora");
        every_data_command_is_refused(&fixture.core);
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            (
                probes.load(Ordering::SeqCst),
                starts.load(Ordering::SeqCst),
                endpoints.load(Ordering::SeqCst)
            ),
            (0, 0, 0),
            "off means no probe, no core start, not even an endpoint lookup"
        );
        let status = fixture.core.status_snapshot();
        assert_eq!((status.state, status.enabled), (CoreState::Disabled, false));
        assert!(!fixture.core.is_enabled());
    }

    #[test]
    fn herdr_core_switching_off_stops_the_controller_for_good() {
        let (fixture, probes, _, _) = counted(vec![ProbeResult::Failed(ProbeFailure::new(
            "protocol", "garbled",
        ))]);
        fixture.core.apply_config(true, "yuzora");
        let unreachable = fixture.emitted.wait_state(CoreState::Unreachable);
        assert_eq!(
            unreachable.reason.as_ref().map(|r| r.code.as_str()),
            Some("protocol"),
            "the probe's own code reaches the status"
        );
        assert!(unreachable
            .reason
            .as_ref()
            .is_some_and(|r| r.detail.contains("garbled")));
        assert_eq!(
            unreachable.socket.as_deref(),
            Some(fixture.socket.to_str().unwrap())
        );

        fixture.core.apply_config(false, "yuzora");
        let seen = probes.load(Ordering::SeqCst);
        every_data_command_is_refused(&fixture.core);
        // The old backoff (1 s) runs out: nothing probes again and nothing is announced.
        thread::sleep(Duration::from_millis(1400));
        assert_eq!(probes.load(Ordering::SeqCst), seen);
        assert_eq!(fixture.emitted.last().state, CoreState::Disabled);
    }

    /// A core the supervisor would adopt once it answers: compatible, started a moment ago.
    fn adoptable_core() -> CoreInfo {
        let names = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect();
        CoreInfo {
            hello: Hello {
                version: caps::BUNDLED_VERSION.into(),
                generation: caps::GENERATION,
                methods: names(&[caps::REQUIRED_METHODS, caps::OPTIONAL_METHODS].concat()),
                features: names(caps::REQUIRED_FEATURES),
                core_id: "late".into(),
                framed: false,
            },
            pid: Some(4242),
            uptime_ms: Some(0),
        }
    }

    #[test]
    fn herdr_core_switching_off_while_a_start_waits_ends_the_wait_and_writes_no_record() {
        // The core is started, then takes about half a second to answer.
        let mut results = vec![ProbeResult::NoCore; 5];
        results.push(ProbeResult::Core(adoptable_core()));
        let (fixture, probes, starts, _) = counted_with(results, Duration::from_secs(5));
        fixture.core.apply_config(true, "yuzora");
        wait_until("the core to be started", || {
            starts.load(Ordering::SeqCst) == 1
        });

        fixture.core.apply_config(false, "yuzora");
        // Disabled is announced by the switch itself, not after the wait.
        assert_eq!(fixture.emitted.last().state, CoreState::Disabled);
        let (announced, seen) = (fixture.emitted.all().len(), probes.load(Ordering::SeqCst));

        // Long enough for the late core to have answered, were the wait still going.
        thread::sleep(Duration::from_millis(900));
        assert!(
            probes.load(Ordering::SeqCst) <= seen + 1,
            "the wait went on probing after the switch went off ({seen} probes then, {} now)",
            probes.load(Ordering::SeqCst)
        );
        let record = Paths {
            data_dir: fixture.dir.path().join("herdr-core-data"),
            resource_dir: None,
        }
        .record_path("yuzora");
        assert!(
            !record.exists(),
            "no start record for a start that was abandoned"
        );
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            fixture.emitted.all().len(),
            announced,
            "nothing is announced after Disabled"
        );
    }

    #[test]
    fn herdr_core_a_core_that_answers_in_time_is_adopted_and_recorded() {
        // The same scenario left alone: the cancel check must not fire for a live run.
        let mut results = vec![ProbeResult::NoCore; 2];
        results.push(ProbeResult::Core(adoptable_core()));
        let (fixture, _, starts, _) = counted_with(results, Duration::from_secs(5));
        fixture.core.apply_config(true, "yuzora");
        // The core answers, but there is no socket behind the fake probe: the connect fails.
        fixture.emitted.wait_state(CoreState::Unreachable);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let record = Paths {
            data_dir: fixture.dir.path().join("herdr-core-data"),
            resource_dir: None,
        }
        .record_path("yuzora");
        assert!(record.exists(), "the core we started is recorded");
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_missing_core_is_started_once_per_attempt_and_never_after_switch_off() {
        // No core answers; the (counting) launcher "starts" one that never appears.
        let (fixture, _, starts, _) = counted(vec![ProbeResult::NoCore]);
        fixture.core.apply_config(true, "yuzora");
        fixture.emitted.wait_state(CoreState::StartFailed);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let failed = fixture.emitted.last();
        assert_eq!(
            failed.reason.as_ref().map(|r| r.code.as_str()),
            Some("no_answer")
        );
        fixture.core.apply_config(false, "yuzora");
        thread::sleep(Duration::from_millis(100));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn herdr_core_a_missing_bundled_binary_is_a_start_failure_and_nothing_is_spawned() {
        let fixture = Fixture::new(None, FakeProcesses(None));
        fixture.core.apply_config(true, "yuzora");
        let failed = fixture.emitted.wait_state(CoreState::StartFailed);
        let reason = failed.reason.unwrap();
        assert_eq!(reason.code, "spawn");
        assert!(reason.detail.contains("missing"), "{}", reason.detail);
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_launcher_guard_refuses_after_stop_and_without_a_binary() {
        let starts = Arc::new(AtomicUsize::new(0));
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr-core");
        let endpoint = endpoint_at(&dir.path().join("s.sock"), "yuzora");
        let signal = Signal::new();
        let guarded = GuardedLauncher {
            inner: CountingLauncher(starts.clone()),
            signal: signal.clone(),
        };
        assert!(guarded.start(&binary, &endpoint).is_err(), "no binary");
        std::fs::write(&binary, b"x").unwrap();
        assert!(guarded.start(&binary, &endpoint).is_ok());
        assert!(guarded.restart(&binary, &endpoint).is_ok());
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        signal.stop();
        assert!(guarded.start(&binary, &endpoint).is_err(), "switched off");
        assert!(guarded.restart(&binary, &endpoint).is_err(), "switched off");
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    // ------------------------------------------------------------------ connecting

    #[test]
    fn herdr_core_an_incompatible_core_is_reported_and_left_alone() {
        let fake = FakeCore::start();
        fake.state.lock().unwrap().generation = 2;
        let fixture = Fixture::against(&fake);
        fixture.core.apply_config(true, "yuzora");
        let status = fixture.emitted.wait_state(CoreState::Incompatible);
        assert_eq!(
            status.reason.as_ref().map(|r| r.code.as_str()),
            Some("generation")
        );
        assert_eq!(status.version.as_deref(), Some("0.1.14"));
        let state = fake.state.lock().unwrap();
        assert_eq!(
            state.framed_connections, 0,
            "an incompatible core is never connected"
        );
        assert!(
            state
                .calls
                .iter()
                .all(|call| call == "hello" || call == "server.status"),
            "only the probe spoke: {:?}",
            state.calls
        );
        drop(state);
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_connects_to_a_running_core_and_reports_it_ready() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let status = fixture.core.status_snapshot();
        assert_eq!(status.state, CoreState::Ready);
        assert_eq!(status.source, Some(CoreSource::External));
        assert_eq!(status.version.as_deref(), Some("0.1.14"));
        assert_eq!(status.session, "yuzora");
        assert!(status.enabled && status.epoch >= 1);
        assert!(status.optional_missing.is_empty());
        assert_eq!(
            status.socket.as_deref(),
            Some(fake.socket.to_str().unwrap())
        );
        assert_eq!(status.bundled_version, "0.1.14");
        // `doc.get` is the worker's first read of the layout document, sent concurrently with
        // Ready: wait for it instead of assuming it already went out.
        for expected in ["events.subscribe", "doc.get", "server.status"] {
            wait_until(expected, || {
                fake.calls().iter().any(|call| call == expected)
            });
        }
        assert_eq!(fake.state.lock().unwrap().framed_connections, 1);
        assert!(fixture.core.is_enabled());
        // Statuses came in order and ended on Ready.
        let seqs: Vec<u64> = fixture.emitted.all().iter().map(|s| s.seq).collect();
        assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_status_source_is_bundled_for_a_core_an_earlier_run_started() {
        let started_s = 1_800_000_000u64;
        let record_for = |exe: &str| StartRecord {
            v: 1,
            pid: 4242,
            started_unix_ms: started_s * 1000,
            exe: exe.into(),
            version: "0.1.14".into(),
            session: "yuzora".into(),
            socket: String::new(),
        };
        for (exe_on_disk, expected) in [
            ("/bundle/herdr-core", CoreSource::Bundled),
            ("/usr/local/bin/herdr-core", CoreSource::External),
        ] {
            let fake = FakeCore::start();
            let processes = FakeProcesses(Some(ProcessFacts {
                exe: Some(PathBuf::from(exe_on_disk)),
                start_time_s: started_s + 1,
            }));
            let fixture = Fixture::new(Some(&fake.socket), processes);
            let paths = Paths {
                data_dir: fixture.dir.path().join("herdr-core"),
                resource_dir: None,
            };
            let mut record = record_for("/bundle/herdr-core");
            record.socket = fake.socket.to_string_lossy().into_owned();
            record.write(&paths.record_path("yuzora")).unwrap();
            fixture.core.apply_config(true, "yuzora");
            let status = fixture.emitted.wait_state(CoreState::Ready);
            assert_eq!(status.source, Some(expected), "{exe_on_disk}");
            fixture.core.apply_config(false, "yuzora");
        }
    }

    #[test]
    fn herdr_core_switching_the_session_reconnects_with_a_new_epoch() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let first = fixture.core.status_snapshot();
        fixture.core.apply_config(true, "default");
        let (_, second) = fixture.emitted.wait_for(0, |s| {
            s.state == CoreState::Ready && s.session == "default" && s.epoch > first.epoch
        });
        assert_eq!(second.session, "default");
        // Idempotent: the same switch again changes nothing.
        let announced = fixture.emitted.all().len();
        fixture.core.apply_config(true, "default");
        assert_eq!(fixture.emitted.all().len(), announced);
        fixture.core.apply_config(false, "default");
    }

    // ------------------------------------------------------------------ terminals and layout

    #[test]
    fn herdr_core_terminal_commands_reach_the_core_with_the_terminal_identity() {
        let fake = FakeCore::start();
        fake.state.lock().unwrap().terminals = vec![json!({"address": "t1", "state": "live"})];
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;

        let listed = core.terminal_list(true).unwrap();
        assert_eq!(listed["terminals"][0]["address"], "t1");

        let run = core.terminal_run(Some("/work/project".into())).unwrap();
        assert_eq!(run["address"], "t1");
        core.terminal_run(None).unwrap();
        core.terminal_run(Some(String::new())).unwrap();
        let runs = fake.state.lock().unwrap().run_params.clone();
        assert_eq!(runs[0]["cwd"], "/work/project");
        assert_eq!(
            runs[0]["env"],
            json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "yuzora"})
        );
        for bare in &runs[1..] {
            assert!(bare.get("cwd").is_none(), "{bare}");
            assert_eq!(bare["env"]["TERM"], "xterm-256color");
        }

        core.terminal_close("t1").unwrap();
        let calls = fake.calls();
        let kill = calls.iter().position(|c| c == "terminal.kill").unwrap();
        assert_eq!(
            calls[kill + 1],
            "terminal.rm",
            "kill, then remove: {calls:?}"
        );

        // Closing what is already gone is not an error and does not go on to `rm`.
        fake.state.lock().unwrap().kill_not_found = true;
        let rms = fake.count("terminal.rm");
        core.terminal_close("t9").unwrap();
        assert_eq!(fake.count("terminal.rm"), rms);
        assert_eq!(core.terminal_close("").unwrap_err().code, "invalid_input");
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_layout_operations_round_trip_and_report_the_terminals_they_removed() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let op = |value: serde_json::Value| -> Op { serde_json::from_value(value).unwrap() };

        let empty = core.layout_get().unwrap();
        assert_eq!(empty.version, 0);

        let added = core
            .layout_apply(&op(
                json!({"op": "addSpace", "id": "s_1", "name": "A", "root": "/r"}),
            ))
            .unwrap();
        assert_eq!((added.version, added.attempts), (1, 1));
        assert_eq!(added.doc.spaces[0].name, "A");
        core.layout_apply(&op(json!({"op": "addTab", "space": "s_1", "tab": "t_1",
            "title": "one", "leaf": "n_1", "terminal": "t1"})))
            .unwrap();
        let closed = core
            .layout_apply(&op(json!({"op": "closeLeaf", "leaf": "n_1"})))
            .unwrap();
        assert_eq!(
            closed.removed_terminals,
            ["t1"],
            "the caller decides whether to end it"
        );
        let json = serde_json::to_value(&closed).unwrap();
        assert_eq!(json["removedTerminals"], json!(["t1"]));
        assert!(json["doc"]["spaces"].is_array());

        let again = core.layout_get().unwrap();
        assert_eq!(again.version, 3);

        let rejected = core
            .layout_apply(&op(
                json!({"op": "renameSpace", "space": "nope", "name": "x"}),
            ))
            .unwrap_err();
        assert_eq!(
            (rejected.code, rejected.detail.as_deref()),
            ("op", Some("no_such_space"))
        );
        core.apply_config(false, "yuzora");
    }

    // ------------------------------------------------------------------ attach

    #[test]
    fn herdr_core_attach_streams_bytes_and_writes_calls_in_the_order_they_were_made() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        let info = core.attach(&request("t1"), send).unwrap();
        assert_eq!(info.terminal["address"], "t1");
        wait_until("first paint", || bytes_of(&out) == b"first paint");
        assert_eq!(fake.state.lock().unwrap().attach_params[0]["history"], 0);
        // Credit is only passed on for bytes the sink was handed, so give it those first.
        fake.push_output(2, &[b'.'; 40_000]);
        wait_until("the bytes the credit is for", || {
            bytes_of(&out).len() == b"first paint".len() + 40_000
        });

        for chunk in [&b"a"[..], b"b", b"c", &[0xff, 0x1b]] {
            core.lane(&info.attach_id, LaneCommand::Input(chunk.to_vec()))
                .unwrap();
        }
        core.lane(&info.attach_id, LaneCommand::Resize(100, 30))
            .unwrap();
        core.lane(&info.attach_id, LaneCommand::Focus(true))
            .unwrap();
        core.lane(&info.attach_id, LaneCommand::Focus(false))
            .unwrap();
        let facts = DisplayFacts {
            bg: Some("#101010".into()),
            cell: Some([9, 18]),
            ..DisplayFacts::default()
        };
        core.lane(&info.attach_id, LaneCommand::Facts(facts))
            .unwrap();
        core.lane(&info.attach_id, LaneCommand::Credit(40_000))
            .unwrap();
        wait_until("every call to be written", || {
            let state = fake.state.lock().unwrap();
            state.input.len() == 5
                && state.credit == 40_000
                && state.facts.len() == 1
                && state.focus.len() == 2
        });
        {
            let state = fake.state.lock().unwrap();
            assert_eq!(state.input, [b'a', b'b', b'c', 0xff, 0x1b]);
            assert_eq!(state.resizes, [(100, 30)]);
            assert_eq!(state.focus, [true, false]);
            assert_eq!(state.facts[0]["bg"], "#101010");
            assert_eq!(state.facts[0]["cell"], json!([9, 18]));
        }

        fake.push_output(2, b" and more");
        wait_until("more output", || bytes_of(&out).ends_with(b" and more"));

        // Detach: the core is told, the stream ends with `detached` after the last byte, and
        // the id is gone. Releasing again is fine.
        core.detach(&info.attach_id).unwrap();
        wait_until("the close frame", || {
            fake.state.lock().unwrap().closes == [2]
        });
        wait_until("the closed message", || closed_end(&out).is_some());
        assert_eq!(closed_end(&out), Some(AttachEndView::Detached));
        assert!(matches!(
            out.lock().unwrap().last(),
            Some(AttachOut::Message(_))
        ));
        assert_eq!(
            core.lane(&info.attach_id, LaneCommand::Credit(1))
                .unwrap_err()
                .code,
            "unknown_attach"
        );
        core.detach(&info.attach_id).unwrap();
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_detach_queued_behind_input_writes_all_of_it_first() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        let info = core.attach(&request("t1"), send).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");

        // Many small writes, then the release straight after: the release is one more item
        // in the same queue, so none of the typed bytes is lost to it.
        let typed: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        for byte in &typed {
            core.lane(&info.attach_id, LaneCommand::Input(vec![*byte]))
                .unwrap();
        }
        core.detach(&info.attach_id).unwrap();
        // The id is gone as soon as the call returns; the CLOSE follows from the lane.
        assert_eq!(
            core.lane(&info.attach_id, LaneCommand::Credit(1))
                .unwrap_err()
                .code,
            "unknown_attach"
        );
        wait_until("the close frame", || {
            fake.state.lock().unwrap().closes == [2]
        });
        wait_until("the closed message", || closed_end(&out).is_some());
        assert_eq!(closed_end(&out), Some(AttachEndView::Detached));
        assert_eq!(
            fake.state.lock().unwrap().input,
            typed,
            "every byte typed before the release reached the core, in order"
        );
        // Releasing again, or an id that never was, is fine.
        core.detach(&info.attach_id).unwrap();
        core.detach("a999").unwrap();
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_the_closed_message_comes_after_the_last_byte_and_frees_the_id() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let (send, out) = collector();
        let info = fixture.core.attach(&request("t1"), send).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");
        fake.push_output(2, b"bye");
        fake.end_stream(2, json!({"reason": "exited", "exit_code": 3}));
        wait_until("the end", || closed_end(&out).is_some());
        assert_eq!(bytes_of(&out), b"first paintbye");
        assert!(matches!(
            out.lock().unwrap().last(),
            Some(AttachOut::Message(_))
        ));
        assert_eq!(
            closed_end(&out),
            Some(AttachEndView::Ended {
                reason: "exited".into(),
                exit_code: Some(3),
                signal: None,
                reattach_after_restart: false,
            })
        );
        wait_until("the id to be freed", || {
            fixture
                .core
                .lane(&info.attach_id, LaneCommand::Credit(1))
                .is_err()
        });
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_sink_that_reports_the_frontend_gone_has_its_viewer_released() {
        // What a Tauri channel does once its WebView is destroyed: `send` fails. (It does not
        // fail for a page that was merely reloaded; `release_for_reload` covers that.)
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let reachable = Arc::new(AtomicBool::new(false));
        let flag = reachable.clone();
        let send: AttachSend = Arc::new(move |_| flag.load(Ordering::SeqCst));
        let info = fixture.core.attach(&request("t1"), send).unwrap();
        // The first paint cannot be delivered: detach, not a stall.
        wait_until("the viewer to be released", || {
            fake.state.lock().unwrap().closes == [2]
        });
        wait_until("the id to be freed", || {
            fixture
                .core
                .lane(&info.attach_id, LaneCommand::Credit(1))
                .is_err()
        });
        fixture.core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_reloaded_page_has_its_viewers_and_subscriptions_released_at_once() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        // Sinks that keep succeeding: this is a reload, so nothing ever reports a failure.
        let (send, out) = collector();
        let one = core.attach(&request("t1"), send.clone()).unwrap();
        let two = core.attach(&request("t2"), send).unwrap();
        let (sink, events) = event_collector();
        let subscription = core.subscribe(sink).unwrap();
        wait_until("both first paints", || {
            bytes_of(&out).len() >= 2 * b"first paint".len()
        });

        // The hook the main WebView's page start runs. By the time it returns the registry is
        // clean (the new page's own attaches are safe), the CLOSE frames follow on a thread.
        let state = HerdrCoreState(Arc::new(core.clone()));
        page_loaded("main", PageLoadEvent::Started, Some(&state));
        for id in [&one.attach_id, &two.attach_id] {
            assert_eq!(
                core.lane(id, LaneCommand::Credit(1)).unwrap_err().code,
                "unknown_attach"
            );
        }
        assert_eq!(
            core.unsubscribe(&subscription).unwrap_err().code,
            "unknown_subscription"
        );
        let (send, fresh) = collector();
        let new_page = core.attach(&request("t3"), send).unwrap();
        wait_until("both close frames", || {
            fake.state.lock().unwrap().closes.len() == 2
        });
        wait_until("both ends", || {
            out.lock()
                .unwrap()
                .iter()
                .filter(|item| matches!(item, AttachOut::Message(_)))
                .count()
                == 2
        });
        // The connection stays, and the page that loaded after the hook is untouched.
        assert_eq!(core.status_snapshot().state, CoreState::Ready);
        wait_until("the new page's first paint", || {
            bytes_of(&fresh) == b"first paint"
        });
        assert!(core
            .lane(&new_page.attach_id, LaneCommand::Credit(1))
            .is_ok());
        let mut closed = fake.state.lock().unwrap().closes.clone();
        closed.sort_unstable();
        assert_eq!(closed, [2, 3], "only the old page's viewers were closed");
        let seen = events.lock().unwrap().len();
        fake.push_event(json!({"seq": 1, "type": "created", "terminal": "t9",
            "data": {"address": "t9", "state": "live"}}));
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            events.lock().unwrap().len(),
            seen,
            "the old page's subscription hears nothing more"
        );
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_page_loads_of_other_webviews_or_other_stages_release_nothing() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        let info = core.attach(&request("t1"), send).unwrap();
        let (sink, _) = event_collector();
        let subscription = core.subscribe(sink).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");

        let state = HerdrCoreState(Arc::new(core.clone()));
        for (label, event) in [
            ("main", PageLoadEvent::Finished),
            ("preview-child", PageLoadEvent::Started),
            ("preview-child", PageLoadEvent::Finished),
        ] {
            page_loaded(label, event, Some(&state));
        }
        thread::sleep(Duration::from_millis(100));
        assert!(core.lane(&info.attach_id, LaneCommand::Credit(1)).is_ok());
        assert!(core.unsubscribe(&subscription).is_ok());
        assert!(fake.state.lock().unwrap().closes.is_empty());
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_an_attach_still_being_answered_when_the_page_reloads_is_released() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let state = HerdrCoreState(Arc::new(core.clone()));
        // The old page's attach is with the core, whose answer is not back yet.
        fake.state.lock().unwrap().hold_attach = true;
        let (send, old_out) = collector();
        let asking = {
            let core = core.clone();
            thread::spawn(move || core.attach(&request("t1"), send))
        };
        wait_until("the attach request", || {
            !fake.state.lock().unwrap().attach_params.is_empty()
        });
        // The reload begins: there is nothing registered to release yet.
        page_loaded("main", PageLoadEvent::Started, Some(&state));
        assert_eq!(core.registered(), (0, 0));
        fake.state.lock().unwrap().hold_attach = false;

        // The answer comes: the viewer is not kept for a page that is gone. Its CLOSE is
        // written, the stream ends for its sink and nothing stays registered (the lane and
        // the pump end with it).
        let error = asking.join().unwrap().unwrap_err();
        assert_eq!(error.code, "page_replaced");
        wait_until("the old viewer's CLOSE", || {
            fake.state.lock().unwrap().closes == [2]
        });
        wait_until("the old stream's end", || closed_end(&old_out).is_some());
        assert_eq!(core.registered(), (0, 0));

        // A page that starts after the reload is not caught up in it.
        let (send, fresh) = collector();
        let new_page = core.attach(&request("t2"), send).unwrap();
        assert_eq!(core.registered(), (1, 0));
        wait_until("the new page's first paint", || {
            bytes_of(&fresh) == b"first paint"
        });
        assert!(core
            .lane(&new_page.attach_id, LaneCommand::Credit(1))
            .is_ok());
        thread::sleep(Duration::from_millis(100));
        assert_eq!(fake.state.lock().unwrap().closes, [2], "only the old one");
        assert_eq!(core.status_snapshot().state, CoreState::Ready);
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_subscription_begun_for_a_replaced_page_registers_nothing() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let state = HerdrCoreState(Arc::new(core.clone()));
        // The first page is number 0, and the reload moves on to the next.
        let first_page = 0;
        page_loaded("main", PageLoadEvent::Started, Some(&state));
        let (sink, events) = event_collector();
        let error = core.subscribe_from_page(first_page, sink).unwrap_err();
        assert_eq!(error.code, "page_replaced");
        assert_eq!(core.registered(), (0, 0));
        assert!(
            events.lock().unwrap().is_empty(),
            "the sink was told nothing"
        );

        // The page that loaded since asks the plain way and gets its subscription.
        let (sink, events) = event_collector();
        core.subscribe(sink).unwrap();
        assert_eq!(core.registered(), (0, 1));
        assert!(!events.lock().unwrap().is_empty());
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_release_all_drops_viewers_and_subscriptions_but_not_the_connection() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        let one = core.attach(&request("t1"), send.clone()).unwrap();
        let two = core.attach(&request("t2"), send).unwrap();
        let (sink, _) = event_collector();
        let subscription = core.subscribe(sink).unwrap();
        wait_until("both first paints", || {
            bytes_of(&out).len() >= 2 * b"first paint".len()
        });

        core.release_all().unwrap();
        wait_until("both close frames", || {
            fake.state.lock().unwrap().closes.len() == 2
        });
        for id in [&one.attach_id, &two.attach_id] {
            assert_eq!(
                core.lane(id, LaneCommand::Credit(1)).unwrap_err().code,
                "unknown_attach"
            );
        }
        assert_eq!(
            core.unsubscribe(&subscription).unwrap_err().code,
            "unknown_subscription"
        );
        // Still connected: a new viewer works.
        assert_eq!(core.status_snapshot().state, CoreState::Ready);
        let (send, _) = collector();
        core.attach(&request("t3"), send).unwrap();
        core.apply_config(false, "yuzora");
    }

    // ------------------------------------------------------------------ events

    #[test]
    fn herdr_core_subscribers_get_the_snapshot_the_layout_and_then_each_change() {
        let fake = FakeCore::start();
        fake.state.lock().unwrap().terminals = vec![json!({"address": "t1", "state": "live"})];
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let epoch = core.status_snapshot().epoch;
        let (sink, events) = event_collector();
        let id = core.subscribe(sink).unwrap();

        wait_until("snapshot and layout", || {
            let events = events.lock().unwrap();
            events
                .iter()
                .any(|e| matches!(e, CoreEvent::Snapshot { .. }))
                && events.iter().any(|e| matches!(e, CoreEvent::Layout { .. }))
        });
        {
            let events = events.lock().unwrap();
            let Some(CoreEvent::Snapshot {
                epoch: at,
                terminals,
            }) = events
                .iter()
                .find(|e| matches!(e, CoreEvent::Snapshot { .. }))
            else {
                unreachable!()
            };
            assert_eq!((*at, terminals.len()), (epoch, 1));
            assert_eq!(terminals[0]["address"], "t1");
        }

        fake.push_event(json!({"seq": 1, "type": "created", "terminal": "t2",
            "data": {"address": "t2", "state": "live"}}));
        wait_until("the added effect", || {
            events.lock().unwrap().iter().any(|e| {
                matches!(e, CoreEvent::Effects { effects, .. }
                    if matches!(effects.as_slice(),
                        [EffectView::Added { terminal, record: Some(record) }]
                            if terminal == "t2" && record["address"] == "t2"))
            })
        });
        assert!(events.lock().unwrap().iter().all(|event| match event {
            CoreEvent::Snapshot { epoch: at, .. }
            | CoreEvent::Effects { epoch: at, .. }
            | CoreEvent::Layout { epoch: at, .. } => *at == epoch,
        }));

        core.unsubscribe(&id).unwrap();
        let seen = events.lock().unwrap().len();
        fake.push_event(json!({"seq": 2, "type": "removed", "terminal": "t2", "data": {}}));
        thread::sleep(Duration::from_millis(200));
        assert_eq!(events.lock().unwrap().len(), seen);
        assert_eq!(
            core.unsubscribe(&id).unwrap_err().code,
            "unknown_subscription"
        );
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_new_subscriber_hears_its_initial_events_before_any_broadcast_made_meanwhile() {
        let fake = FakeCore::start();
        fake.state.lock().unwrap().terminals = vec![json!({"address": "t1", "state": "live"})];
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        // The session has delivered the (empty, version 0) layout once it shows up here.
        let (watcher, watched) = event_collector();
        core.subscribe(watcher).unwrap();
        wait_until("the first layout", || {
            watched
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, CoreEvent::Layout { .. }))
        });
        // A newer layout is in the core, not announced yet.
        let add_space: Op = serde_json::from_value(
            json!({"op": "addSpace", "id": "s_1", "name": "A", "root": "/r"}),
        )
        .unwrap();
        assert_eq!(core.layout_apply(&add_space).unwrap().version, 1);

        // A subscriber whose sink stalls on its first event, as a busy WebView would.
        let log: Arc<Mutex<Vec<CoreEvent>>> = Arc::default();
        let release = Arc::new(AtomicBool::new(false));
        let sink: EventSink = {
            let (log, release) = (log.clone(), release.clone());
            Arc::new(move |event| {
                let first = {
                    let mut log = log.lock().unwrap();
                    log.push(event.clone());
                    log.len() == 1
                };
                // Bounded, so a test that fails leaves no thread spinning for good.
                let give_up = Instant::now() + WAIT;
                while first && !release.load(Ordering::SeqCst) && Instant::now() < give_up {
                    thread::sleep(Duration::from_millis(2));
                }
                true
            })
        };
        let subscribing = {
            let core = core.clone();
            thread::spawn(move || core.subscribe(sink))
        };
        wait_until("the initial snapshot to be handed over", || {
            !log.lock().unwrap().is_empty()
        });

        // The core announces the newer layout meanwhile: the session's worker reads it and
        // broadcasts. That broadcast must wait for this subscriber's turn.
        fake.push_event(json!({"seq": 1, "type": "doc",
            "data": {"name": "yuzora", "version": 1}}));
        for _ in 0..30 {
            let heard = log.lock().unwrap().clone();
            assert_eq!(
                heard.len(),
                1,
                "a broadcast overtook the initial events: {heard:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        release.store(true, Ordering::SeqCst);
        subscribing.join().unwrap().unwrap();
        let layout_versions = |events: &[CoreEvent]| -> Vec<u64> {
            events
                .iter()
                .filter_map(|event| match event {
                    CoreEvent::Layout { version, .. } => Some(*version),
                    _ => None,
                })
                .collect()
        };
        wait_until("the newer layout", || {
            layout_versions(&log.lock().unwrap()).contains(&1)
        });

        let events = log.lock().unwrap().clone();
        assert!(
            matches!(events[0], CoreEvent::Snapshot { .. }),
            "{events:?}"
        );
        assert_eq!(
            layout_versions(&events),
            [0, 1],
            "version 0 first (initial), then 1; never 1 then 0"
        );
        let position = |wanted: fn(&CoreEvent) -> bool| events.iter().position(wanted).unwrap();
        assert!(
            position(|e| matches!(e, CoreEvent::Layout { version: 0, .. }))
                < position(|e| matches!(e, CoreEvent::Effects { .. })),
            "the initial layout precedes the broadcast that came in during it: {events:?}"
        );
        assert!(
            position(|e| matches!(e, CoreEvent::Effects { .. }))
                < position(|e| matches!(e, CoreEvent::Layout { version: 1, .. }))
        );
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_title_change_reaches_subscribers_as_an_updated_effect_with_the_new_record() {
        let fake = FakeCore::start();
        fake.state.lock().unwrap().terminals =
            vec![json!({"address": "t1", "state": "live", "title": "old"})];
        let fixture = Fixture::ready(&fake);
        let (sink, events) = event_collector();
        fixture.core.subscribe(sink).unwrap();
        wait_until("the snapshot", || !events.lock().unwrap().is_empty());

        fake.push_event(json!({"seq": 1, "type": "title", "terminal": "t1",
            "data": {"title": "renamed"}}));
        wait_until("the updated effect", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(event, CoreEvent::Effects { effects, .. }
                    if matches!(effects.as_slice(),
                        [EffectView::Updated { terminal, record: Some(record) }]
                            if terminal == "t1" && record["title"] == "renamed"))
            })
        });
        // On the wire it is `{"kind": "updated", ...}` inside an `effects` event.
        let sent = events
            .lock()
            .unwrap()
            .iter()
            .find(|event| matches!(event, CoreEvent::Effects { .. }))
            .map(|event| serde_json::to_value(event).unwrap())
            .unwrap();
        assert_eq!(sent["type"], "effects");
        assert_eq!(sent["effects"][0]["kind"], "updated");
        assert_eq!(sent["effects"][0]["record"]["title"], "renamed");
        fixture.core.apply_config(false, "yuzora");
    }

    // ------------------------------------------------------------------ loss, switch off, exit

    #[test]
    fn herdr_core_a_lost_connection_ends_the_viewers_and_reconnects_as_a_new_epoch() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let before = core.status_snapshot();
        let (send, out) = collector();
        let info = core.attach(&request("t1"), send).unwrap();
        let (sink, events) = event_collector();
        core.subscribe(sink).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");

        fake.drop_connections();
        wait_until("the session end", || closed_end(&out).is_some());
        assert!(matches!(
            closed_end(&out),
            Some(AttachEndView::Session { .. })
        ));
        let (_, again) = fixture
            .emitted
            .wait_for(0, |s| s.state == CoreState::Ready && s.epoch > before.epoch);
        assert!(again.epoch > before.epoch);
        // The viewer of the old connection is gone; the new connection starts clean.
        assert_eq!(
            core.lane(&info.attach_id, LaneCommand::Credit(1))
                .unwrap_err()
                .code,
            "unknown_attach"
        );
        wait_until("a snapshot of the new epoch", || {
            events
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, CoreEvent::Snapshot { epoch, .. } if *epoch == again.epoch))
        });
        let connecting = fixture
            .emitted
            .all()
            .into_iter()
            .find(|s| s.state == CoreState::Connecting && s.reason.is_some())
            .expect("reconnecting says why the last connection ended");
        assert!(connecting.reason.unwrap().code.len() > 1);
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_a_core_that_goes_away_is_waited_for_not_replaced() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        fake.shut_down();
        let waiting = fixture.emitted.wait_state(CoreState::Waiting);
        let retry = waiting.retry_in_ms.expect("the grace period is shown");
        assert!(retry > 0 && retry <= 10_000, "{retry}");
        fixture.core.apply_config(false, "yuzora");
        assert_eq!(fixture.emitted.last().state, CoreState::Disabled);
    }

    #[test]
    fn herdr_core_switching_off_releases_viewers_closes_the_connection_and_leaves_the_core_running()
    {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        core.attach(&request("t1"), send).unwrap();
        let (sink, _) = event_collector();
        core.subscribe(sink).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");
        let calls_before = fake.calls();

        core.apply_config(false, "yuzora");
        assert_eq!(core.status_snapshot().state, CoreState::Disabled);
        assert_eq!(fixture.emitted.last().state, CoreState::Disabled);
        wait_until("the viewer to be released", || closed_end(&out).is_some());
        wait_until("its close frame", || {
            fake.state.lock().unwrap().closes == [2]
        });
        every_data_command_is_refused(core);

        // The core was only disconnected from: no call that ends anything, and it still
        // accepts connections.
        let after = fake.calls();
        for call in after.iter().skip(calls_before.len()) {
            assert!(!ENDS_SOMETHING.contains(&call.as_str()), "{after:?}");
        }
        assert!(std::os::unix::net::UnixStream::connect(&fake.socket).is_ok());

        // Switched on again, it connects again, as a new epoch.
        let first_epoch = fixture.emitted.all().iter().map(|s| s.epoch).max().unwrap();
        core.apply_config(true, "yuzora");
        let (_, ready) = fixture
            .emitted
            .wait_for(0, |s| s.state == CoreState::Ready && s.epoch > first_epoch);
        assert!(ready.enabled);
        core.apply_config(false, "yuzora");
    }

    #[test]
    fn herdr_core_exit_release_is_bounded_reentrant_and_final() {
        let fake = FakeCore::start();
        let fixture = Fixture::ready(&fake);
        let core = &fixture.core;
        let (send, out) = collector();
        core.attach(&request("t1"), send).unwrap();
        wait_until("first paint", || bytes_of(&out) == b"first paint");

        let began = Instant::now();
        core.release_all_within(EXIT_RELEASE_DEADLINE);
        assert!(began.elapsed() < EXIT_RELEASE_DEADLINE + Duration::from_millis(500));
        wait_until("the viewer's end", || closed_end(&out).is_some());
        wait_until("its close frame", || {
            fake.state.lock().unwrap().closes == [2]
        });
        every_data_command_is_refused(core);

        // ExitRequested and Exit both arrive: the second finds nothing and returns at once.
        let began = Instant::now();
        core.release_all_within(EXIT_RELEASE_DEADLINE);
        assert!(began.elapsed() < Duration::from_millis(200));

        // Nothing starts again after exit began, whatever the switch says.
        let connections = fake.state.lock().unwrap().framed_connections;
        core.apply_config(true, "yuzora");
        thread::sleep(Duration::from_millis(300));
        assert_eq!(fake.state.lock().unwrap().framed_connections, connections);
        assert!(!core.is_enabled());
        // The core itself was not touched.
        assert!(fake
            .calls()
            .iter()
            .all(|call| !ENDS_SOMETHING.contains(&call.as_str())));
    }
}
