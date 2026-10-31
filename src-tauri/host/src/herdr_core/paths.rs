//! Where a session's core listens, resolved the way upstream `paths.rs` and
//! `platform/{unix,windows}.rs` do (0.1.14), so Yuzora finds the same endpoint `hc` would
//! without running it.
//!
//! Yuzora never reads `HC_SESSION`, `HC_SOCKET` or the config file's `socket` setting: it
//! names the session itself, computes the endpoint here, and pins the child core to it with
//! `HC_SOCKET` (see `startup::core_command`).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Unix { max_socket_path: usize },
    Windows,
}

/// macOS `sun_path` is 104 bytes (103 + NUL); Linux 108.
pub const MACOS: Os = Os::Unix {
    max_socket_path: 103,
};
pub const LINUX: Os = Os::Unix {
    max_socket_path: 107,
};
/// Hex digits in a holder socket name.
pub const HOLDER_ID: usize = 8;
/// Windows pipe names: up to 256 characters.
const WINDOWS_MAX_PIPE_PATH: usize = 256;
/// Where Windows pipes live. `herdr_transport` adds this itself, so it is never passed down.
const WINDOWS_PIPE_NAMESPACE: &str = r"\\.\pipe\";

impl Os {
    pub fn current() -> Os {
        #[cfg(unix)]
        {
            // SAFETY: an all-zero `sockaddr_un` is a valid value; only its array length is read.
            let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            Os::Unix {
                max_socket_path: addr.sun_path.len() - 1,
            }
        }
        #[cfg(not(unix))]
        {
            Os::Windows
        }
    }
}

/// The inputs upstream reads from the environment and the filesystem, passed in explicitly.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub home: Option<String>,
    pub xdg_state_home: Option<String>,
    pub xdg_runtime_dir: Option<String>,
    pub uid: u32,
    /// `/run/user/<uid>` exists (Linux).
    pub run_user_exists: bool,
    pub username: Option<String>,
    pub localappdata: Option<String>,
    pub userprofile: Option<String>,
}

impl Env {
    /// This process's view. Never `HC_*`.
    pub fn from_process() -> Env {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let uid = current_uid();
        Env {
            home: var("HOME"),
            xdg_state_home: var("XDG_STATE_HOME"),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
            uid,
            run_user_exists: Path::new(&format!("/run/user/{uid}")).is_dir(),
            username: var("USERNAME"),
            localappdata: var("LOCALAPPDATA"),
            userprofile: var("USERPROFILE"),
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub session: String,
    /// What the core listens on, and what `HC_SOCKET` is set to. On Windows this is the full
    /// `\\.\pipe\…` path.
    pub socket: PathBuf,
    pub holder_socket: PathBuf,
    pub state: PathBuf,
}

impl Endpoint {
    /// The string to hand to `herdr_transport::connect_local_stream`. Windows adds the
    /// `\\.\pipe\` namespace itself, so a pipe is passed as its bare name
    /// (`herdr-core-<user>-<session>.sock`); a full path would be prefixed twice.
    pub fn transport_name(&self) -> String {
        let socket = self.socket.to_string_lossy();
        socket
            .strip_prefix(WINDOWS_PIPE_NAMESPACE)
            .unwrap_or(&socket)
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    InvalidSession(String),
    SocketPathTooLong {
        session: String,
        bytes: usize,
        max: usize,
    },
}

impl PathError {
    pub fn code(&self) -> &'static str {
        match self {
            PathError::InvalidSession(_) => "invalid_session",
            PathError::SocketPathTooLong { .. } => "socket_path_too_long",
        }
    }
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::InvalidSession(name) => {
                write!(f, "session {name:?} can't be a folder name")
            }
            PathError::SocketPathTooLong {
                session,
                bytes,
                max,
            } => write!(
                f,
                "the socket path for session {session:?} is too long here ({bytes} bytes; this system allows {max})"
            ),
        }
    }
}

impl std::error::Error for PathError {}

/// A session name Yuzora can pass as `HC_SESSION`: non-empty, no surrounding spaces (upstream
/// trims them, which would silently name another session), nothing a folder or pipe name
/// cannot hold.
pub fn validate_session(name: &str) -> Result<&str, PathError> {
    let bad = name.is_empty()
        || name.trim() != name
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || r#"/\:*?"<>|"#.contains(c));
    if bad {
        Err(PathError::InvalidSession(name.to_string()))
    } else {
        Ok(name)
    }
}

