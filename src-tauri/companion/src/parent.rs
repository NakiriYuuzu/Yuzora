//! Whether the desktop that started this process is still there.
//!
//! Stdin EOF is the gateway's normal signal that the desktop is gone, but a copy
//! of the stdin write end that leaked into another process keeps the pipe open
//! for good. The parent pid is the second, independent signal. There is no check
//! on Windows: its parent id is a snapshot that never changes.
use std::time::Duration;

/// How often the parent is looked at.
pub const POLL: Duration = Duration::from_secs(1);
/// How long the gateway keeps serving once the parent is gone. The tsnet sidecar
/// polls faster, so it has unserved the node before the gateway lets go of its port.
pub const GONE_GRACE: Duration = Duration::from_millis(500);

pub type Probe = Box<dyn Fn() -> bool + Send>;

/// macOS hands every orphan to launchd (pid 1) and has no subreapers, and a
/// debugger that attaches also becomes the parent, which must not stop the
/// process. Elsewhere an orphan goes to init or the nearest subreaper, so any
/// change from the pid seen at start counts, but a start under pid 1 can be a
/// live desktop in a pid namespace.
#[cfg(any(unix, test))]
fn gone(macos: bool, baseline: u32, current: u32) -> bool {
    if macos {
        current == 1
    } else {
        current != baseline
    }
}

/// Records the parent pid now: call it first thing in `main`, because the first
/// command may arrive five seconds later.
#[cfg(unix)]
pub fn probe() -> Option<Probe> {
    probe_with(std::os::unix::process::parent_id)
}

/// `probe` over any reader of the parent pid, so a test can move the parent.
#[cfg(unix)]
fn probe_with(parent_id: impl Fn() -> u32 + Send + 'static) -> Option<Probe> {
    let baseline = parent_id();
    Some(Box::new(move || {
        gone(cfg!(target_os = "macos"), baseline, parent_id())
    }))
}

#[cfg(not(unix))]
pub fn probe() -> Option<Probe> {
    None
}

/// Completes when the parent is gone, and never without a probe. Poll it by
/// `&mut` from one `select!` that outlives the loop: an arm that completes drops
/// the other arms' futures, and a dropped `admin::read` loses a half-read frame.
pub async fn watch(probe: Option<Probe>, every: Duration) {
    let Some(probe) = probe else {
        return std::future::pending().await;
    };
    while !probe() {
        tokio::time::sleep(every).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::atomic::AtomicU32;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::time::timeout;

    #[test]
    fn macos_means_launchd_and_ignores_a_debugger() {
        assert!(gone(true, 4242, 1));
        assert!(gone(true, 1, 1));
        assert!(!gone(true, 4242, 4242));
        assert!(!gone(true, 4242, 9999));
    }

    #[test]
    fn elsewhere_means_a_changed_parent() {
        assert!(gone(false, 4242, 1));
        assert!(gone(false, 4242, 9999));
        assert!(!gone(false, 4242, 4242));
        assert!(!gone(false, 1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn the_live_parent_is_not_gone() {
        assert!(!probe().expect("unix probes the parent")());
    }

    /// A probe over a parent pid that the test moves: it starts at `start`.
    #[cfg(unix)]
    fn moving_parent(start: u32) -> (Arc<AtomicU32>, Probe) {
        let pid = Arc::new(AtomicU32::new(start));
        let reader = pid.clone();
        let probe = probe_with(move || reader.load(Ordering::SeqCst));
        (pid, probe.expect("unix probes the parent"))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_follows_the_macos_rule() {
        let (pid, probe) = moving_parent(4242);
        assert!(!probe());
        // A debugger that attached is the parent now: the desktop is still there.
        pid.store(9999, Ordering::SeqCst);
        assert!(!probe(), "a changed parent is not launchd");
        // The desktop crashed and launchd adopted it.
        pid.store(1, Ordering::SeqCst);
        assert!(probe(), "launchd is the parent now");
        // Started under launchd: the desktop was gone before the first read.
        let (_, probe) = moving_parent(1);
        assert!(probe(), "a start under pid 1 is an orphan");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_probe_follows_the_rule_elsewhere() {
        let (pid, probe) = moving_parent(4242);
        assert!(!probe());
        // Init or a subreaper adopted the process.
        pid.store(9999, Ordering::SeqCst);
        assert!(probe(), "any changed parent is a gone parent");
        pid.store(1, Ordering::SeqCst);
        assert!(probe(), "init is a changed parent too");
        // A live desktop can be pid 1 in a pid namespace.
        let (_, probe) = moving_parent(1);
        assert!(!probe(), "a start under pid 1 is not an orphan");
    }

    #[tokio::test]
    async fn watch_completes_once_the_probe_says_so() {
        let flag = Arc::new(AtomicBool::new(false));
        let seen = flag.clone();
        let probe: Probe = Box::new(move || seen.load(Ordering::SeqCst));
        let watching = tokio::spawn(watch(Some(probe), Duration::from_millis(5)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!watching.is_finished());
        flag.store(true, Ordering::SeqCst);
        timeout(Duration::from_secs(2), watching)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn watch_sees_a_parent_that_is_gone_from_the_start() {
        timeout(
            Duration::from_secs(2),
            watch(Some(Box::new(|| true)), Duration::from_secs(60)),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn watch_without_a_probe_never_completes() {
        assert!(timeout(
            Duration::from_millis(100),
            watch(None, Duration::from_millis(5))
        )
        .await
        .is_err());
    }
}
