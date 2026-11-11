//! Finding this app's herdr-core server.
//!
//! Where the server sits in the process tree depends on who started it. The
//! app's child `hc connect` starts `hc serve` when none runs, so then it's
//! the app's grandchild; one already running (or left after the app
//! restarted) runs under launchd / init. Its holder (`hc hold`) runs the
//! terminals. Several servers can run on one computer (one per
//! `HC_SESSION`), so a name match would guess. Instead the app's own
//! `hc` answers `hc server status --json` with the same environment the app
//! connected with: `{pid, holders: [{pid}], …}` for exactly that server.
//!
//! Asking starts a process, so the answer is kept: asked again only when the
//! server it named is gone or is another process now (pid reused: its start
//! time changed), after [`REFRESH`], or [`RETRY`] after a failed ask.

use std::time::{Duration, Instant};

/// How long an answer is trusted while its server stays the same process.
pub const REFRESH: Duration = Duration::from_secs(60);
/// How long after a failed ask to try again.
pub const RETRY: Duration = Duration::from_secs(10);
/// `hc server status` answers at once; this only bounds a stuck one.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// What `hc server status --json` says about the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub pid: u32,
    /// Its holders' pids. A holder left by a restart drains under launchd, no
    /// longer under the server, so it's walked from here too.
    pub holders: Vec<u32>,
}

impl Status {
    /// Where the core's trees start: the server, then its holders.
    pub fn roots(&self) -> Vec<u32> {
        let mut roots = vec![self.pid];
        roots.extend(self.holders.iter().copied().filter(|h| *h != self.pid));
        roots
    }
}

/// Reads `hc server status --json`.
pub fn parse(stdout: &str) -> Result<Status, String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|_| format!("hc server status: not JSON: {}", short(stdout)))?;
    if let Some(e) = v.get("error") {
        let message = e
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("hc server status failed");
        return Err(message.to_owned());
    }
    let pid = v
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 0);
    let pid = pid.ok_or_else(|| "hc server status: no pid".to_owned())?;
    let holders = v
        .get("holders")
        .and_then(serde_json::Value::as_array)
        .map(|hs| {
            hs.iter()
                .filter_map(|h| h.get("pid")?.as_u64()?.try_into().ok())
                .filter(|p: &u32| *p > 0)
                .collect()
        })
        .unwrap_or_default();
    Ok(Status { pid, holders })
}

fn short(s: &str) -> String {
    let s = s.trim();
    let cut: String = s.chars().take(120).collect();
    if cut.len() < s.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// Asks the app's `hc` (blocking; call it off the UI thread).
pub fn ask() -> Result<Status, String> {
    let program = yuzora_core::remote::hc_program()
        .to_string_lossy()
        .into_owned();
    let req = yuzora_core::remote::ExecRequest::new(program, ["server", "status", "--json"])
        .timeout(TIMEOUT)
        .max_output(1 << 20);
    let out = yuzora_core::remote::run(&req).map_err(|e| format!("hc server status: {e}"))?;
    if out.code != 0 {
        // hc's own errors come as JSON on stderr.
        let detail = out.stderr_text();
        return Err(match parse(&detail) {
            Err(e) if !detail.trim().is_empty() => e,
            _ => format!("hc server status exited {}", out.code),
        });
    }
    parse(&out.stdout_text())
}

/// The kept answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Known {
    #[default]
    Nothing,
    Found {
        status: Status,
        /// The server's start time the first time it was seen after asking
        /// (sysinfo's, seconds): a pid reused by another process differs.
        start: Option<u64>,
        at: Instant,
    },
    Failed {
        error: String,
        at: Instant,
    },
}

impl Known {
    /// Whether to ask again. `server_start`: the start time of the process
    /// now at the kept server's pid, None when there's none.
    pub fn stale(&self, now: Instant, server_start: Option<u64>) -> bool {
        match self {
            Known::Nothing => true,
            Known::Failed { at, .. } => now.duration_since(*at) >= RETRY,
            Known::Found { start, at, .. } => {
                server_start.is_none()
                    || (start.is_some() && *start != server_start)
                    || now.duration_since(*at) >= REFRESH
            }
        }
    }

