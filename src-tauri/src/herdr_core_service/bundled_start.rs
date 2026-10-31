//! K9: was the core that answers started by Yuzora from the bundled binary?
//!
//! After an update the new Yuzora run has no memory of the core the old run started, only the
//! start record on disk. A record alone proves nothing: its pid may belong to another
//! process by now. The core counts as ours only when the operating system agrees on all of:
//! the pid is alive and is the pid the core at the endpoint reports, its executable is the
//! one the record names, and it started when the record says (a reused pid starts later).
//!
//! Restarting that core with the new binary (`Supervisor::restart_with`) is P4, not wired.
use std::path::{Path, PathBuf};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use yuzora_host::herdr_core::paths::Endpoint;
use yuzora_host::herdr_core::startup::{StartRecord, START_TIMEOUT};
use yuzora_host::herdr_service::strip_verbatim_prefix;

/// Clock slack when the process started before the start was requested: never expected, but
/// wall clocks and sysinfo's whole seconds disagree by a little.
const STARTED_EARLY_SLACK_S: u64 = 2;
/// A core answers within `START_TIMEOUT` of the request; the rest is slack.
const STARTED_LATE_SLACK_S: u64 = 5;

/// What the operating system says about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFacts {
    pub exe: Option<PathBuf>,
    /// Seconds since the Unix epoch.
    pub start_time_s: u64,
}

pub trait ProcessLookup: Send + Sync {
    /// `None` when no such process is alive.
    fn lookup(&self, pid: u32) -> Option<ProcessFacts>;
}

/// The real thing: sysinfo, refreshing only the one pid.
pub struct SystemProcesses;

impl ProcessLookup for SystemProcesses {
    fn lookup(&self, pid: u32) -> Option<ProcessFacts> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
        );
        let process = system.process(pid)?;
        Some(ProcessFacts {
            exe: process.exe().map(Path::to_path_buf),
            start_time_s: process.start_time(),
        })
    }
}

/// Why a core is not recognized as started by the bundled binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotBundled {
    NoRecord,
    /// The record is for another session or socket.
    OtherEndpoint,
    /// The core at the endpoint did not report the record's pid.
    PidMismatch,
    NotRunning,
    /// The operating system would not say which executable the process runs.
    ExeUnknown,
    ExeMismatch,
    /// Alive, same pid, same executable, but not started when the record says: pid reuse.
    StartedAtMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgment {
    /// Started by Yuzora from this executable.
    Bundled(StartRecord),
    NotBundled(NotBundled),
}

impl Judgment {
    pub fn is_bundled(&self) -> bool {
        matches!(self, Judgment::Bundled(_))
    }
}

