//! Windows installers cannot replace an executable that is still running. The
//! update confirmation lists every process of the HERDR binary this app is
//! currently using and of the bundled one the installer replaces, and only
//! after the user confirms losing that work are those process trees terminated
//! so the installer can replace the files.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::herdr_service::HerdrState;
use crate::process_kill;

const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHerdrProcesses {
    path: Option<String>,
    version: Option<String>,
    pids: Vec<u32>,
}

/// Comparable path text: Windows paths drop the verbatim prefix and fold case,
/// so canonical and non-canonical spellings of one file compare equal.
fn fold_path(text: &str, windows: bool) -> String {
    if !windows {
        return text.to_owned();
    }
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(text).to_owned(),
    };
    text.to_lowercase()
}

fn comparable(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    fold_path(&path.to_string_lossy(), cfg!(windows))
}

/// The HERDR binary in use plus the ConPTY hosts shipped beside it
/// (`conpty/<arch>/OpenConsole.exe`), which outlive a crashed server.
struct HerdrExecutables {
    binary: String,
    binary_name: String,
    conpty_dir: String,
}

impl HerdrExecutables {
    fn new(binary: &Path) -> Self {
        let canonical = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
        let conpty_dir = canonical
            .parent()
            .map(|dir| comparable(&dir.join("conpty")) + std::path::MAIN_SEPARATOR_STR)
            .unwrap_or_default();
        Self {
            binary: comparable(binary),
            binary_name: file_name(binary),
            conpty_dir,
        }
    }

    fn matches(&self, exe: &Path) -> bool {
        // Cheap file-name filter first; only HERDR candidates pay for canonicalize.
        let name = file_name(exe);
        let console = name == fold_path("OpenConsole.exe", cfg!(windows));
        if name != self.binary_name && !console {
            return false;
        }
        let exe = comparable(exe);
        exe == self.binary || (console && exe.starts_with(&self.conpty_dir))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| fold_path(&name.to_string_lossy(), cfg!(windows)))
        .unwrap_or_default()
}

/// Running HERDR processes, excluding this app, as `(pid, parent)`.
fn binary_processes(system: &mut System, binary: &Path) -> Vec<(u32, Option<u32>)> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let own = get_current_pid().ok();
    let executables = HerdrExecutables::new(binary);
    let mut processes: Vec<_> = system
        .processes()
        .iter()
        .filter(|(pid, _)| Some(**pid) != own)
        .filter(|(_, process)| process.exe().is_some_and(|exe| executables.matches(exe)))
        .map(|(pid, process)| (pid.as_u32(), process.parent().map(Pid::as_u32)))
        .collect();
    processes.sort_unstable();
    processes
}

/// Kill the root of every matching process tree until none remain. Descendants
/// (panes, OpenConsole) go with their root; anything that reappears is retried.
fn stop_binary_processes(
    binary: &Path,
    kill_tree: impl Fn(u32) -> std::io::Result<()>,
    timeout: Duration,
) -> Result<(), String> {
    let mut system = System::new();
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    loop {
        let running = binary_processes(&mut system, binary);
        if running.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let pids: Vec<_> = running.iter().map(|(pid, _)| pid.to_string()).collect();
            let detail = last_error.map_or_else(String::new, |error| format!(" ({error})"));
            return Err(format!(
                "HERDR processes are still running: {}{detail}",
                pids.join(", ")
            ));
        }
        for &(pid, parent) in &running {
            if parent.is_some_and(|parent| running.iter().any(|(other, _)| *other == parent)) {
                continue;
            }
            if let Err(error) = kill_tree(pid) {
                last_error = Some(format!("{pid}: {error}"));
            }
        }
        std::thread::sleep(STOP_POLL_INTERVAL);
    }
}

/// The HERDR in use plus the bundled one, which a server started before
/// switching to another source can still hold open.
fn update_binaries(active: Option<PathBuf>, managed: Option<PathBuf>) -> Vec<PathBuf> {
    let mut binaries: Vec<PathBuf> = Vec::new();
    for binary in [active, managed].into_iter().flatten() {
        if !binaries
            .iter()
            .any(|seen| comparable(seen) == comparable(&binary))
        {
            binaries.push(binary);
        }
    }
    binaries
}

