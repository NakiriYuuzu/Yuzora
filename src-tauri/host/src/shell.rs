//! Account shell selection shared by native terminals and host processes.
use std::path::PathBuf;

pub fn resolve_shell(override_shell: Option<&str>) -> PathBuf {
    resolve_shell_from(
        override_shell,
        std::env::var_os("SHELL").map(PathBuf::from),
        passwd_shell(),
    )
}

pub fn resolve_shell_from(
    override_shell: Option<&str>,
    env_shell: Option<PathBuf>,
    fallback_shell: Option<PathBuf>,
) -> PathBuf {
    if let Some(shell) = override_shell.filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(shell);
    }
    if let Some(shell) = env_shell {
        return shell;
    }
    fallback_shell.unwrap_or_else(default_shell)
}

#[cfg(unix)]
fn passwd_shell() -> Option<PathBuf> {
    use std::ffi::CStr;

    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() {
        return None;
    }
    let shell = unsafe { CStr::from_ptr((*entry).pw_shell) };
    let path = PathBuf::from(shell.to_string_lossy().into_owned());
    if is_executable(&path) {
        Some(path)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn passwd_shell() -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"))
    }
    #[cfg(all(unix, target_os = "macos"))]
    {
        PathBuf::from("/bin/zsh")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/bin/sh")
    }
}
