use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
thread_local! { static CANCELLATION: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) }; }

/// All subprocesses in a synchronous typed Git operation share its cancellation.
pub fn with_cancellation<T>(cancelled: Arc<AtomicBool>, task: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<AtomicBool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CANCELLATION.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CANCELLATION.with(|slot| slot.replace(Some(cancelled))));
    task()
}

pub fn token() -> Arc<AtomicBool> {
    CANCELLATION
        .with(|slot| slot.borrow().clone())
        .unwrap_or_default()
}
pub fn check() -> Result<(), String> {
    if token().load(Ordering::Acquire) {
        Err("operation-cancelled".into())
    } else {
        Ok(())
    }
}