#[tauri::command]
pub async fn update_herdr_processes(
    state: tauri::State<'_, HerdrState>,
) -> Result<UpdateHerdrProcesses, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let info = manager.binary_source_info();
        let mut system = System::new();
        let mut pids: Vec<u32> =
            update_binaries(manager.resolve_binary(), manager.managed_binary())
                .iter()
                .flat_map(|binary| binary_processes(&mut system, binary))
                .map(|(pid, _)| pid)
                .collect();
        pids.sort_unstable();
        pids.dedup();
        UpdateHerdrProcesses {
            path: info.path,
            version: info.version,
            pids,
        }
    })
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn update_stop_herdr(state: tauri::State<'_, HerdrState>) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let binaries = update_binaries(manager.resolve_binary(), manager.managed_binary());
        if binaries.is_empty() {
            return Ok(());
        }
        manager.release_all_connectors();
        for binary in &binaries {
            stop_binary_processes(binary, process_kill::kill_tree_pid, STOP_TIMEOUT)?;
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Child, Command};

    /// Run a copy of `sleep` from `binary`, which may sit in nested directories.
    fn spawn_copy(binary: &Path) -> Child {
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::copy("/bin/sleep", binary).unwrap();
        // macOS kills a relocated platform binary unless it is re-signed.
        if cfg!(target_os = "macos") {
            let signed = Command::new("codesign")
                .args(["-s", "-", "-f"])
                .arg(binary)
                .output()
                .unwrap();
            assert!(signed.status.success(), "{signed:?}");
        }
        Command::new(binary).arg("30").spawn().unwrap()
    }

    fn wait_until_listed(binary: &Path, pid: u32) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !binary_processes(&mut System::new(), binary)
            .iter()
            .any(|(listed, _)| *listed == pid)
        {
            assert!(Instant::now() < deadline, "process {pid} was never listed");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn sigkill(pid: u32) -> std::io::Result<()> {
        let status = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("kill failed"))
        }
    }

    #[test]
    fn update_covers_the_bundled_binary_beside_another_selected_source() {
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("custom-herdr");
        let bundled = dir.path().join("bundled-herdr");
        let alias = dir.path().join("bundled-alias");
        std::fs::write(&custom, "").unwrap();
        std::fs::write(&bundled, "").unwrap();
        std::os::unix::fs::symlink(&bundled, &alias).unwrap();

        assert_eq!(
            update_binaries(Some(custom.clone()), Some(bundled.clone())),
            vec![custom, bundled.clone()]
        );
        assert_eq!(
            update_binaries(Some(alias.clone()), Some(bundled.clone())),
            vec![alias]
        );
        assert_eq!(update_binaries(None, Some(bundled.clone())), vec![bundled]);
        assert!(update_binaries(None, None).is_empty());
    }

    #[test]
    fn lists_only_processes_of_the_selected_binary() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr");
        let mut selected = spawn_copy(&binary);
        let mut other = Command::new("/bin/sleep").arg("30").spawn().unwrap();

        wait_until_listed(&binary, selected.id());
        let pids: Vec<_> = binary_processes(&mut System::new(), &binary)
            .into_iter()
            .map(|(pid, _)| pid)
            .collect();
        assert_eq!(pids, vec![selected.id()]);

        let _ = selected.kill();
        let _ = other.kill();
        let _ = selected.wait();
        let _ = other.wait();
    }

    #[test]
    fn lists_conpty_hosts_shipped_beside_the_selected_binary() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr");
        std::fs::copy("/bin/sleep", &binary).unwrap();
        let mut console = spawn_copy(&dir.path().join("conpty/x64/OpenConsole.exe"));
        let mut unrelated = spawn_copy(&other.path().join("conpty/x64/OpenConsole.exe"));

        wait_until_listed(&binary, console.id());
        // The unrelated host is running and visible; it belongs to another HERDR.
        wait_until_listed(&other.path().join("herdr"), unrelated.id());
        let pids: Vec<_> = binary_processes(&mut System::new(), &binary)
            .into_iter()
            .map(|(pid, _)| pid)
            .collect();
        assert_eq!(pids, vec![console.id()]);

        let _ = console.kill();
        let _ = unrelated.kill();
        let _ = console.wait();
        let _ = unrelated.wait();
    }

    #[test]
    fn windows_paths_compare_without_verbatim_prefix_or_case() {
        assert_eq!(
            fold_path(r"\\?\C:\Program Files\Yuzora\herdr\herdr.exe", true),
            fold_path(r"C:\program files\yuzora\HERDR\herdr.EXE", true)
        );
        assert_eq!(
            fold_path(r"\\?\UNC\server\share\herdr.exe", true),
            fold_path(r"\\server\share\herdr.exe", true)
        );
        assert_ne!(
            fold_path("/opt/Herdr", false),
            fold_path("/opt/herdr", false)
        );
    }

    #[test]
    fn stops_every_process_of_the_selected_binary() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr");
        let selected = spawn_copy(&binary);
        let pid = selected.id();
        wait_until_listed(&binary, pid);
        // Reap concurrently so the killed child cannot linger as a zombie.
        let reaper = std::thread::spawn(move || {
            let mut selected = selected;
            selected.wait()
        });

        stop_binary_processes(&binary, sigkill, Duration::from_secs(5)).unwrap();

        assert!(binary_processes(&mut System::new(), &binary).is_empty());
        reaper.join().unwrap().unwrap();
    }

    #[test]
    fn reports_processes_that_survive_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr");
        let mut selected = spawn_copy(&binary);
        wait_until_listed(&binary, selected.id());

        let error =
            stop_binary_processes(&binary, |_| Ok(()), Duration::from_millis(300)).unwrap_err();
        assert!(error.contains(&selected.id().to_string()), "{error}");

        let _ = selected.kill();
        let _ = selected.wait();
    }
}
