//! Yuzora L4（SSH／SFTP）: SSH hosts, SFTP transfers and tunnels (other lanes reach remote databases through its tunnels).
//!
//! Owned by lane L4. Scripts reach it as `import { … } from "yuzora:ssh"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding in [`js`], with its line in [`DECLARATIONS`].
//!
//! - [`config`]: `~/.ssh/config` (Include, Host/Match, ProxyJump…), read only.
//! - [`hosts`]: config hosts, Yuzora profiles (settings `ssh`) and herdr machines, merged.
//! - [`known_hosts`]: OpenSSH known_hosts; Yuzora writes only its own file
//!   (`<app home>/yuzora/known_hosts`) and reads `~/.ssh`'s.
//! - [`session`]: one shared SSH session per host ([`Manager::session`]), host key
//!   checks, ProxyJump / ProxyCommand; [`auth`] logs in (agent, keys, password,
//!   keyboard-interactive) with questions through [`prompt`].
//! - [`sftp`], [`transfer`], [`edit`]: the SFTP pane's browsing, transfer queue
//!   (resume, preserve) and editing a host's file locally.
//! - [`tunnel`]: local port forwards for other lanes (L3's remote databases).
//! - [`runtime`]: the crate's tokio runtime; its futures can be awaited anywhere.

pub mod auth;
pub mod config;
pub mod context;
pub mod edit;
pub mod error;
pub mod hosts;
pub mod js;
pub mod known_hosts;
pub mod prompt;
pub mod runtime;
pub mod session;
pub mod sftp;
mod strings;
pub mod transfer;
pub mod tunnel;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

pub use context::Context;
pub use error::{ErrorKind, SshError};
pub use js::{DECLARATIONS, NAME, host_module};
pub use session::{HostSession, Interaction, Manager, manager};
