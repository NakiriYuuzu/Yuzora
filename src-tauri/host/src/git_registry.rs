//! Native repository authority is per workspace, with independent operation locks.
use crate::git_service::GitEnvironment;
use crate::git_watch::GitWatcher;
use crate::workspace_trust::{observe_identity, WorkspaceIdentity};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};

/// Concurrent git subprocesses across all native repositories.
const MAX_GIT_JOBS: usize = 4;
/// How long a request queues for a job slot before reporting `git-jobs-busy`.
/// Rejecting immediately made user actions fail whenever background status,
/// log and remote checks happened to be in flight.
const GIT_JOB_WAIT: Duration = Duration::from_secs(30);

struct Repository {
    identity: WorkspaceIdentity,
    mutation_admission: JobGate,
    operation: Mutex<()>,
    cancelled: Arc<AtomicBool>,
}
struct Workspace {
    generation: u64,
    root: Option<PathBuf>,
    watcher: Option<GitWatcher>,
}
#[derive(Default)]
struct State {
    generation: u64,
    workspaces: HashMap<String, Workspace>,
    repositories: HashMap<PathBuf, Arc<Repository>>,
}
#[derive(Default)]
struct JobGate {
    running: Mutex<usize>,
    freed: Condvar,
}
#[derive(Default)]
pub struct GitRegistry(Mutex<State>, JobGate);

pub struct GitJob<'a>(&'a JobGate);
impl Drop for GitJob<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.0.running.lock() {
            *running = running.saturating_sub(1);
        }
        self.0.freed.notify_one();
    }
}

impl GitRegistry {
    /// Queues for one of the bounded git job slots. Callers run on blocking
    /// threads, so waiting here never parks an async worker.
    pub fn acquire_job(&self) -> Result<GitJob<'_>, String> {
        self.acquire_job_within(GIT_JOB_WAIT)
    }

    fn acquire_job_within(&self, wait: Duration) -> Result<GitJob<'_>, String> {
        self.1.acquire_within(wait)
    }
    pub fn begin(&self, workspace: &str) -> Result<u64, String> {
        let mut state = self.0.lock().map_err(|e| e.to_string())?;
        if state.workspaces.len() >= 128 && !state.workspaces.contains_key(workspace) {
            return Err("git-workspace-limit".into());
        }
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("git-generation-exhausted")?;
        let generation = state.generation;
        state
            .workspaces
            .entry(workspace.into())
            .or_insert(Workspace {
                generation,
                root: None,
                watcher: None,
            })
            .generation = generation;
        Ok(generation)
    }

    pub fn finish(
        &self,
        workspace: &str,
        generation: u64,
        environment: &GitEnvironment,
        watcher: Option<GitWatcher>,
    ) -> Result<bool, String> {
        let identity = if let GitEnvironment::Ready { root, .. } = environment {
            Some(observe_identity(root).map_err(|e| e.to_frontend())?)
        } else {
            None
        };
        let mut state = self.0.lock().map_err(|e| e.to_string())?;
        if state
            .workspaces
            .get(workspace)
            .is_none_or(|slot| slot.generation != generation)
        {
            return Ok(false);
        }
        let root = identity
            .as_ref()
            .map(|identity| PathBuf::from(&identity.canonical_path));
        if let Some(identity) = identity {
            let key = root.clone().unwrap();
            if state
                .repositories
                .get(&key)
                .is_none_or(|repo| repo.identity != identity)
            {
                if let Some(old) = state.repositories.insert(
                    key,
                    Arc::new(Repository {
                        identity,
                        mutation_admission: JobGate::default(),
                        operation: Mutex::new(()),
                        cancelled: Arc::new(AtomicBool::new(false)),
                    }),
                ) {
                    old.cancelled.store(true, Ordering::Release);
                }
            }
        }
        let slot = state.workspaces.get_mut(workspace).unwrap();
        slot.root = root;
        let old_watcher = std::mem::replace(&mut slot.watcher, watcher);
        Self::remove_orphans(&mut state);
        drop(state);
        // A watcher may join its worker; never hold the global registry lock.
        drop(old_watcher);
        Ok(true)
    }

    pub fn close(&self, workspace: &str, generation: u64) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|e| e.to_string())?;
        if state
            .workspaces
            .get(workspace)
            .is_none_or(|slot| slot.generation != generation)
        {
            return Ok(());
        }
        let old = state.workspaces.remove(workspace);
        Self::remove_orphans(&mut state);
        drop(state);
        drop(old);
        Ok(())
    }

    fn remove_orphans(state: &mut State) {
        state.repositories.retain(|root, repo| {
            let retained = state
                .workspaces
                .values()
                .any(|slot| slot.root.as_ref() == Some(root));
            if !retained {
                repo.cancelled.store(true, Ordering::Release);
            }
            retained
        });
    }

    /// Runs a mutation: serialized with every other mutation of the repository.
    pub fn with_repository<T>(
        &self,
        requested: &str,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_access(requested, true, operation)
    }

    /// Runs a read-only command. Reads do not take the mutation lock, so status,
    /// log and diff stay responsive while a push/pull/fetch holds it for up to
    /// the remote timeout; git itself keeps those reads consistent.
    pub fn with_repository_read<T>(
        &self,
        requested: &str,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_repository_access(requested, false, operation)
    }

    fn with_repository_access<T>(
        &self,
        requested: &str,
        exclusive: bool,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        let identity = observe_identity(requested).map_err(|e| e.to_frontend())?;
        let root = PathBuf::from(&identity.canonical_path);
        let repo = self
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .repositories
            .get(&root)
            .cloned()
            .ok_or("no repository detected for this workspace")?;
        // Preserve bounded admission for a busy repository without reserving
        // execution slots while its mutations wait for the operation lock.
        let _admission = if exclusive {
            Some(repo.mutation_admission.acquire_within(GIT_JOB_WAIT)?)
        } else {
            None
        };
        let _operation = if exclusive {
            Some(repo.operation.lock().map_err(|e| e.to_string())?)
        } else {
            None
        };
        let _job = self.acquire_job()?;
        if repo.cancelled.load(Ordering::Acquire)
            || observe_identity(requested).map_err(|e| e.to_frontend())? != repo.identity
        {
            return Err("git repository changed before operation".into());
        }
        crate::git_process::with_cancellation(repo.cancelled.clone(), || operation(&root))
    }
}

