//! Resolve account tooling locally on the host, without contaminating RPC stdout.
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const START: &[u8] = b"\0YUZORA_ENV_START\0";
const END: &[u8] = b"\0YUZORA_ENV_END\0";
const PROBE: &str =
    "printf '\\000YUZORA_ENV_START\\000'; /usr/bin/env -0; printf '\\000YUZORA_ENV_END\\000'";

/// Call only before starting the helper's Tokio runtime or any other threads.
/// The account's login environment stays on this host and is never returned by RPC.
pub fn initialize() -> Result<(), &'static str> {
    let home = dirs::home_dir().ok_or("account-home-unavailable")?;
    let shell = crate::shell::resolve_shell(None);
    if !shell.is_absolute() {
        return Err("account-shell-not-absolute");
    }
    let mut command = Command::new(shell);
    command.args(["-ilc", PROBE]).current_dir(home);
    let captured = capture(&mut command, Duration::from_secs(5), 256 * 1024)?;
    for (key, value) in parse(&captured)? {
        if key == "PATH" {
            let path = merge_path(&value, std::env::var_os("PATH").as_deref());
            if !path.is_empty() {
                std::env::set_var("PATH", path);
            }
        } else if import_key(&key) && std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
        }
    }
    Ok(())
}

struct Probe(Child);
impl Drop for Probe {
    fn drop(&mut self) {
        // Also end descendants retaining the capture socket on timeout/overflow.
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn capture(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    let (mut reader, writer) = UnixStream::pair().map_err(|_| "environment-pipe-failed")?;
    reader
        .set_nonblocking(true)
        .map_err(|_| "environment-pipe-failed")?;
    // WSL exec can retain a controlling terminal. A background process group
    // alone makes interactive bash stop itself with SIGTTIN before the probe.
    // A new session detaches that terminal while retaining group cancellation.
    crate::process_kill::configure_new_group(command);
    let mut child = Probe(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "environment-shell-failed")?,
    );
    let deadline = Instant::now() + timeout;
    let mut output = Zeroizing::new(Vec::new());
    let mut buffer = Zeroizing::new([0u8; 8192]);
    loop {
        if Instant::now() >= deadline {
            return Err("environment-shell-timeout");
        }
        match reader.read(&mut *buffer) {
            Ok(0) => match child.0.try_wait().map_err(|_| "environment-shell-failed")? {
                Some(status) if status.success() => return Ok(output),
                Some(_) => return Err("environment-shell-failed"),
                None => std::thread::sleep(Duration::from_millis(5)),
            },
            Ok(size) => {
                if output.len() + size > limit {
                    return Err("environment-output-limit");
                }
                output.extend_from_slice(&buffer[..size]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                // A shell's background children may retain stdout after it exits.
                if let Some(status) = child.0.try_wait().map_err(|_| "environment-shell-failed")? {
                    return if status.success() {
                        Ok(output)
                    } else {
                        Err("environment-shell-failed")
                    };
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return Err("environment-read-failed"),
        }
    }
}

fn parse(output: &[u8]) -> Result<Vec<(OsString, OsString)>, &'static str> {
    let begin = output
        .windows(START.len())
        .position(|w| w == START)
        .ok_or("environment-markers-missing")?
        + START.len();
    let rest = &output[begin..];
    let end = rest
        .windows(END.len())
        .position(|w| w == END)
        .ok_or("environment-markers-missing")?;
    Ok(rest[..end]
        .split(|b| *b == 0)
        .filter_map(|entry| {
            let split = entry.iter().position(|b| *b == b'=')?;
            if split == 0 {
                return None;
            }
            Some((
                OsString::from_vec(entry[..split].to_vec()),
                OsString::from_vec(entry[split + 1..].to_vec()),
            ))
        })
        .collect())
}

fn import_key(key: &OsStr) -> bool {
    let bytes = key.as_bytes();
    !matches!(
        bytes,
        b"HOME" | b"USER" | b"LOGNAME" | b"SHELL" | b"PWD" | b"OLDPWD" | b"SHLVL" | b"_"
    ) && !bytes.starts_with(b"YUZORA_")
        && !bytes.starts_with(b"HERDR_")
}

fn merge_path(shell: &OsStr, inherited: Option<&OsStr>) -> OsString {
    let mut paths = Vec::new();
    for path in
        std::env::split_paths(shell).chain(inherited.into_iter().flat_map(std::env::split_paths))
    {
        if is_trusted_path_entry(Path::new(&path)) && !paths.contains(&path) {
            paths.push(path);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn account_probe_detaches_from_the_parent_terminal_session() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "read -r pid comm state ppid pgid sid rest < /proc/self/stat; printf '\\000YUZORA_ENV_START\\000SID=%s\\000\\000YUZORA_ENV_END\\000' \"$sid\""]);
        let vars = parse(&capture(&mut command, Duration::from_secs(2), 4096).unwrap()).unwrap();
        let parent_session = unsafe { libc::getsid(0) }.to_string();
        assert_ne!(vars[0].1, OsString::from(parent_session));
    }

    #[test]
    fn captures_nul_values_without_rpc_noise_or_losing_equals_and_unicode() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'rc noise\\n\\000YUZORA_ENV_START\\000PATH=/tools:/bin\\000TOKEN=a=b 中文\\nnext\\000\\000YUZORA_ENV_END\\000tail'"]);
        let bytes = capture(&mut command, Duration::from_secs(2), 4096).unwrap();
        let vars = parse(&bytes).unwrap();
        assert_eq!(
            vars,
            vec![
                ("PATH".into(), "/tools:/bin".into()),
                ("TOKEN".into(), "a=b 中文\nnext".into())
            ]
        );
    }

    #[test]
    fn bounds_hanging_and_flooding_account_shells() {
        let mut hanging = Command::new("/bin/sh");
        hanging.args(["-c", "sleep 30"]);
        assert_eq!(
            capture(&mut hanging, Duration::from_millis(50), 1024).unwrap_err(),
            "environment-shell-timeout"
        );
        let mut flooding = Command::new("/bin/sh");
        flooding.args(["-c", "while :; do printf '1234567890'; done"]);
        assert_eq!(
            capture(&mut flooding, Duration::from_secs(2), 1024).unwrap_err(),
            "environment-output-limit"
        );
    }

    #[test]
    fn rejects_missing_markers_and_preserves_account_and_runtime_identity() {
        assert!(parse(b"login failed").is_err());
        for key in [
            "HOME",
            "USER",
            "LOGNAME",
            "PWD",
            "YUZORA_DB_WORKER",
            "HERDR_SESSION",
        ] {
            assert!(!import_key(OsStr::new(key)));
        }
        assert!(import_key(OsStr::new("GIT_CONFIG_GLOBAL")));
        assert_eq!(
            merge_path(
                OsStr::new("/nvm/bin:.:relative:/bin:/a/../b"),
                Some(OsStr::new("/bin:/usr/bin"))
            ),
            "/nvm/bin:/bin:/usr/bin"
        );
    }
}

fn is_trusted_path_entry(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|part| {
            !matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
}