/// Comparable path text: canonical, with the Windows verbatim prefix dropped and case folded,
/// so two spellings of one file compare equal.
fn comparable(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = strip_verbatim_prefix(path).to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// `record`: the persisted record, if any. `live_pid`: the pid the core at `endpoint` reports
/// in `server.status`.
pub fn judge(
    record: Option<&StartRecord>,
    endpoint: &Endpoint,
    live_pid: Option<u32>,
    processes: &dyn ProcessLookup,
) -> Judgment {
    let Some(record) = record else {
        return Judgment::NotBundled(NotBundled::NoRecord);
    };
    if record.session != endpoint.session || record.socket != endpoint.socket.to_string_lossy() {
        return Judgment::NotBundled(NotBundled::OtherEndpoint);
    }
    if live_pid != Some(record.pid) {
        return Judgment::NotBundled(NotBundled::PidMismatch);
    }
    let Some(process) = processes.lookup(record.pid) else {
        return Judgment::NotBundled(NotBundled::NotRunning);
    };
    let Some(exe) = process.exe else {
        return Judgment::NotBundled(NotBundled::ExeUnknown);
    };
    if comparable(&exe) != comparable(&record.exe_path()) {
        return Judgment::NotBundled(NotBundled::ExeMismatch);
    }
    let requested_s = record.started_unix_ms / 1000;
    let early = process.start_time_s + STARTED_EARLY_SLACK_S < requested_s;
    let late = process.start_time_s > requested_s + START_TIMEOUT.as_secs() + STARTED_LATE_SLACK_S;
    if early || late {
        return Judgment::NotBundled(NotBundled::StartedAtMismatch);
    }
    Judgment::Bundled(record.clone())
}

/// [`judge`] for the record file. A file this build cannot read is not a record: which core
/// is Yuzora's is never guessed.
pub fn judge_from_file(
    record_path: &Path,
    endpoint: &Endpoint,
    live_pid: Option<u32>,
    processes: &dyn ProcessLookup,
) -> Judgment {
    let record = StartRecord::read(record_path).ok().flatten();
    judge(record.as_ref(), endpoint, live_pid, processes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use yuzora_host::herdr_core::paths::{self, Env};

    fn endpoint() -> Endpoint {
        paths::resolve(
            paths::MACOS,
            &Env {
                home: Some("/Users/test".into()),
                ..Env::default()
            },
            "yuzora",
        )
        .unwrap()
    }

    struct Fake(HashMap<u32, ProcessFacts>);

    impl ProcessLookup for Fake {
        fn lookup(&self, pid: u32) -> Option<ProcessFacts> {
            self.0.get(&pid).cloned()
        }
    }

    const PID: u32 = 4242;
    const REQUESTED_MS: u64 = 1_800_000_000_000;

    fn record_for(endpoint: &Endpoint, exe: &str) -> StartRecord {
        StartRecord {
            v: 1,
            pid: PID,
            started_unix_ms: REQUESTED_MS,
            exe: exe.into(),
            version: "0.1.14".into(),
            session: endpoint.session.clone(),
            socket: endpoint.socket.to_string_lossy().into_owned(),
        }
    }

    fn running(exe: &str, start_time_s: u64) -> Fake {
        Fake(HashMap::from([(
            PID,
            ProcessFacts {
                exe: Some(PathBuf::from(exe)),
                start_time_s,
            },
        )]))
    }

    #[test]
    fn herdr_core_k9_recognizes_a_core_started_from_the_bundled_binary() {
        let ep = endpoint();
        let record = record_for(&ep, "/App/herdr-core");
        let started = REQUESTED_MS / 1000 + 1;
        let judgment = judge(
            Some(&record),
            &ep,
            Some(PID),
            &running("/App/herdr-core", started),
        );
        assert_eq!(judgment, Judgment::Bundled(record.clone()));
        assert!(judgment.is_bundled());
        // The edges of the window around the request still count.
        for start in [
            REQUESTED_MS / 1000 - STARTED_EARLY_SLACK_S,
            REQUESTED_MS / 1000 + START_TIMEOUT.as_secs() + STARTED_LATE_SLACK_S,
        ] {
            let at_edge = judge(
                Some(&record),
                &ep,
                Some(PID),
                &running("/App/herdr-core", start),
            );
            assert!(at_edge.is_bundled(), "{start}");
        }
    }

    #[test]
    fn herdr_core_k9_refuses_every_mismatch() {
        let ep = endpoint();
        let record = record_for(&ep, "/App/herdr-core");
        let started = REQUESTED_MS / 1000 + 1;
        let alive = running("/App/herdr-core", started);
        let verdict = |record: Option<&StartRecord>, live: Option<u32>, processes: &Fake| {
            judge(record, &ep, live, processes)
        };
        let not = |reason| Judgment::NotBundled(reason);

        assert_eq!(verdict(None, Some(PID), &alive), not(NotBundled::NoRecord));
        // The core at the endpoint is a different process than the recorded one.
        assert_eq!(
            verdict(Some(&record), Some(PID + 1), &alive),
            not(NotBundled::PidMismatch)
        );
        assert_eq!(
            verdict(Some(&record), None, &alive),
            not(NotBundled::PidMismatch)
        );
        // Another session's or socket's record.
        for other in [
            StartRecord {
                session: "default".into(),
                ..record.clone()
            },
            StartRecord {
                socket: "/elsewhere.sock".into(),
                ..record.clone()
            },
        ] {
            assert_eq!(
                verdict(Some(&other), Some(PID), &alive),
                not(NotBundled::OtherEndpoint)
            );
        }
        // The pid is gone.
        assert_eq!(
            verdict(Some(&record), Some(PID), &Fake(HashMap::new())),
            not(NotBundled::NotRunning)
        );
        // Alive, but running something else (the user's own herdr-core, a reused pid).
        assert_eq!(
            verdict(
                Some(&record),
                Some(PID),
                &running("/usr/local/bin/herdr-core", started)
            ),
            not(NotBundled::ExeMismatch)
        );
        let no_exe = Fake(HashMap::from([(
            PID,
            ProcessFacts {
                exe: None,
                start_time_s: started,
            },
        )]));
        assert_eq!(
            verdict(Some(&record), Some(PID), &no_exe),
            not(NotBundled::ExeUnknown)
        );
        // Same executable path, but the process started long after (or before) the request:
        // the pid was reused by another run of the same binary.
        for start in [
            started + 3600,
            REQUESTED_MS / 1000 + START_TIMEOUT.as_secs() + STARTED_LATE_SLACK_S + 1,
            REQUESTED_MS / 1000 - STARTED_EARLY_SLACK_S - 1,
            1,
        ] {
            assert_eq!(
                verdict(Some(&record), Some(PID), &running("/App/herdr-core", start)),
                not(NotBundled::StartedAtMismatch),
                "{start}"
            );
        }
    }

    #[test]
    fn herdr_core_k9_matches_this_very_process_through_sysinfo() {
        let ep = endpoint();
        let me = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let facts = SystemProcesses.lookup(me).expect("this process is alive");
        assert!(facts.start_time_s > 1_500_000_000, "{facts:?}");
        assert!(comparable(facts.exe.as_deref().unwrap()) == comparable(&exe));

        let record = StartRecord {
            pid: me,
            started_unix_ms: facts.start_time_s * 1000,
            exe: exe.to_string_lossy().into_owned(),
            ..record_for(&ep, "")
        };
        assert!(judge(Some(&record), &ep, Some(me), &SystemProcesses).is_bundled());

        // The pid-reuse guard: the same pid and executable, recorded as started an hour
        // after this process actually started.
        let later = StartRecord {
            started_unix_ms: (facts.start_time_s + 3600) * 1000,
            ..record.clone()
        };
        assert_eq!(
            judge(Some(&later), &ep, Some(me), &SystemProcesses),
            Judgment::NotBundled(NotBundled::StartedAtMismatch)
        );
        // Another executable name: not ours.
        let other = StartRecord {
            exe: exe
                .with_file_name("herdr-core-other")
                .to_string_lossy()
                .into_owned(),
            ..record
        };
        assert_eq!(
            judge(Some(&other), &ep, Some(me), &SystemProcesses),
            Judgment::NotBundled(NotBundled::ExeMismatch)
        );
        // A pid no process has.
        assert_eq!(SystemProcesses.lookup(u32::MAX - 1), None);
    }

    #[test]
    fn herdr_core_k9_reads_the_record_file_and_never_guesses_from_a_bad_one() {
        let ep = endpoint();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("start-record-yuzora.json");
        let started = REQUESTED_MS / 1000;
        let alive = running("/App/herdr-core", started);

        assert_eq!(
            judge_from_file(&path, &ep, Some(PID), &alive),
            Judgment::NotBundled(NotBundled::NoRecord)
        );
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            judge_from_file(&path, &ep, Some(PID), &alive),
            Judgment::NotBundled(NotBundled::NoRecord)
        );
        let record = record_for(&ep, "/App/herdr-core");
        record.write(&path).unwrap();
        assert!(judge_from_file(&path, &ep, Some(PID), &alive).is_bundled());
        // A record of a format this build does not know.
        std::fs::write(
            &path,
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("\"v\": 1", "\"v\": 9"),
        )
        .unwrap();
        assert_eq!(
            judge_from_file(&path, &ep, Some(PID), &alive),
            Judgment::NotBundled(NotBundled::NoRecord)
        );
    }
}