impl JobGate {
    fn acquire_within(&self, wait: Duration) -> Result<GitJob<'_>, String> {
        let deadline = Instant::now() + wait;
        let mut running = self.running.lock().map_err(|e| e.to_string())?;
        while *running >= MAX_GIT_JOBS {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("git-jobs-busy".into());
            }
            running = self
                .freed
                .wait_timeout(running, left)
                .map_err(|e| e.to_string())?
                .0;
        }
        *running += 1;
        Ok(GitJob(self))
    }
}

impl Drop for GitRegistry {
    fn drop(&mut self) {
        if let Ok(state) = self.0.get_mut() {
            for repo in state.repositories.values() {
                repo.cancelled.store(true, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bind(registry: &GitRegistry, path: &Path) -> u64 {
        let path = path.to_str().unwrap();
        let generation = registry.begin(path).unwrap();
        registry
            .finish(
                path,
                generation,
                &GitEnvironment::Ready {
                    root: path.into(),
                    version: "test".into(),
                },
                None,
            )
            .unwrap();
        generation
    }
    #[test]
    fn independent_workspaces_never_hold_each_others_operation_lock() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let registry = GitRegistry::default();
        let ga = bind(&registry, a.path());
        bind(&registry, b.path());
        registry
            .with_repository(a.path().to_str().unwrap(), |_| {
                registry.with_repository(b.path().to_str().unwrap(), |_| Ok(()))
            })
            .unwrap();
        registry.close(a.path().to_str().unwrap(), ga).unwrap();
        assert!(registry
            .with_repository(a.path().to_str().unwrap(), |_| Ok(()))
            .is_err());
        assert!(registry
            .with_repository(b.path().to_str().unwrap(), |_| Ok(()))
            .is_ok());
    }
    #[test]
    fn job_admission_queues_until_a_slot_frees_instead_of_rejecting() {
        let registry = Arc::new(GitRegistry::default());
        let held: Vec<_> = (0..MAX_GIT_JOBS)
            .map(|_| registry.acquire_job().unwrap())
            .collect();
        assert_eq!(
            registry
                .acquire_job_within(Duration::from_millis(20))
                .err()
                .as_deref(),
            Some("git-jobs-busy")
        );
        let waiter = {
            let registry = registry.clone();
            std::thread::spawn(move || {
                registry
                    .acquire_job_within(Duration::from_secs(5))
                    .map(|_| ())
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        assert!(waiter.join().unwrap().is_ok());
    }
    #[test]
    fn reads_do_not_wait_for_a_running_mutation() {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().to_str().unwrap().to_string();
        let registry = Arc::new(GitRegistry::default());
        bind(&registry, repo.path());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let mutation = {
            let registry = registry.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                registry.with_repository(&path, |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
            })
        };
        entered_rx.recv().unwrap();
        registry.with_repository_read(&path, |_| Ok(())).unwrap();
        release_tx.send(()).unwrap();
        mutation.join().unwrap().unwrap();
    }
    #[test]
    fn close_and_late_detect_cannot_replace_a_new_generation() {
        let path = tempfile::tempdir().unwrap();
        let workspace = path.path().to_str().unwrap();
        let registry = GitRegistry::default();
        let before = bind(&registry, path.path());
        let after = bind(&registry, path.path());
        assert!(!registry
            .finish(workspace, before, &GitEnvironment::NotARepo, None)
            .unwrap());
        registry.close(workspace, before).unwrap();
        assert!(registry.with_repository(workspace, |_| Ok(())).is_ok());
        registry.close(workspace, after).unwrap();
        assert!(!registry
            .finish(
                workspace,
                after,
                &GitEnvironment::Ready {
                    root: workspace.into(),
                    version: "test".into()
                },
                None
            )
            .unwrap());
    }

    #[test]
    fn queued_mutations_do_not_occupy_execution_slots() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let path_a = a.path().to_str().unwrap().to_string();
        let path_b = b.path().to_str().unwrap().to_string();
        let registry = Arc::new(GitRegistry::default());
        bind(&registry, a.path());
        bind(&registry, b.path());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let active = {
            let registry = registry.clone();
            let path = path_a.clone();
            std::thread::spawn(move || {
                registry.with_repository(&path, |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
            })
        };
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let root = PathBuf::from(observe_identity(&path_a).unwrap().canonical_path);
        let repository = registry.0.lock().unwrap().repositories[&root].clone();
        let expected_owners = Arc::strong_count(&repository) + MAX_GIT_JOBS - 1;
        let queued: Vec<_> = (1..MAX_GIT_JOBS)
            .map(|_| {
                let registry = registry.clone();
                let path = path_a.clone();
                std::thread::spawn(move || registry.with_repository(&path, |_| Ok(())))
            })
            .collect();
        // Observe actual entry into repository admission, rather than guessing
        // that a newly spawned thread has already reached the mutex.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&repository) < expected_owners && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let all_queued = Arc::strong_count(&repository) == expected_owners;
        let occupied = *registry.1.running.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let independent: Vec<_> = [(path_a, false), (path_b, true)]
            .into_iter()
            .map(|(path, exclusive)| {
                let registry = registry.clone();
                let done_tx = done_tx.clone();
                std::thread::spawn(move || {
                    let result = registry.with_repository_access(&path, exclusive, |_| Ok(()));
                    done_tx.send(result.is_ok()).unwrap();
                    result
                })
            })
            .collect();
        let completions = [
            done_rx.recv_timeout(Duration::from_secs(2)),
            done_rx.recv_timeout(Duration::from_secs(2)),
        ];
        // Always drain our threads before asserting, including the failing baseline.
        release_tx.send(()).unwrap();
        active.join().unwrap().unwrap();
        for thread in queued.into_iter().chain(independent) {
            thread.join().unwrap().unwrap();
        }
        assert!(
            all_queued,
            "queued threads did not enter repository admission"
        );
        assert_eq!(occupied, 1, "waiting mutations consumed execution slots");
        assert!(
            completions.iter().all(|result| matches!(result, Ok(true))),
            "read/other repository waited behind queued mutations: {completions:?}"
        );
        assert_eq!(*registry.1.running.lock().unwrap(), 0);
    }

    #[test]
    fn queued_mutation_revalidates_after_close_or_identity_replacement() {
        for replace_identity in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let path = fixture.path().join("repo");
            std::fs::create_dir(&path).unwrap();
            let requested = path.to_str().unwrap().to_string();
            let registry = Arc::new(GitRegistry::default());
            let generation = bind(&registry, &path);
            let root = PathBuf::from(observe_identity(&requested).unwrap().canonical_path);
            let repository = registry.0.lock().unwrap().repositories[&root].clone();
            let operation_lock = repository.operation.lock().unwrap();
            let entered = Arc::new(AtomicBool::new(false));
            let queued = {
                let registry = registry.clone();
                let requested = requested.clone();
                let entered = entered.clone();
                std::thread::spawn(move || {
                    registry.with_repository(&requested, |_| {
                        entered.store(true, Ordering::Release);
                        Ok(())
                    })
                })
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            while Arc::strong_count(&repository) < 3 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            let admitted = Arc::strong_count(&repository) == 3;
            if replace_identity {
                std::fs::rename(&path, fixture.path().join("old-repo")).unwrap();
                std::fs::create_dir(&path).unwrap();
                bind(&registry, &path);
                registry.with_repository(&requested, |_| Ok(())).unwrap();
            } else {
                registry.close(&requested, generation).unwrap();
            }
            drop(operation_lock);
            let result = queued.join().unwrap();
            assert!(admitted, "worker did not reach repository admission");
            assert_eq!(
                result.err().as_deref(),
                Some("git repository changed before operation")
            );
            assert!(!entered.load(Ordering::Acquire));
            assert_eq!(*registry.1.running.lock().unwrap(), 0);
        }
    }

    #[test]
    fn overflow_mutation_retains_admission_deadline() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().to_str().unwrap().to_string();
        let registry = Arc::new(GitRegistry::default());
        bind(&registry, fixture.path());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let active = {
            let registry = registry.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                registry.with_repository(&path, |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
            })
        };
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let root = PathBuf::from(observe_identity(&path).unwrap().canonical_path);
        let repository = registry.0.lock().unwrap().repositories[&root].clone();
        let expected = Arc::strong_count(&repository) + MAX_GIT_JOBS - 1;
        let queued: Vec<_> = (1..MAX_GIT_JOBS)
            .map(|_| {
                let registry = registry.clone();
                let path = path.clone();
                std::thread::spawn(move || registry.with_repository(&path, |_| Ok(())))
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(5);
        while (Arc::strong_count(&repository) < expected
            || *repository.mutation_admission.running.lock().unwrap() < MAX_GIT_JOBS)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let queued_ready = Arc::strong_count(&repository) == expected
            && *repository.mutation_admission.running.lock().unwrap() == MAX_GIT_JOBS;
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let overflow = {
            let registry = registry.clone();
            std::thread::spawn(move || {
                let result = registry.with_repository(&path, |_| Ok(()));
                done_tx.send(result.clone()).unwrap();
                result
            })
        };
        let before_release = done_rx.recv_timeout(GIT_JOB_WAIT + Duration::from_secs(2));
        release_tx.send(()).unwrap();
        active.join().unwrap().unwrap();
        for job in queued {
            job.join().unwrap().unwrap();
        }
        let result = overflow.join().unwrap();
        assert!(queued_ready, "waiters did not reach repository admission");
        assert_eq!(
            before_release,
            Ok(Err("git-jobs-busy".into())),
            "overflow bypassed admission deadline; final result: {result:?}"
        );
    }
}
