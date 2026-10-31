//! Client for herdr-core (`hc`), the experimental second terminal runtime.
//!
//! Only the public API (protocol generation 1) is used: JSON-RPC 2.0 over a Unix socket or
//! Windows named pipe, with a framed mode for attach streams and events. Nothing here reads
//! or writes a core's state directory, and nothing replaces or stops a core Yuzora did not
//! start.
//!
//! * Pure logic: [`frame`], [`rpc`], [`caps`], [`events`], [`docs`], [`paths`], [`status`].
//! * [`client`]: one [`client::Session`] per core session, over two connections. The plain one
//!   only runs RPC (a plain connection that subscribes to events stops answering RPC); the
//!   framed one carries the events channel and every attach stream.
//! * [`startup`]: the environment every `herdr-core` child gets, the start record, and the
//!   [`startup::Supervisor`] that decides when the bundled core is started.

pub mod caps;
pub mod client;
pub mod docs;
pub mod events;
pub mod frame;
pub mod paths;
pub mod rpc;
pub mod startup;
pub mod status;

#[cfg(test)]
mod attach_replay;
#[cfg(test)]
mod client_tests;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod real_core;
#[cfg(test)]
mod testkit;
