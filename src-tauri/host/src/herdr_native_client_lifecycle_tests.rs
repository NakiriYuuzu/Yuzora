//! Real PTY fixtures; no HERDR runtime, user configuration or process groups.
use super::*;
use portable_pty::Child as PtyChild;
use std::io;

struct OwnedPty {
    _sandbox: tempfile::TempDir,
    master: Option<Box<dyn MasterPty + Send>>,
    input: Option<Box<dyn Write + Send>>,
    output: Option<Box<dyn Read + Send>>,
    child: Option<Box<dyn PtyChild + Send + Sync>>,
    pid: i32,
    owned: bool,
}

impl OwnedPty {
    fn spawn() -> Result<Self, String> {
        let sandbox = tempfile::tempdir().map_err(|e| e.to_string())?;
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(|e| e.to_string())?;
        let output = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let input = pair.master.take_writer().map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new("/bin/sh");
        command.env_clear();
        command.env("PATH", "/usr/bin:/bin");
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
            "TMPDIR",
        ] {
            let path = sandbox.path().join(key);
            std::fs::create_dir(&path).map_err(|e| e.to_string())?;
            command.env(key, path);
        }
        command.cwd(sandbox.path());
        command.args(["-c", "printf R; IFS= read -r line; exit 0"]);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let pid = child.process_id().expect("Unix PTY PID") as i32;
        let mut fixture = Self {
            _sandbox: sandbox,
            master: Some(pair.master),
            input: Some(input),
            output: Some(output),
            child: Some(child),
            pid,
            owned: true,
        };
        let mut poll = libc::pollfd {
            fd: fixture.master.as_ref().unwrap().as_raw_fd().unwrap(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, 2000) };
        if ready <= 0 {
            return Err("owned PTY readiness failed".into());
        }
        let mut marker = [0];
        fixture
            .output
            .as_mut()
            .unwrap()
            .read_exact(&mut marker)
            .map_err(|e| e.to_string())?;
        if marker != *b"R" {
            return Err("unexpected PTY readiness marker".into());
        }
        Ok(fixture)
    }

    fn release(&mut self, manager: &HerdrManager, id: &str) -> Result<(), String> {
        // A reader that has already stopped leaves only the client's master.
        self.output.take();
        manager.native_clients.lock().unwrap().insert(
            id.into(),
            Arc::new(NativeHerdrClient {
                master: Mutex::new(self.master.take().unwrap().into()),
                input: Mutex::new(self.input.take()),
                child: Mutex::new(self.child.take().unwrap()),
                reader: Mutex::new(None),
                closed: AtomicBool::new(false),
                released: AtomicBool::new(false),
            }),
        );
        manager.release_native_client(id)
    }

    fn observe(&mut self) -> Result<&'static str, String> {
        if !self.owned {
            return Ok("reaped");
        }
        match process_kill::leader_exited_without_reaping(self.pid) {
            Ok(true) => Ok("zombie"),
            Ok(false) => Ok("running"),
            Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                self.owned = false;
                Ok("reaped")
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => self.observe(),
            Err(error) => Err(format!("observe {}: {error}", self.pid)),
        }
    }

    fn settled_state(&mut self) -> Result<&'static str, String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let state = self.observe()?;
            if state != "running" || Instant::now() >= deadline {
                return Ok(state);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn cleanup(&mut self) -> Result<(), String> {
        self.output.take();
        self.input.take();
        self.master.take();
        if !self.owned {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut signalled = false;
        loop {
            let mut status = 0;
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if result == self.pid {
                self.owned = false;
                return Ok(());
            }
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    self.owned = false;
                    return Ok(());
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("owned cleanup waitpid {}: {error}", self.pid));
            }
            // Only waitpid == 0 proves that this PID is still our child.
            if !signalled {
                unsafe {
                    libc::kill(self.pid, libc::SIGKILL);
                }
                signalled = true;
            }
            if Instant::now() >= deadline {
                return Err(format!("owned cleanup timeout {}", self.pid));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
impl Drop for OwnedPty {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            if std::thread::panicking() {
                eprintln!("fixture cleanup failed during unwind: {error}");
            } else {
                panic!("fixture cleanup failed: {error}");
            }
        }
    }
}

#[test]
fn native_client_close_reaps_when_pty_output_is_not_drained() {
    let manager = HerdrManager::new();
    let mut fixture = OwnedPty::spawn().unwrap();
    let result = fixture.release(&manager, "owned-fixture-client");
    let state = fixture.settled_state();
    let repeated = manager.release_native_client("owned-fixture-client");
    let empty = manager.native_clients.lock().unwrap().is_empty();
    eprintln!("actual manager release: {result:?}; state before cleanup: {state:?}; repeated={repeated:?}; map_empty={empty}");
    let cleanup = fixture.cleanup();
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(result.is_ok(), "actual manager release failed: {result:?}");
    assert_eq!(state.unwrap(), "reaped");
    assert!(repeated.is_ok() && empty);
}

#[test]
fn native_client_repeated_undrained_release_does_not_accumulate_children() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let root = tempfile::tempdir().unwrap();
    for name in [
        "home", "config", "data", "state", "cache", "run", "tmp", "history",
    ] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    let shell = root.path().join("fixture-shell");
    std::fs::write(&shell, "#!/bin/sh\nexec /bin/sh \"$@\"\n").unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
    let herdr = root.path().join("herdr.toml");
    std::fs::write(
        &herdr,
        "onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n",
    )
    .unwrap();
    std::fs::write(root.path().join("owner"), std::process::id().to_string()).unwrap();
    let log_path = root.path().join("lifecycle.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "herdr_service::native_client::lifecycle_tests::owned_native_client_lifecycle_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.path().join("home"))
        .env("CFFIXED_USER_HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env("TMPDIR", root.path().join("tmp"))
        .env("SHELL", &shell)
        .env("HISTFILE", root.path().join("history/shell-history"))
        .env("HERDR_CONFIG_PATH", &herdr)
        .env("YUZORA_NATIVE_CLIENT_LIFECYCLE_ROOT", root.path())
        .current_dir(root.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .status()
        .unwrap();
    let output = std::fs::read_to_string(log_path).unwrap();
    print!("{output}");
    assert!(status.success(), "isolated lifecycle child failed");
    let census: Vec<_> = output
        .lines()
        .filter_map(|line| {
            line.split_once("NATIVE_CLIENT_CENSUS ")
                .map(|(_, json)| json)
        })
        .collect();
    assert_eq!(census.len(), 1);
    let census: serde_json::Value = serde_json::from_str(census[0]).unwrap();
    assert_eq!(census["warmup"].as_array().unwrap().len(), 20);
    assert_eq!(census["rows"].as_array().unwrap().len(), 100);
}

#[test]
#[ignore = "parent-only isolated resource-count fixture"]
fn owned_native_client_lifecycle_child() {
    let root = std::path::PathBuf::from(
        std::env::var("YUZORA_NATIVE_CLIENT_LIFECYCLE_ROOT").expect("parent-only fixture"),
    );
    assert_eq!(
        std::fs::read_to_string(root.join("owner")).unwrap(),
        unsafe { libc::getppid() }.to_string()
    );
    for (key, relative) in [
        ("HOME", "home"),
        ("CFFIXED_USER_HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
        ("TMPDIR", "tmp"),
        ("SHELL", "fixture-shell"),
        ("HISTFILE", "history/shell-history"),
        ("HERDR_CONFIG_PATH", "herdr.toml"),
    ] {
        assert_eq!(
            std::path::PathBuf::from(std::env::var_os(key).unwrap()),
            root.join(relative)
        );
    }
    assert!(
        std::env::vars_os()
            .all(|(key, _)| !key.to_string_lossy().starts_with("HERDR_")
                || key == "HERDR_CONFIG_PATH")
    );
    // A relative census must not accept a foreign PTY as part of its baseline.
    let inherited_ptys: Vec<_> = std::fs::read_dir("/dev/fd")
        .unwrap()
        .map(Result::unwrap)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|fd| *fd > 2 && unsafe { libc::isatty(*fd) } == 1)
        .map(|fd| {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
            let rc = unsafe { libc::fstat(fd, stat.as_mut_ptr()) };
            assert_eq!(rc, 0);
            let stat = unsafe { stat.assume_init() };
            serde_json::json!({"fd":fd,"flags":unsafe {libc::fcntl(fd,libc::F_GETFD)},"dev":stat.st_dev,"ino":stat.st_ino,"rdev":stat.st_rdev})
        })
        .collect();
    assert!(
        inherited_ptys.is_empty(),
        "inherited PTYs: {inherited_ptys:?}"
    );
    #[cfg(target_os = "macos")]
    setup_checks::verify();
    let manager = HerdrManager::new();
    let fd_count = || {
        std::fs::read_dir("/dev/fd")
            .map(|entries| entries.count())
            .map_err(|e| e.to_string())
    };
    let mut warmup = Vec::new();
    for cycle in 0..20 {
        let mut fixture = OwnedPty::spawn().unwrap();
        let release = fixture.release(&manager, &format!("warm-{cycle}"));
        let state = fixture.settled_state();
        let cleanup = fixture.cleanup();
        warmup.push(
            serde_json::json!({"cycle":cycle,"release":release,"state":state,"cleanup":cleanup}),
        );
    }
    // A release drops the PTY writer on a thread of its own (`release_input`),
    // so its descriptor closes shortly after the release returns: the baseline
    // is the lowest count once that settles, and each census waits for it.
    let baseline_fds = (0..100)
        .map(|_| {
            std::thread::sleep(Duration::from_millis(1));
            fd_count().unwrap()
        })
        .min()
        .unwrap();
    let settled_fds = || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let count = fd_count();
            if !matches!(count, Ok(count) if count > baseline_fds) || Instant::now() >= deadline {
                return count;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    };
    let mut retained = Vec::new();
    let mut rows = Vec::new();
    let mut failed_setup = None;
    for cycle in 0..100 {
        let mut fixture = match OwnedPty::spawn() {
            Ok(fixture) => fixture,
            Err(error) => {
                failed_setup = Some(error);
                break;
            }
        };
        let id = format!("measured-{cycle}");
        let release = fixture.release(&manager, &id);
        let state = fixture.settled_state();
        let repeated = manager.release_native_client(&id);
        let empty = manager.native_clients.lock().unwrap().is_empty();
        let pid = fixture.pid;
        retained.push(fixture);
        let mut zombies = 0;
        let mut running = 0;
        let mut census_errors = Vec::new();
        for child in &mut retained {
            // Reaped entries return their remembered state without another PID lookup.
            match child.observe() {
                Ok("zombie") => zombies += 1,
                Ok("running") => running += 1,
                Ok(_) => {}
                Err(error) => census_errors.push(error),
            }
        }
        rows.push(serde_json::json!({"cycle":cycle,"pid":pid,"release":release,"state":state,"repeated":repeated,"map_empty":empty,"zombies":zombies,"running":running,"census_errors":census_errors,"fds":settled_fds()}));
    }
    // All evidence is written while the 100 measured lifetimes still belong to
    // this same parent; cleanup cannot turn a failed observation into a pass.
    eprintln!(
        "NATIVE_CLIENT_CENSUS {}",
        serde_json::json!({"parent":std::process::id(),"warmup":warmup,"baseline_fds":baseline_fds,"setup_error":failed_setup,"rows":rows})
    );
    std::io::stderr().flush().unwrap();
    let cleanup: Vec<_> = retained.iter_mut().map(OwnedPty::cleanup).collect();
    assert!(cleanup.iter().all(Result::is_ok), "{cleanup:?}");
    assert!(failed_setup.is_none(), "{failed_setup:?}");
    assert_eq!(rows.len(), 100);
    assert!(warmup
        .iter()
        .all(|row| row["cleanup"]["Ok"].is_null() && row["cleanup"].get("Err").is_none()));
    assert!(rows.iter().all(|row| row["release"].get("Err").is_none()
        && row["state"]["Ok"] == "reaped"
        && row["zombies"] == 0
        && row["running"] == 0
        && row["map_empty"] == true
        && row["repeated"].get("Err").is_none()
        && row["census_errors"].as_array().unwrap().is_empty()
        && row["fds"]["Ok"] == baseline_fds));
}

#[cfg(target_os = "macos")]
#[path = "herdr_native_client_setup_tests.rs"]
mod setup_checks;
