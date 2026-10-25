use super::{
    read_log_enabled_from, read_log_level_from, write_log_enabled_to, write_log_level_to, LogSink,
};
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_TEST: &str = "logging::settings_lifecycle_tests::limited_settings_child";
const ORIGINAL: &[u8] = br#"{"minLevel":"error","enabled":false}"#;

thread_local! {
    static WRITE_FAILURE: Cell<Option<&'static str>> = const { Cell::new(None) };
}

pub(super) fn fail_write_at(stage: &str) -> std::io::Result<()> {
    if WRITE_FAILURE.get() == Some(stage) {
        return Err(std::io::Error::other(format!("injected {stage} failure")));
    }
    Ok(())
}

pub(super) fn prepare_persist(temporary: &tempfile::NamedTempFile) -> std::io::Result<()> {
    if WRITE_FAILURE.get() == Some("persist-missing-source") {
        // Only remove this test's temporary pathname. The actual persist call
        // must return its real OS error while the old target remains intact.
        fs::remove_file(temporary.path())?;
    }
    Ok(())
}

fn assert_no_temporary_files(root: &std::path::Path) {
    assert!(!fs::read_dir(root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".logging-")));
}

#[test]
fn limited_settings_child() {
    let Some(root) = std::env::var_os("YUZORA_EVAL_LOG_SETTINGS_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    assert_eq!(
        fs::read(root.join("owner")).unwrap(),
        b"049-owned-settings-fixture"
    );
    let field = std::env::var("YUZORA_EVAL_LOG_SETTINGS_FIELD").unwrap();
    let setter = field.starts_with("setter-");
    let path = if setter {
        super::log_config_path()
    } else {
        root.join("logging.json")
    };
    if setter {
        fs::write(&path, ORIGINAL).unwrap();
        super::apply_persisted_log_level();
    }
    if field == "setter-success" {
        super::set_log_level("warn".into()).unwrap();
        assert!(!read_log_enabled_from(&path));
        assert_eq!(read_log_level_from(&path), "warn");
        super::set_log_enabled(true).unwrap();
        assert_eq!(read_log_level_from(&path), "warn");
        assert!(read_log_enabled_from(&path));
        let sink = super::global_sink().lock().unwrap();
        assert!(sink.enabled);
        assert_eq!(sink.min_level, super::LEVEL_WARN);
        assert_no_temporary_files(path.parent().unwrap());
        println!("OWNED_SETTINGS_SUCCESS");
        return;
    }
    if field == "setter-concurrent" {
        for _ in 0..20 {
            super::set_log_level("error".into()).unwrap();
            super::set_log_enabled(false).unwrap();
            let barrier = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    super::set_log_enabled(true).unwrap();
                });
                barrier.wait();
                super::set_log_level("warn".into()).unwrap();
            });
            let sink = super::global_sink().lock().unwrap();
            assert!(sink.enabled && read_log_enabled_from(&path));
            assert_eq!(sink.min_level, super::LEVEL_WARN);
            assert_eq!(read_log_level_from(&path), "warn");
        }
        assert_no_temporary_files(path.parent().unwrap());
        println!("OWNED_SETTINGS_SUCCESS");
        return;
    }
    if field.contains("persist-missing-source") {
        WRITE_FAILURE.set(Some("persist-missing-source"));
    } else if field.contains("sync") {
        WRITE_FAILURE.set(Some("sync"));
    } else if field.contains("persist") {
        WRITE_FAILURE.set(Some("persist"));
    } else {
        let limit = libc::rlimit {
            rlim_cur: 8,
            rlim_max: 8,
        };
        unsafe {
            assert_ne!(libc::signal(libc::SIGXFSZ, libc::SIG_IGN), libc::SIG_ERR);
            assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
        }
    }
    let result = match field.as_str() {
        "level" => write_log_level_to(&path, "warn"),
        "enabled" => write_log_enabled_to(&path, true),
        "setter-level"
        | "setter-sync-level"
        | "setter-persist-level"
        | "setter-persist-missing-source-level" => super::set_log_level("warn".into()),
        "setter-enabled"
        | "setter-sync-enabled"
        | "setter-persist-enabled"
        | "setter-persist-missing-source-enabled" => super::set_log_enabled(true),
        _ => panic!("unknown settings field"),
    };
    assert!(result.is_err(), "expected settings write failure");
    println!("OWNED_SETTINGS_FAILED: {}", result.unwrap_err());
    if setter {
        assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
        let sink = super::global_sink().lock().unwrap();
        assert!(!sink.enabled);
        assert_eq!(sink.min_level, super::LEVEL_ERROR);
        let mut reopened = LogSink::new(root.join("reopened-logs"));
        reopened.apply_persisted_settings(&path);
        assert!(!reopened.enabled);
        assert_eq!(reopened.min_level, super::LEVEL_ERROR);
        assert_no_temporary_files(path.parent().unwrap());
    }
}

