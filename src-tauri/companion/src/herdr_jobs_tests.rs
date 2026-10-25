use super::*;
use crate::herdr::Result;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use yuzora_host::herdr_service::{
    HerdrNamedSession, HerdrSnapshotResult, OnSubscriptionEvent, OnTerminalEvent,
};
struct Gate(Arc<AtomicBool>);
impl Drop for Gate {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn bridge_jobs_four_active_calls_orphans_are_owned_and_drained() {
    let jobs = Jobs::new();
    let gate = Gate(Arc::new(AtomicBool::new(true)));
    let mut pending = Vec::new();
    for i in 0..4 {
        let blocked = gate.0.clone();
        pending.push(
            jobs.submit_device(&format!("device-{i}"), false, move || {
                while blocked.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                7
            })
            .unwrap(),
        );
    }
    assert_eq!(jobs.submit(|| 7).unwrap_err(), "busy");
    drop(pending);
    assert_eq!(jobs.slots.available_permits(), 0);
    drop(gate);
    tokio::time::timeout(Duration::from_secs(3), jobs.drain())
        .await
        .unwrap();
    assert_eq!(jobs.slots.available_permits(), 4);
    assert!(jobs.tasks.lock().unwrap().is_empty());
}
#[derive(Default)]
struct Releasing {
    active: AtomicUsize,
    max: AtomicUsize,
    released: AtomicUsize,
}
impl HerdrPort for Releasing {
    crate::test_support::reject_writes!();
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>> {
        unreachable!()
    }
    fn snapshot(&self, _: &str) -> Result<HerdrSnapshotResult> {
        unreachable!()
    }
    fn observe(&self, _: &str, _: &str, _: u16, _: u16, _: OnTerminalEvent) -> Result<String> {
        unreachable!()
    }
    fn subscribe(&self, _: &str, _: Vec<String>, _: OnSubscriptionEvent) -> Result<String> {
        unreachable!()
    }
    fn release(&self, _: Resource) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn bridge_jobs_cleanup_capacity_and_concurrency_share_four_slots() {
    let jobs = Jobs::new();
    let port = Arc::new(Releasing::default());
    let mut leases = Vec::new();
    for id in 0..128 {
        leases.push(Lease::new(
            jobs.clone(),
            port.clone(),
            Resource::Observe(id.to_string()),
            jobs.reserve().unwrap(),
        ));
    }
    assert_eq!(jobs.reserve().unwrap_err(), "busy");
    drop(leases);
    assert!(jobs.tasks.lock().unwrap().len() <= 128);
    assert_eq!(jobs.resources.available_permits(), 0);
    tokio::time::timeout(Duration::from_secs(3), jobs.drain())
        .await
        .unwrap();
    assert_eq!(port.released.load(Ordering::SeqCst), 128);
    assert!(port.max.load(Ordering::SeqCst) <= 4);
    assert_eq!(jobs.resources.available_permits(), 128);
}

#[tokio::test]
async fn bridge_jobs_device_quota_retained_by_orphans_and_write_slots_independent() {
    let jobs = Jobs::new();
    let gate = Gate(Arc::new(AtomicBool::new(true)));
    let mut pending = Vec::new();
    for (index, device) in ["a", "a", "b", "b"].into_iter().enumerate() {
        if index == 2 {
            pending.clear(); // Orphans still charge the device, not its dead peer.
            assert_eq!(jobs.submit_device("a", false, || ()).unwrap_err(), "busy");
        }
        let blocked = gate.0.clone();
        pending.push(
            jobs.submit_device(device, false, move || {
                while blocked.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .unwrap(),
        );
    }
    drop(pending);
    assert_eq!(jobs.submit_device("a", false, || ()).unwrap_err(), "busy");
    assert_eq!(jobs.submit_device("c", false, || ()).unwrap_err(), "busy");
    // Reads, including this device's orphaned calls, cannot consume write slots.
    assert_eq!(
        jobs.submit_device("a", true, || 7).unwrap().await.unwrap(),
        7
    );
    drop(gate);
    jobs.drain().await;
    assert_eq!(
        jobs.submit_device("a", false, || 8).unwrap().await.unwrap(),
        8
    );
}
/// A device's next write is submitted as soon as the previous result arrives
/// (the write pump): by then the previous job's permits must be back, or the
/// next write is refused as `busy` (seen on 2-vCPU Linux runners). The result
/// is polled in a spin so the next job follows it at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_jobs_permits_return_before_the_result_is_delivered() {
    let jobs = Jobs::new();
    for round in 0..2000 {
        let mut result = jobs
            .submit_device("a", true, move || round)
            .unwrap_or_else(|code| panic!("round {round}: {code}"));
        let value = loop {
            match result.try_recv() {
                Ok(value) => break value,
                Err(oneshot::error::TryRecvError::Empty) => std::hint::spin_loop(),
                Err(oneshot::error::TryRecvError::Closed) => panic!("round {round}: dropped"),
            }
        };
        assert_eq!(value, round);
    }
}
