//! Gateway-owned bounded executor for the Host facade's synchronous methods.
//! Cancelling a peer orphans a running call; it does not abort synchronous IO.
#[cfg(test)]
#[path = "herdr_jobs_tests.rs"]
mod tests;

use crate::herdr::{HerdrPort, Resource};
use std::sync::{Arc, Mutex};
use tokio::{
    sync::{oneshot, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};

pub(crate) const MAX_CALLS: usize = 4;
const MAX_RESOURCES: usize = crate::bridge::MAX_PEERS * 8;

pub(crate) struct Jobs {
    slots: Arc<Semaphore>,
    write_slots: Arc<Semaphore>,
    cleanup_slots: Arc<Semaphore>,
    devices: Mutex<std::collections::HashMap<(String, bool), std::sync::Weak<Semaphore>>>,
    resources: Arc<Semaphore>,
    tasks: Mutex<JoinSet<()>>,
}
impl Jobs {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slots: Arc::new(Semaphore::new(MAX_CALLS)),
            write_slots: Arc::new(Semaphore::new(2)),
            cleanup_slots: Arc::new(Semaphore::new(2)),
            devices: Mutex::new(std::collections::HashMap::new()),
            resources: Arc::new(Semaphore::new(MAX_RESOURCES)),
            tasks: Mutex::new(JoinSet::new()),
        })
    }
    /// Every slot and permit is free again.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        self.slots.available_permits() == MAX_CALLS
            && self.write_slots.available_permits() == 2
            && self.cleanup_slots.available_permits() == 2
            && self.resources.available_permits() == MAX_RESOURCES
    }
    pub(crate) fn reap(&self) {
        let mut tasks = self.tasks.lock().unwrap();
        while tasks.try_join_next().is_some() {}
    }
    #[cfg(test)]
    pub(crate) fn submit<T: Send + 'static>(
        &self,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> std::result::Result<oneshot::Receiver<T>, &'static str> {
        self.submit_device("test", false, call)
    }
    pub(crate) fn submit_device<T: Send + 'static>(
        &self,
        device: &str,
        write: bool,
        call: impl FnOnce() -> T + Send + 'static,
    ) -> std::result::Result<oneshot::Receiver<T>, &'static str> {
        let device_slots = {
            let mut devices = self.devices.lock().unwrap();
            devices.retain(|_, s| s.strong_count() > 0);
            let key = (device.to_owned(), write);
            devices
                .get(&key)
                .and_then(std::sync::Weak::upgrade)
                .unwrap_or_else(|| {
                    let slots = Arc::new(Semaphore::new(if write { 1 } else { 2 }));
                    devices.insert(key, Arc::downgrade(&slots));
                    slots
                })
        };
        let device_permit = device_slots.try_acquire_owned().map_err(|_| "busy")?;
        let slots = if write {
            &self.write_slots
        } else {
            &self.slots
        };
        let permit = slots.clone().try_acquire_owned().map_err(|_| "busy")?;
        let (send, receive) = oneshot::channel();
        let mut tasks = self.tasks.lock().unwrap();
        while tasks.try_join_next().is_some() {}
        tasks.spawn_blocking(move || {
            let (_permit, _device) = (permit, device_permit);
            // A dropped receiver owns cancellation. A successfully opened Lease
            // in an orphaned result is dropped here and scheduled for release.
            let _ = send.send(call());
        });
        Ok(receive)
    }
    pub(crate) fn reserve(&self) -> std::result::Result<OwnedSemaphorePermit, &'static str> {
        self.resources
            .clone()
            .try_acquire_owned()
            .map_err(|_| "busy")
    }
    fn release(
        &self,
        port: Arc<dyn HerdrPort>,
        resource: Resource,
        permit: OwnedSemaphorePermit,
        full: Option<OwnedSemaphorePermit>,
        audit: Option<Box<crate::audit::Audit>>,
    ) {
        let slots = self.cleanup_slots.clone();
        let mut tasks = self.tasks.lock().unwrap();
        while tasks.try_join_next().is_some() {}
        // At most MAX_RESOURCES cleanup waiters: every one retains its resource
        // permit until reaped. Cleanup has two reserved slots, independent of reads/writes.
        tasks.spawn(async move {
            let cleanup = async move {
                let Ok(slot) = slots.acquire_owned().await else {
                    return;
                };
                let mut release = JoinSet::new();
                release.spawn_blocking(move || {
                    let (_slot, _resource, _full) = (slot, permit, full);
                    port.release(resource);
                });
                let _ = release.join_next().await;
            };
            tokio::pin!(cleanup);
            if tokio::time::timeout(std::time::Duration::from_secs(10), &mut cleanup)
                .await
                .is_err()
            {
                if let Some(audit) = audit {
                    audit.stalled("release");
                }
                // Observation timeout, NOT cancellation: retain task and permits
                // until Host returns. A blocked pipe write cannot be aborted here.
                cleanup.await;
            }
        });
    }
    pub(crate) async fn drain(&self) {
        loop {
            self.reap();
            if self.tasks.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }
}

/// Never joins a Host thread on the Tokio reactor. The global resource permit
/// also bounds pending cleanup, including resources returned after invalidation.
pub(crate) struct Lease {
    jobs: Arc<Jobs>,
    port: Arc<dyn HerdrPort>,
    resource: Option<(Resource, OwnedSemaphorePermit)>,
    full: Option<OwnedSemaphorePermit>,
    audit: Option<Box<crate::audit::Audit>>,
}
impl Lease {
    pub(crate) fn new(
        jobs: Arc<Jobs>,
        port: Arc<dyn HerdrPort>,
        resource: Resource,
        permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            jobs,
            port,
            resource: Some((resource, permit)),
            full: None,
            audit: None,
        }
    }
}
impl Lease {
    pub(crate) fn id(&self) -> &str {
        match &self.resource.as_ref().expect("live resource").0 {
            Resource::Observe(id) | Resource::Events(id) => id,
        }
    }
    pub(crate) fn audit_release(&mut self, audit: crate::audit::Audit) {
        self.audit = Some(Box::new(audit));
    }
    pub(crate) fn hold_full(&mut self, permit: OwnedSemaphorePermit) {
        self.full = Some(permit);
    }
    /// Only on the blocking executor, for ordered control -> Full coordination.
    pub(crate) fn release_now(mut self) {
        if let Some((resource, _permit)) = self.resource.take() {
            self.port.release(resource);
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some((resource, permit)) = self.resource.take() {
            self.jobs.release(
                self.port.clone(),
                resource,
                permit,
                self.full.take(),
                self.audit.take(),
            );
        }
    }
}
