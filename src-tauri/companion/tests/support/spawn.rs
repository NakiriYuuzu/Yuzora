//! Child processes and pipe closes on parallel test threads, one at a time.
//!
//! Two windows let one thread's pipe end reach another thread's child. One lock
//! around every spawn, and around a close the test depends on, shuts both:
//! - macOS has no `pipe2`: std makes a `Stdio::piped()` pipe with `pipe()` and
//!   marks both ends close-on-exec in a second step. A spawn that lands between
//!   the two hands those ends to its child, which then holds them for good.
//! - A spawn holds a copy of every open fd, close-on-exec or not, until its exec
//!   completes. An end closed meanwhile stays open in that copy for a moment.
//!
//! A gateway that holds the read end of a closed stdout never sees EPIPE, and
//! one holding the stdin write end never sees EOF.
use std::{
    io,
    process::{Child, Command},
    sync::{Mutex, MutexGuard},
};

static SPAWN: Mutex<()> = Mutex::new(());

// A panicking sibling test must not fail the others.
fn exclusive() -> MutexGuard<'static, ()> {
    SPAWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn serialized(command: &mut Command) -> io::Result<Child> {
    let _only = exclusive();
    command.spawn()
}

/// Closes a pipe end whose closing the test needs the child to see at once,
/// like the one write the child makes to a closed stdout.
pub fn close<T>(end: T) {
    let _only = exclusive();
    drop(end);
}