    /// Remembers the server's start time once it's been seen.
    pub fn seen(&mut self, server_start: Option<u64>) {
        if let Known::Found { start, .. } = self
            && start.is_none()
        {
            *start = server_start;
        }
    }

    pub fn status(&self) -> Option<&Status> {
        match self {
            Known::Found { status, .. } => Some(status),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Known::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    pub fn from(answer: Result<Status, String>, at: Instant) -> Known {
        match answer {
            Ok(status) => Known::Found {
                status,
                start: None,
                at,
            },
            Err(error) => Known::Failed { error, at },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // What hc 0.1.13 printed on this computer (`hc server status --json`).
    const STATUS: &str = r#"{"core":{"id":"90be87782970f742dec4952bf23c6319","name":"yuuzudemacbook-pro"},"finished":0,"holder":null,"holders":[{"pid":8470,"instance":"a1","current":true,"terminals":2},{"pid":8301,"instance":"9f","current":false,"terminals":1}],"installed_by":null,"live":3,"pid":8462,"uptime_ms":13,"version":"0.1.13"}"#;

    #[test]
    fn the_server_and_its_holders_come_from_status() {
        let s = parse(STATUS).unwrap();
        assert_eq!(
            s,
            Status {
                pid: 8462,
                holders: vec![8470, 8301]
            }
        );
        assert_eq!(s.roots(), vec![8462, 8470, 8301]);
        let fresh = parse(r#"{"holders":[],"pid":95082,"version":"0.1.13"}"#).unwrap();
        assert_eq!(fresh.roots(), vec![95082]);
    }

    #[test]
    fn anything_but_a_pid_is_an_error_not_a_guess() {
        assert!(parse("").is_err());
        assert!(
            parse("hc: command not found")
                .unwrap_err()
                .contains("not JSON")
        );
        assert_eq!(
            parse(r#"{"error":{"message":"no core"}}"#).unwrap_err(),
            "no core"
        );
        assert!(parse(r#"{"pid":0}"#).is_err());
        assert!(parse(r#"{"pid":-3}"#).is_err());
        assert!(parse(r#"{"holders":[{"pid":7}]}"#).is_err());
        // A holder without a pid is dropped, not fatal.
        assert_eq!(
            parse(r#"{"pid":5,"holders":[{"x":1},{"pid":6}]}"#)
                .unwrap()
                .holders,
            vec![6]
        );
    }

    #[test]
    fn asks_until_found_then_only_when_the_server_changes_or_ages() {
        let t0 = Instant::now();
        assert!(Known::Nothing.stale(t0, None));

        let mut k = Known::from(
            Ok(Status {
                pid: 50,
                holders: vec![51],
            }),
            t0,
        );
        // Start time not known yet: the server only has to be there.
        assert!(!k.stale(t0, Some(1_000)));
        k.seen(Some(1_000));
        k.seen(Some(2_000)); // kept from the first sighting
        assert!(!k.stale(t0 + Duration::from_secs(2), Some(1_000)));
        // Gone, or the pid is another process now.
        assert!(k.stale(t0 + Duration::from_secs(2), None));
        assert!(k.stale(t0 + Duration::from_secs(2), Some(2_000)));
        // Old answers are asked again (holders after a core update).
        assert!(k.stale(t0 + REFRESH, Some(1_000)));
        assert_eq!(k.status().map(Status::roots), Some(vec![50, 51]));
    }

    #[test]
    fn a_failed_ask_waits_before_asking_again() {
        let t0 = Instant::now();
        let k = Known::from(Err("hc: not found".into()), t0);
        assert_eq!(k.error(), Some("hc: not found"));
        assert!(k.status().is_none());
        assert!(!k.stale(t0 + Duration::from_secs(2), None));
        assert!(k.stale(t0 + RETRY, None));
    }
}
