//! One sample of this computer's Yuzora: refresh the process table, find
//! herdr-core, aggregate (see [`crate::tree`]).
//!
//! The `System` persists between samples: CPU is a delta from the previous
//! refresh, so the first sample after start reads 0 and a process seen for
//! the first time reads 0 once.
//!
//! On macOS only the trees measured are refreshed: the kernel's child-pid
//! lists say who is under the app and the core, and sysinfo refreshes just
//! those (a full refresh reads every process's argv and environment, about
//! 10 ms for 650 processes). Elsewhere the whole table is refreshed.
//!
//! Known limits (best effort): a process re-parented away from its tree
//! (Windows' re-parenting, a helper that daemonizes itself) leaves it.

use crate::core::{Known, Status};
use crate::tree::{Row, Usages, aggregate};
use std::collections::HashSet;
use std::time::Instant;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// What one sample reports.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Snapshot {
    #[serde(flatten)]
    pub usage: Usages,
    /// The herdr-core server measured, or null with `core_error`.
    pub core_pid: Option<u32>,
    pub core_error: Option<String>,
}

pub struct Sampler {
    system: System,
    app: Pid,
    /// The names `hc` runs under (`hc`, `herdr-core`): a child of the app by
    /// one of them is its connection to its core (`hc connect`).
    hc_names: Vec<String>,
    core: Known,
    /// Asks the app's `hc` where its server is; a test swaps it.
    ask: Box<dyn FnMut() -> Result<Status, String> + Send>,
}

/// Why there's no herdr-core while the app holds no connection to one.
pub const NOT_CONNECTED: &str = "not connected to herdr-core";

impl Sampler {
    pub fn new(
        app: Pid,
        hc_names: &[&str],
        ask: impl FnMut() -> Result<Status, String> + Send + 'static,
    ) -> Self {
        let hc_names = hc_names.iter().map(|n| program_name(n)).collect();
        Self {
            system: System::new(),
            app,
            hc_names,
            core: Known::default(),
            ask: Box::new(ask),
        }
    }

    /// This process, its tree and its herdr-core.
    pub fn for_this_app() -> Result<Self, String> {
        let app = sysinfo::get_current_pid().map_err(str::to_owned)?;
        let program = yuzora_core::remote::hc_program();
        let installed = program
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::new(
            app,
            &["hc", "herdr-core", &installed],
            crate::core::ask,
        ))
    }

    /// Ok(None): the app's own process wasn't in the table (no data).
    pub fn sample(&mut self) -> Result<Option<Snapshot>, String> {
        let mut rows = self.rows();
        if !self.connected() {
            // hc starts a core when none runs, so asking is only safe while
            // the app holds a connection to one. `herdr-app check` and tests
            // never do, and never touch the user's core.
            self.core = Known::Nothing;
        } else if self.core.stale(Instant::now(), self.server_start()) {
            // First sample, the server went away (a restart: rather than a
            // core of zero for one sample), or the answer aged.
            self.core = Known::from((self.ask)(), Instant::now());
            rows = self.rows();
        }
        self.core.seen(self.server_start());
        let roots = self.core.status().map(Status::roots).unwrap_or_default();
        let Some(usage) = aggregate(&rows, self.app.as_u32(), &roots) else {
            return Ok(None);
        };
        let core_pid = self.core.status().map(|s| s.pid);
        let core_error = core_pid
            .is_none()
            .then(|| self.core.error().unwrap_or(NOT_CONNECTED).to_owned());
        Ok(Some(Snapshot {
            usage,
            core_pid,
            core_error,
        }))
    }

    /// Whether a child of the app is `hc` (as of the last refresh).
    fn connected(&self) -> bool {
        self.system.processes().values().any(|p| {
            p.parent() == Some(self.app)
                && self
                    .hc_names
                    .contains(&program_name(&p.name().to_string_lossy()))
        })
    }

    /// The start time of whatever process holds the kept server's pid.
    fn server_start(&self) -> Option<u64> {
        let pid = self.core.status()?.pid;
        self.system
            .process(Pid::from_u32(pid))
            .map(sysinfo::Process::start_time)
    }

    fn rows(&mut self) -> Vec<Row> {
        let mut roots = vec![self.app];
        if let Some(status) = self.core.status() {
            roots.extend(status.roots().into_iter().map(Pid::from_u32));
        }
        collect(&mut self.system, &roots)
    }
}

/// A program's name as the process table has it, compared without case or
/// Windows' `.exe`.
fn program_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .map(str::to_owned)
        .unwrap_or(lower)
}

