//! Native repository authority is per workspace, with independent operation locks.
use crate::git_service::GitEnvironment;
use crate::git_watch::GitWatcher;
use crate::workspace_trust::{observe_identity, WorkspaceIdentity};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

struct Repository {
    identity: WorkspaceIdentity,
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
pub struct GitRegistry(Mutex<State>, AtomicUsize);

pub struct GitJob<'a>(&'a AtomicUsize);
impl Drop for GitJob<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl GitRegistry {
    pub fn try_job(&self) -> Result<GitJob<'_>, String> {
        self.1
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 4).then_some(count + 1)
            })
            .map(|_| GitJob(&self.1))
            .map_err(|_| "git-jobs-busy".into())
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

    pub fn with_repository<T>(
        &self,
        requested: &str,
        operation: impl FnOnce(&Path) -> Result<T, String>,
    ) -> Result<T, String> {
        let _job = self.try_job()?;
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
        let _operation = repo.operation.lock().map_err(|e| e.to_string())?;
        if repo.cancelled.load(Ordering::Acquire)
            || observe_identity(requested).map_err(|e| e.to_frontend())? != repo.identity
        {
            return Err("git repository changed before operation".into());
        }
        crate::git_process::with_cancellation(repo.cancelled.clone(), || operation(&root))
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
}