fn failed_update(field: &str) {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::write(root.join("owner"), b"049-owned-settings-fixture").unwrap();
    let path = root.join("logging.json");
    fs::write(&path, ORIGINAL).unwrap();
    for name in ["home", "config", "state", "cache", "run", "tmp"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let herdr = root.join("herdr.toml");
    fs::write(
        &herdr,
        "onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n",
    )
    .unwrap();
    let git_ignore = root.join("empty.gitignore");
    let git_config = root.join("gitconfig");
    fs::write(&git_ignore, "").unwrap();
    fs::write(
        &git_config,
        format!("[core]\n\texcludesFile = {}\n", git_ignore.display()),
    )
    .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            !key.to_string_lossy().starts_with("HERDR_")
                && !key
                    .to_string_lossy()
                    .starts_with("YUZORA_EVAL_LOG_SETTINGS_")
        }));
    command
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env("YUZORA_EVAL_LOG_SETTINGS_ROOT", root)
        .env("YUZORA_EVAL_LOG_SETTINGS_FIELD", field)
        .env("HOME", root.join("home"))
        .env("HERDR_CONFIG_PATH", herdr)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("TMPDIR", root.join("tmp"))
        .env("GIT_CONFIG_GLOBAL", git_config)
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("owned settings child deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(
            if field == "setter-success" || field == "setter-concurrent" {
                "OWNED_SETTINGS_SUCCESS"
            } else {
                "OWNED_SETTINGS_FAILED:"
            }
        ),
        "child body did not run: {stdout}"
    );
    let mut reopened = LogSink::new(root.join("logs"));
    reopened.apply_persisted_settings(&path);
    eprintln!(
        "field={field}, child={stdout}, bytes={}, restartEnabled={}, restartLevel={}",
        fs::metadata(&path).unwrap().len(),
        read_log_enabled_from(&path),
        read_log_level_from(&path)
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        ORIGINAL,
        "failed update must preserve previous settings"
    );
    assert!(!reopened.enabled, "restart must remain disabled");
    assert_eq!(reopened.min_level, super::LEVEL_ERROR);
    assert_eq!(read_log_level_from(&path), "error");
    assert!(!fs::read_dir(root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".logging-")));
}

#[test]
fn failed_level_update_preserves_disabled_restart_settings() {
    failed_update("level");
}

#[test]
fn failed_enabled_update_preserves_disabled_restart_settings() {
    failed_update("enabled");
}

#[test]
fn setters_preserve_disk_and_memory_on_precommit_failures() {
    for field in [
        "setter-level",
        "setter-enabled",
        "setter-sync-level",
        "setter-sync-enabled",
        "setter-persist-level",
        "setter-persist-enabled",
        "setter-persist-missing-source-level",
        "setter-persist-missing-source-enabled",
    ] {
        failed_update(field);
    }
}

#[test]
fn successful_setters_preserve_other_setting_and_publish_memory() {
    failed_update("setter-success");
}

#[test]
fn concurrent_setters_preserve_both_fields() {
    failed_update("setter-concurrent");
}

#[test]
fn settings_replacement_preserves_permissions_and_symlink_target() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("target.json");
    let alias = fixture.path().join("logging.json");
    fs::write(&target, ORIGINAL).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
    symlink("target.json", &alias).unwrap();
    write_log_level_to(&alias, "warn").unwrap();
    assert!(fs::symlink_metadata(&alias)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_link(&alias).unwrap(),
        std::path::Path::new("target.json")
    );
    assert_eq!(read_log_level_from(&target), "warn");
    assert!(!read_log_enabled_from(&alias));
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
    write_log_enabled_to(&alias, true).unwrap();
    assert_eq!(read_log_level_from(&target), "warn");
    assert!(read_log_enabled_from(&target));
    assert_no_temporary_files(fixture.path());
}

#[test]
fn settings_create_private_file_and_reject_dangling_alias() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("nested/logging.json");
    write_log_enabled_to(&path, false).unwrap();
    assert!(!read_log_enabled_from(&path));
    assert_eq!(read_log_level_from(&path), "info");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_no_temporary_files(path.parent().unwrap());
    let alias = fixture.path().join("alias.json");
    symlink("absent.json", &alias).unwrap();
    assert!(write_log_level_to(&alias, "warn").is_err());
    assert!(fs::symlink_metadata(&alias)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!fixture.path().join("absent.json").exists());
    assert_no_temporary_files(fixture.path());
}

#[test]
fn readonly_settings_cannot_be_replaced_via_writable_parent() {
    // Root bypasses Unix mode checks; this control requires an ordinary user.
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "readonly control requires an ordinary user"
    );
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("logging.json");
    fs::write(&path, ORIGINAL).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(write_log_level_to(&path, "warn").is_err());
    assert!(write_log_enabled_to(&path, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
    assert_no_temporary_files(fixture.path());
}
