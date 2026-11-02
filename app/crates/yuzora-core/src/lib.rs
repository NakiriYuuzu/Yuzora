//! Yuzora's shared layer under every lane crate and Yuzora mod.
//!
//! - [`paths`]: the app home (`~/.config/yuzora`, `YUZORA_APP_HOME`).
//! - [`remote`]: one program here or on a machine (`hc exec --on`), with a timeout.
//! - [`secrets`]: the system credential store.
//! - [`i18n`]: en / zh-TW strings per mod or crate, the language setting.
//! - [`settings`]: Yuzora's `settings.json`, one section per lane.
//! - [`discovery`]: tool panes and settings pages declared in `mod.json`.
//! - [`tokens`]: design tokens a theme has no name for.
//! - [`host`]: helpers for a lane's `host_module()`.
//! - [`js`]: the `yuzora:core` module itself.
//! - [`migrate`]: bringing data over from the Tauri app (0.1.0-beta.1 and older; L7).

pub mod discovery;
pub mod host;
pub mod i18n;
pub mod js;
pub mod migrate;
pub mod paths;
pub mod remote;
pub mod secrets;
pub mod settings;
pub mod tokens;

pub use gpui_shell::{HostArguments, HostError, HostModule, HostObject, HostValue};

/// Yuzora's version: the app's, and every yuzora-* crate's.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `yuzora:core` for one mod copy; `mod_name` picks the strings `t(key)` reads.
pub fn host_module_for(mod_name: &str) -> HostModule {
    js::module(mod_name)
}

/// `yuzora:core` outside any mod (`herdr-app check`, tests).
pub fn host_module() -> HostModule {
    host_module_for("")
}