/// Refreshes the table and returns its rows: on macOS only the processes in
/// the trees under `roots` (falling back to everything if listing fails).
pub fn collect(system: &mut System, roots: &[Pid]) -> Vec<Row> {
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    #[cfg(target_os = "macos")]
    if let Some(tree) = macos::trees(roots) {
        // Cached pids missing from the OS's list are refreshed too, so sysinfo
        // drops them; a live former member stays cached but isn't reported.
        let update: Vec<Pid> = tree
            .owned
            .iter()
            .copied()
            .chain(
                system
                    .processes()
                    .keys()
                    .filter(|p| !tree.live.contains(p))
                    .copied(),
            )
            .collect();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&update), true, kind);
        return rows(system, |pid| tree.owned.contains(pid));
    }
    let _ = roots;
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    rows(system, |_| true)
}

fn rows(system: &System, keep: impl Fn(&Pid) -> bool) -> Vec<Row> {
    system
        .processes()
        .iter()
        .filter(|(pid, _)| keep(pid))
        .map(|(pid, p)| Row {
            pid: pid.as_u32(),
            parent: p.parent().map(|p| p.as_u32()),
            cpu: p.cpu_usage(),
            memory: p.memory(),
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::collections::VecDeque;

    pub struct Trees {
        /// Every pid the OS lists now.
        pub live: HashSet<Pid>,
        /// The roots that are alive and everything under them.
        pub owned: HashSet<Pid>,
    }

    /// libproc answers in pids for both lists. Its size query may count
    /// unrelated processes, so the whole answer is reserved and a full buffer
    /// is refused (it may have been cut short).
    pub fn pid_list(parent: Option<Pid>) -> Option<Vec<Pid>> {
        let read = |buffer: *mut libc::c_void, size| unsafe {
            // SAFETY: libproc writes at most `size` bytes into `buffer` (or
            // only counts, given a null buffer).
            match parent {
                Some(parent) => {
                    libc::proc_listchildpids(parent.as_u32() as libc::pid_t, buffer, size)
                }
                None => libc::proc_listallpids(buffer, size),
            }
        };
        let capacity = read(std::ptr::null_mut(), 0);
        if capacity <= 0 {
            return None;
        }
        let mut pids = vec![0 as libc::pid_t; (capacity as usize).checked_add(32)?];
        let bytes = pids.len().checked_mul(std::mem::size_of::<libc::pid_t>())?;
        let count = read(pids.as_mut_ptr().cast(), i32::try_from(bytes).ok()?);
        if count < 0 || count as usize >= pids.len() {
            return None;
        }
        pids.truncate(count as usize);
        Some(
            pids.into_iter()
                .filter(|pid| *pid > 0)
                .map(|pid| Pid::from_u32(pid as u32))
                .collect(),
        )
    }

    /// Walks the trees through the kernel's child-pid lists. Reading every
    /// unrelated process's info can be denied even when listing pids isn't.
    pub fn trees(roots: &[Pid]) -> Option<Trees> {
        let mut live: HashSet<Pid> = pid_list(None)?.into_iter().collect();
        let mut owned = HashSet::new();
        let mut queue: VecDeque<Pid> = roots.iter().copied().filter(|r| live.contains(r)).collect();
        owned.extend(queue.iter().copied());
        while let Some(parent) = queue.pop_front() {
            for child in pid_list(Some(parent))? {
                live.insert(child);
                if owned.insert(child) {
                    queue.push_back(child);
                }
            }
        }
        Some(Trees { live, owned })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A child that lives until the test kills it. Not through `cmd /C` on
    /// Windows: `Child::kill` ends one process, so a grandchild would outlive
    /// the test.
    fn spawn_child() -> std::process::Child {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        };
        cmd.stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn a child")
    }

    fn me() -> Pid {
        sysinfo::get_current_pid().unwrap()
    }

    #[test]
    fn samples_this_process_with_memory_and_a_finite_cpu() {
        let asked = Arc::new(Mutex::new(0));
        let counter = asked.clone();
        let mut s = Sampler::new(me(), &["no-such-hc"], move || {
            *counter.lock().unwrap() += 1;
            Err("asked".into())
        });
        let _ = s.sample();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snap = s.sample().unwrap().expect("this process is alive");
        assert!(snap.usage.app.memory > 0);
        // false for NaN too
        assert!(snap.usage.total.cpu >= 0.0);
        assert_eq!(snap.core_pid, None);
        assert_eq!(snap.core_error.as_deref(), Some(NOT_CONNECTED));
        assert_eq!(
            *asked.lock().unwrap(),
            0,
            "no hc child: hc is never asked (it would start a core)"
        );
    }

    #[test]
    fn a_missing_app_is_no_data_not_zero() {
        let mut s = Sampler::new(Pid::from_u32(u32::MAX), &[], || Err("x".into()));
        assert_eq!(s.sample(), Ok(None));
    }

    #[test]
    fn hc_is_known_by_its_names_on_every_system() {
        assert_eq!(program_name("herdr-core"), "herdr-core");
        assert_eq!(program_name("HC.EXE"), "hc");
        assert_eq!(program_name("herdr-core.exe"), "herdr-core");
    }

    #[test]
    fn a_real_child_counts_as_a_tool_and_leaves_after_it_exits() {
        let mut system = System::new();
        let _ = collect(&mut system, &[me()]);
        let mut child = spawn_child();
        let pid = Pid::from_u32(child.id());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let alive = collect(&mut system, &[me()]);
        let usage = aggregate(&alive, me().as_u32(), &[]);
        // Reap before asserting: Child's drop doesn't kill.
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let after = collect(&mut system, &[me()]);

        assert!(
            alive
                .iter()
                .any(|r| r.pid == pid.as_u32() && r.parent == Some(me().as_u32())),
            "the child is listed under this process"
        );
        let usage = usage.expect("this process is alive");
        // Other tests may run children too: at least one.
        assert!(usage.tools.count >= 1);
        assert!(usage.total.memory > usage.app.memory);
        assert!(
            !after.iter().any(|r| r.pid == pid.as_u32()),
            "an exited child leaves the table"
        );
        assert!(system.process(pid).is_none(), "and sysinfo's cache");
    }

    #[test]
    fn the_core_named_by_hc_is_measured_and_asked_again_when_it_goes() {
        // A child stands in for the server. It is under this process, but
        // being named by hc makes it core, not a tool.
        let mut first = spawn_child();
        let mut second = spawn_child();
        let (a, b) = (first.id(), second.id());
        let answer = Arc::new(Mutex::new(a));
        let asked = Arc::new(Mutex::new(0));
        let (answer2, asked2) = (answer.clone(), asked.clone());
        // The children spawned here stand in for `hc connect` too.
        let mut s = Sampler::new(me(), &["sleep", "ping"], move || {
            *asked2.lock().unwrap() += 1;
            Ok(Status {
                pid: *answer2.lock().unwrap(),
                holders: vec![],
            })
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        let one = s.sample().unwrap().unwrap();
        let two = s.sample().unwrap().unwrap();
        let asked_while_alive = *asked.lock().unwrap();
        // The server "restarts": the old one ends, hc names the new one.
        let _ = first.kill();
        let _ = first.wait();
        *answer.lock().unwrap() = b;
        std::thread::sleep(std::time::Duration::from_millis(200));
        let three = s.sample().unwrap().unwrap();
        let _ = second.kill();
        let _ = second.wait();

        assert_eq!((one.core_pid, one.core_error.clone()), (Some(a), None));
        assert_eq!(one.usage.core.count, 1, "the server is core, not a tool");
        assert!(one.usage.core.memory > 0);
        assert_eq!(asked_while_alive, 1, "kept while the server lives");
        assert_eq!(two.core_pid, Some(a));
        assert_eq!(
            three.core_pid,
            Some(b),
            "asked again once the server was gone"
        );
        assert_eq!(*asked.lock().unwrap(), 2);
        assert_eq!(three.usage.core.count, 1);
        let u = three.usage;
        assert_eq!(
            u.app.memory + u.core.memory + u.tools.memory,
            u.total.memory
        );
        assert_eq!(u.app.count + u.core.count + u.tools.count, u.total.count);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_walks_past_a_childless_leaf_and_several_roots() {
        // A childless process lists no children; read as a failure, every
        // sample would fall back to the full refresh.
        let mut child = spawn_child();
        let mut other = spawn_child();
        let (pid, other_pid) = (Pid::from_u32(child.id()), Pid::from_u32(other.id()));
        let leaf = macos::pid_list(Some(pid));
        let trees = macos::trees(&[me(), other_pid, Pid::from_u32(u32::MAX)]);
        let _ = child.kill();
        let _ = child.wait();
        let _ = other.kill();
        let _ = other.wait();
        assert_eq!(leaf, Some(Vec::new()));
        let trees = trees.expect("trees found");
        assert!(
            trees.owned.contains(&me())
                && trees.owned.contains(&pid)
                && trees.owned.contains(&other_pid)
        );
        assert!(
            !trees.owned.contains(&Pid::from_u32(u32::MAX)),
            "a root that isn't alive is skipped"
        );
    }
}
