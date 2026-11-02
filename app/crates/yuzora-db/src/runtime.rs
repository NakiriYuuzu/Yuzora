//! The one tokio runtime every driver runs on.
//!
//! A host async function's future is driven by GPUI's background executor,
//! which is not tokio: tokio-postgres, tiberius and our timers need a tokio
//! reactor. So the work is spawned here and the host future only awaits a
//! runtime-agnostic channel ([`run`]).

use std::future::Future;
use std::sync::OnceLock;

use crate::error::DbError;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("yuzora-db")
            .enable_all()
            .build()
            .expect("the yuzora-db runtime starts")
    })
}

/// Starts `work` on the runtime; the returned future (any executor) resolves
/// with its result. Dropping it does not stop the work.
pub fn run<T, F>(work: F) -> impl Future<Output = Result<T, DbError>> + Send + 'static
where
    T: Send + 'static,
    F: Future<Output = Result<T, DbError>> + Send + 'static,
{
    let (tx, rx) = futures_channel::oneshot::channel();
    runtime().spawn(async move {
        let _ = tx.send(work.await);
    });
    async move { rx.await.unwrap_or_else(|_| Err(DbError::Cancelled)) }
}

/// Starts `work` and forgets it.
pub fn spawn<F>(work: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(work);
}

/// Runs `work` to completion on the runtime from a plain thread (tests, `Drop`).
pub fn block_on<F: Future>(work: F) -> F::Output {
    runtime().block_on(work)
}

/// The runtime's handle, for code that has to enter it (rusqlite's blocking pool).
pub fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}
