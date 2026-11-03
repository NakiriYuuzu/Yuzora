//! The crate's own tokio runtime. russh and russh-sftp need tokio; the app's
//! mods run on GPUI's executor, and other lanes may run their own runtime, so
//! every piece of SSH work runs here and hands its result back through a
//! oneshot, which any executor can await.

use std::future::Future;
use std::sync::OnceLock;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("yuzora-ssh")
            .enable_all()
            .build()
            .expect("the SSH runtime starts")
    })
}

/// The runtime's handle, for spawning.
pub fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}

/// Runs `work` on the SSH runtime. The returned future can be awaited on any
/// executor; it yields `None` only if the work panicked.
pub fn bridge<T: Send + 'static>(work: impl Future<Output = T> + Send + 'static) -> impl Future<Output = Option<T>> + Send + 'static {
    let (tx, rx) = tokio::sync::oneshot::channel();
    handle().spawn(async move {
        let _ = tx.send(work.await);
    });
    async move { rx.await.ok() }
}

/// Runs `work` on the SSH runtime and blocks this thread until it is done.
/// Not for use inside an async task (it parks the thread).
pub fn block_on<T: Send + 'static>(work: impl Future<Output = T> + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    handle().spawn(async move {
        let _ = tx.send(work.await);
    });
    rx.recv().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridged_work_resolves_off_the_runtime() {
        assert_eq!(block_on(async { 2 + 3 }), Some(5));
        let fut = bridge(async { tokio::time::sleep(std::time::Duration::from_millis(5)).await; "done" });
        // Awaited by a different runtime, as another lane's would be.
        let other = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(other.block_on(fut), Some("done"));
    }
}