pub fn resolve(os: Os, env: &Env, session: &str) -> Result<Endpoint, PathError> {
    let session = validate_session(session)?.to_string();
    let sep = match os {
        Os::Unix { .. } => '/',
        Os::Windows => '\\',
    };
    let default_state_home = match os {
        Os::Unix { .. } => Path::new(env.home.as_deref().unwrap_or("/")).join(".local/state"),
        Os::Windows => match nonempty(&env.localappdata) {
            Some(local) => PathBuf::from(local),
            None => PathBuf::from(format!(
                r"{}\AppData\Local",
                nonempty(&env.userprofile)
                    .unwrap_or(r"C:\")
                    .trim_end_matches('\\')
            )),
        },
    };
    let state_home = nonempty(&env.xdg_state_home)
        .map(PathBuf::from)
        .unwrap_or(default_state_home);
    let join = |base: &Path, parts: &[&str]| -> PathBuf {
        let mut joined = base.to_string_lossy().trim_end_matches(sep).to_string();
        for part in parts {
            joined.push(sep);
            joined.push_str(part);
        }
        PathBuf::from(joined)
    };
    let state = join(&state_home, &["herdr-core", &session]);
    let (run, prefix): (PathBuf, String) = match os {
        Os::Unix { .. } => {
            let run = nonempty(&env.xdg_runtime_dir)
                .map(PathBuf::from)
                .or_else(|| {
                    env.run_user_exists
                        .then(|| PathBuf::from(format!("/run/user/{}", env.uid)))
                });
            (
                run.map_or_else(|| state.clone(), |r| join(&r, &["herdr-core"])),
                String::new(),
            )
        }
        Os::Windows => {
            let user: String = env
                .username
                .as_deref()
                .unwrap_or_default()
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .collect();
            (
                PathBuf::from(WINDOWS_PIPE_NAMESPACE),
                format!("herdr-core-{user}-"),
            )
        }
    };
    let socket = join(&run, &[&format!("{prefix}{session}.sock")]);
    let holder_socket = join(&run, &[&format!("{prefix}{session}.holder.sock")]);
    // Holders listen on `<holder_socket minus .sock>.<id>.sock`: the longest path decides.
    let longest = holder_socket.as_os_str().len() + 1 + HOLDER_ID;
    let max = match os {
        Os::Unix { max_socket_path } => max_socket_path,
        Os::Windows => WINDOWS_MAX_PIPE_PATH,
    };
    if socket.as_os_str().len().max(longest) > max {
        return Err(PathError::SocketPathTooLong {
            session,
            bytes: longest,
            max,
        });
    }
    Ok(Endpoint {
        session,
        socket,
        holder_socket,
        state,
    })
}

fn nonempty(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(home: &str) -> Env {
        Env {
            home: Some(home.into()),
            uid: 501,
            ..Env::default()
        }
    }

    #[test]
    fn macos_default_lives_in_the_state_dir() {
        // macOS has no XDG_RUNTIME_DIR and no /run/user: the socket sits in the state dir
        // (the layout a running `hc` uses: ~/.local/state/herdr-core/default/default.sock).
        let e = resolve(MACOS, &mac("/Users/yuuzu"), "default").unwrap();
        assert_eq!(
            e.socket,
            PathBuf::from("/Users/yuuzu/.local/state/herdr-core/default/default.sock")
        );
        assert_eq!(
            e.holder_socket,
            PathBuf::from("/Users/yuuzu/.local/state/herdr-core/default/default.holder.sock")
        );
        let yuzora = resolve(MACOS, &mac("/Users/yuuzu"), "yuzora").unwrap();
        assert_eq!(
            yuzora.socket,
            PathBuf::from("/Users/yuuzu/.local/state/herdr-core/yuzora/yuzora.sock")
        );
        assert_eq!(yuzora.transport_name(), yuzora.socket.to_string_lossy());
    }

    #[test]
    fn runtime_dir_and_state_home_follow_upstream() {
        let mut env = mac("/Users/yuuzu");
        env.xdg_runtime_dir = Some("/tmp/yz/x".into());
        env.xdg_state_home = Some("/tmp/yz/xdg/state".into());
        let e = resolve(MACOS, &env, "yzs8a").unwrap();
        assert_eq!(e.socket, PathBuf::from("/tmp/yz/x/herdr-core/yzs8a.sock"));
        assert_eq!(e.state, PathBuf::from("/tmp/yz/xdg/state/herdr-core/yzs8a"));
        // Empty variables count as unset, like upstream.
        env.xdg_runtime_dir = Some(String::new());
        env.xdg_state_home = Some(String::new());
        assert_eq!(
            resolve(MACOS, &env, "yzs8a").unwrap().socket,
            PathBuf::from("/Users/yuuzu/.local/state/herdr-core/yzs8a/yzs8a.sock")
        );
        // Linux without XDG_RUNTIME_DIR uses /run/user/<uid> when it exists.
        let mut linux = mac("/home/u");
        linux.run_user_exists = true;
        assert_eq!(
            resolve(LINUX, &linux, "yzs8a").unwrap().socket,
            PathBuf::from("/run/user/501/herdr-core/yzs8a.sock")
        );
    }

    #[test]
    fn session_names_are_checked_not_trimmed() {
        for ok in ["yuzora", "yuzora-dev", "default", "MyTest", "a.b_c"] {
            assert_eq!(validate_session(ok), Ok(ok));
        }
        for bad in [
            "", " ", " yuzora", "yuzora ", "a/b", r"a\b", "a:b", "..", ".", "a\tb", "a|b",
        ] {
            assert!(
                matches!(validate_session(bad), Err(PathError::InvalidSession(_))),
                "{bad:?}"
            );
        }
        assert!(resolve(MACOS, &mac("/Users/yuuzu"), "a/b").is_err());
    }

    #[test]
    fn long_paths_are_refused_like_upstream_holder_suffix_included() {
        // Holder sockets are `<session>.holder.<8 hex>.sock`: the longest path decides.
        let ok = resolve(MACOS, &mac("/Users/yuuzu"), "yuzora").unwrap();
        let longest = ok.holder_socket.as_os_str().len() + 1 + HOLDER_ID;
        assert_eq!(longest, 71, "HOME=/Users/yuuzu, session yuzora");
        let home = |len: usize| mac(&format!("/Users/{}", "x".repeat(len - "/Users/".len())));
        // The exact limit for session `yuzora`: HOME of 44 bytes fits (103), 45 does not (104).
        let fits = resolve(MACOS, &home(44), "yuzora").unwrap();
        assert_eq!(fits.holder_socket.as_os_str().len() + 1 + HOLDER_ID, 103);
        // The plain socket path alone (97 bytes here) is shorter: only the holder suffix fails.
        assert!(fits.socket.as_os_str().len() < 103);
        assert_eq!(
            resolve(MACOS, &home(45), "yuzora"),
            Err(PathError::SocketPathTooLong {
                session: "yuzora".into(),
                bytes: 104,
                max: 103
            })
        );
        // Session `default` is one byte longer and appears twice in the path: HOME of 42
        // fits, 43 does not.
        assert!(resolve(MACOS, &home(42), "default").is_ok());
        assert!(resolve(MACOS, &home(43), "default").is_err());
        // The shorter session name buys space; the debug session `yuzora-dev` needs a shorter HOME.
        assert!(resolve(MACOS, &home(44), "yuzora-dev").is_err());
        assert_eq!(
            PathError::SocketPathTooLong {
                session: String::new(),
                bytes: 0,
                max: 0
            }
            .code(),
            "socket_path_too_long"
        );
    }

    #[test]
    fn windows_pipe_names_are_bare_for_the_transport() {
        let env = Env {
            username: Some("runner.admin".into()),
            localappdata: Some(r"C:\Users\runneradmin\AppData\Local".into()),
            ..Env::default()
        };
        let e = resolve(Os::Windows, &env, "yzs8w").unwrap();
        // What the core listens on (and HC_SOCKET) is the full pipe path ...
        assert_eq!(
            e.socket,
            PathBuf::from(r"\\.\pipe\herdr-core-runneradmin-yzs8w.sock")
        );
        assert_eq!(
            e.holder_socket,
            PathBuf::from(r"\\.\pipe\herdr-core-runneradmin-yzs8w.holder.sock")
        );
        assert_eq!(
            e.state,
            PathBuf::from(r"C:\Users\runneradmin\AppData\Local\herdr-core\yzs8w")
        );
        // ... but herdr_transport prepends `\\.\pipe\` itself, so it gets the bare name.
        assert_eq!(e.transport_name(), "herdr-core-runneradmin-yzs8w.sock");
        assert!(!e.transport_name().contains('\\'));
        let default = resolve(
            Os::Windows,
            &Env {
                username: Some("yuuzu".into()),
                ..Env::default()
            },
            "default",
        )
        .unwrap();
        assert_eq!(default.transport_name(), "herdr-core-yuuzu-default.sock");
        // No LOCALAPPDATA: the profile's AppData\Local, like upstream.
        let profile = Env {
            userprofile: Some(r"C:\Users\me".into()),
            ..Env::default()
        };
        assert_eq!(
            resolve(Os::Windows, &profile, "yuzora").unwrap().state,
            PathBuf::from(r"C:\Users\me\AppData\Local\herdr-core\yuzora")
        );
    }

    #[test]
    fn current_os_matches_this_build() {
        #[cfg(unix)]
        match Os::current() {
            Os::Unix { max_socket_path } => assert!(
                max_socket_path == 103 || max_socket_path == 107,
                "{max_socket_path}"
            ),
            Os::Windows => panic!("a Unix build resolved the Windows layout"),
        }
        #[cfg(windows)]
        assert_eq!(Os::current(), Os::Windows);
    }
}
