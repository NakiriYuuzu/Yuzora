//! Runtime-provider identity and dispatch seam for Herdr.
//!
//! The provider owns runtime-specific session discovery, capability/snapshot
//! requests, subscriptions, and terminal connector opening. The typed facade in
//! `herdr_service` owns DTO/schema parsing and routes those runtime operations
//! here. This phase deliberately registers only the host-native adapter; WSL is
//! a first-class identity but is rejected until its in-runtime transport exists.

pub(crate) mod native;
pub(crate) mod wsl;

use std::{fmt, path::PathBuf, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::herdr_service::{
    HerdrBinarySource, HerdrBinarySourceInfo, HerdrBinarySourceSetResult, HerdrCapabilities,
    HerdrManager, HerdrNamedSession, HerdrScrollDirection, HerdrSnapshotResult, HerdrTerminalMode,
    HerdrTerminalOpenResult, OnSubscriptionEvent, OnTerminalEvent,
};

pub use native::NativeHerdrRuntimeProvider;
pub use wsl::{HerdrWslDistribution, HerdrWslWorkspaceLocation, WslHerdrRuntimeProvider};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum HerdrRuntimeTarget {
    #[default]
    Native,
    Wsl {
        distro: String,
    },
}

impl HerdrRuntimeTarget {
    pub fn cache_key(&self) -> String {
        match self {
            Self::Native => "native".to_string(),
            Self::Wsl { distro } => format!("wsl:{}:{distro}", distro.len()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrRuntimeKey {
    pub runtime_target: HerdrRuntimeTarget,
    pub session_name: String,
}

impl HerdrRuntimeKey {
    pub fn new(runtime_target: HerdrRuntimeTarget, session_name: impl Into<String>) -> Self {
        Self {
            runtime_target,
            session_name: session_name.into(),
        }
    }

    /// Delimiter-safe cache key. Runtime target and session are distinct
    /// identity dimensions even when names include punctuation or whitespace.
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.runtime_target.cache_key(),
            self.session_name.len(),
            self.session_name
        )
    }
}

/// Runtime-owned operations. Typed command parsing and schema gating stay in
/// the facade; transport/session/socket/connector ownership lives here.
pub trait HerdrRuntimeProvider: Send + Sync {
    fn target(&self) -> HerdrRuntimeTarget;
    fn validate(&self, target: &HerdrRuntimeTarget) -> Result<(), String>;
    fn list_sessions(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
    ) -> Result<Vec<HerdrNamedSession>, String>;
    fn capabilities(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> HerdrCapabilities;
    fn snapshot(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> Result<HerdrSnapshotResult, String>;
    fn request(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
    #[allow(clippy::too_many_arguments)]
    fn open_terminal(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        target: String,
        mode: HerdrTerminalMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        session_name: Option<String>,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String>;
    fn terminal_input(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String>;
    fn terminal_resize(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String>;
    fn terminal_scroll(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        direction: HerdrScrollDirection,
        lines: u32,
    ) -> Result<(), String>;
    fn terminal_release(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
    ) -> Result<(), String>;
    fn subscribe(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        session_name: Option<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String>;
    fn release_subscription(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        subscription_id: &str,
    ) -> Result<(), String>;
}

/// Registry deliberately has one provider in Phase 1. WSL registration cannot
/// become a no-op: the adapter must implement every runtime-owned operation.
pub struct HerdrRuntimeProviderRegistry {
    native: NativeHerdrRuntimeProvider,
    wsl: WslHerdrRuntimeProvider,
}

impl Default for HerdrRuntimeProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HerdrRuntimeProviderRegistry {
    pub fn new() -> Self {
        Self {
            native: NativeHerdrRuntimeProvider::new(),
            wsl: WslHerdrRuntimeProvider,
        }
    }

    fn provider(&self, target: &HerdrRuntimeTarget) -> Result<&dyn HerdrRuntimeProvider, String> {
        match target {
            HerdrRuntimeTarget::Native => Ok(&self.native),
            HerdrRuntimeTarget::Wsl { .. } => Ok(&self.wsl),
        }
    }

    pub fn require(&self, target: &HerdrRuntimeTarget) -> Result<(), String> {
        self.provider(target)?.validate(target)
    }

    /// Native executable source selection is owned by the Native provider, not
    /// by the typed facade. These methods are intentionally unavailable for a
    /// WSL target: a distro's Herdr binary is discovered in that distro.
    pub fn configure_native_binary_paths(
        &self,
        config_dir: PathBuf,
        resource_dir: Option<PathBuf>,
    ) {
        self.native
            .binary_source()
            .configure_paths(config_dir, resource_dir);
    }

    pub fn native_binary_source_info(&self) -> HerdrBinarySourceInfo {
        self.native.binary_source().info()
    }

    pub fn native_configured_binary_source(&self) -> HerdrBinarySource {
        self.native.binary_source().configured_source()
    }

    pub fn set_native_binary_source(
        &self,
        source: HerdrBinarySource,
    ) -> Result<HerdrBinarySourceSetResult, String> {
        self.native.binary_source().set_configured_source(source)
    }

    pub fn resolve_native_binary(&self) -> Option<PathBuf> {
        self.native.binary_source().resolve_active_binary()
    }

    #[cfg(test)]
    pub(crate) fn native_binary_source_for_test(&self) -> &native::NativeBinarySourceState {
        self.native.binary_source()
    }

    pub fn list_sessions(
        &self,
        target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
    ) -> Result<Vec<HerdrNamedSession>, String> {
        let provider = self.provider(target)?;
        provider.validate(target)?;
        provider.list_sessions(target, manager)
    }

    pub fn capabilities(
        &self,
        target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> Result<HerdrCapabilities, String> {
        let provider = self.provider(target)?;
        provider.validate(target)?;
        Ok(provider.capabilities(target, manager, session_name))
    }

    pub fn snapshot(
        &self,
        target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> Result<HerdrSnapshotResult, String> {
        let provider = self.provider(target)?;
        provider.validate(target)?;
        provider.snapshot(target, manager, session_name)
    }

    pub fn request(
        &self,
        target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let provider = self.provider(target)?;
        provider.validate(target)?;
        provider.request(target, manager, session_name, method, params)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_terminal(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        target: String,
        mode: HerdrTerminalMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        session_name: Option<String>,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.open_terminal(
            runtime_target,
            manager,
            target,
            mode,
            takeover,
            cols,
            rows,
            session_name,
            on_event,
        )
    }

    pub fn terminal_input(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.terminal_input(runtime_target, manager, session_id, text, bytes_base64)
    }

    pub fn terminal_resize(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.terminal_resize(runtime_target, manager, session_id, cols, rows)
    }

    pub fn terminal_scroll(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        direction: HerdrScrollDirection,
        lines: u32,
    ) -> Result<(), String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.terminal_scroll(runtime_target, manager, session_id, direction, lines)
    }

    pub fn terminal_release(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
    ) -> Result<(), String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.terminal_release(runtime_target, manager, session_id)
    }

    pub fn subscribe(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        session_name: Option<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.subscribe(runtime_target, manager, session_name, on_event)
    }

    pub fn release_subscription(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        subscription_id: &str,
    ) -> Result<(), String> {
        let provider = self.provider(runtime_target)?;
        provider.validate(runtime_target)?;
        provider.release_subscription(runtime_target, manager, subscription_id)
    }
}

impl fmt::Display for HerdrRuntimeTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native => formatter.write_str("native"),
            Self::Wsl { distro } => write!(formatter, "wsl:{distro}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_keys_keep_native_and_wsl_sessions_distinct() {
        let native = HerdrRuntimeKey::new(HerdrRuntimeTarget::Native, "default");
        let ubuntu = HerdrRuntimeKey::new(
            HerdrRuntimeTarget::Wsl {
                distro: "Ubuntu".to_string(),
            },
            "default",
        );
        assert_ne!(native, ubuntu);
        assert_ne!(native.cache_key(), ubuntu.cache_key());
    }

    #[test]
    fn runtime_key_is_delimiter_safe_for_whitespace_and_unicode() {
        let first = HerdrRuntimeKey::new(
            HerdrRuntimeTarget::Wsl {
                distro: "Ubuntu:開発".to_string(),
            },
            "default: session / α",
        );
        let second = HerdrRuntimeKey::new(
            HerdrRuntimeTarget::Wsl {
                distro: "Ubuntu".to_string(),
            },
            "開発:default: session / α",
        );
        assert_ne!(first.cache_key(), second.cache_key());
    }

    #[test]
    fn registry_routes_wsl_to_a_dedicated_adapter() {
        let registry = HerdrRuntimeProviderRegistry::new();
        assert!(registry.require(&HerdrRuntimeTarget::Native).is_ok());
        let wsl = HerdrRuntimeTarget::Wsl {
            distro: "Ubuntu".to_string(),
        };
        #[cfg(windows)]
        assert!(registry.require(&wsl).is_ok());
        #[cfg(not(windows))]
        assert!(registry.require(&wsl).is_err());
    }
}
