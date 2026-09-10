//! Account shell selection shared by native terminals and host processes.
use std::path::PathBuf;

#[cfg(any(windows, test))]
fn windows_shell_cwd_candidate(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    if rest
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\"))
    {
        return Some(format!(r"\\{}", &rest[4..]));
    }
    let bytes = rest.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| rest.to_owned())
}

/// PowerShell treats a verbatim device path as a provider-qualified location.
/// Use Win32 cwd syntax only when it resolves to exactly the same directory;
/// identity-sensitive filesystem/trust paths remain canonical elsewhere.
pub fn working_directory(path: String) -> String {
    #[cfg(windows)]
    if let Some(candidate) = windows_shell_cwd_candidate(&path) {
        if let (Ok(original), Ok(simplified)) = (
            std::fs::canonicalize(&path),
            std::fs::canonicalize(&candidate),
        ) {
            if original == simplified {
                return candidate;
            }
        }
    }
    path
}

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

#[cfg(test)]
mod cwd_tests {
    use super::*;

    #[test]
    fn windows_shell_cwd_uses_drive_or_unc_syntax_instead_of_a_device_namespace() {
        for (canonical, expected) in [
            (r"\\?\C:\Work\中文 project", r"C:\Work\中文 project"),
            (r"\\?\C:\", r"C:\"),
            (r"\\?\UNC\server\share\project", r"\\server\share\project"),
        ] {
            assert_eq!(
                windows_shell_cwd_candidate(canonical).as_deref(),
                Some(expected)
            );
        }
        for path in [
            "/home/me/project",
            r"C:\Work",
            r"\\server\share",
            r"\\.\pipe\name",
            r"\\?\Volume{guid}\",
        ] {
            assert_eq!(windows_shell_cwd_candidate(path), None);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_cwd_preserves_the_actual_directory_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("中文 project");
        std::fs::create_dir(&folder).unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        let cwd = working_directory(canonical.to_str().unwrap().to_owned());
        assert!(!cwd.starts_with(r"\\?\"));
        assert_eq!(std::fs::canonicalize(cwd).unwrap(), canonical);
        // A path that cannot be verified must never silently change identity.
        let missing = r"\\?\C:\yuzora-missing-fixture-5fbd6513".to_owned();
        assert_eq!(working_directory(missing.clone()), missing);
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shell_cwd_is_not_reinterpreted_as_a_windows_path() {
        for path in ["/home/me/中文 project", r"/home/me/\\?\C:\literal"] {
            assert_eq!(working_directory(path.to_owned()), path);
        }
    }
}
