//! Herdr runtime lane: public NDJSON API + official terminal session connectors.
//!
//! Authority is the selected installed `herdr` binary (discover/interrogate at
//! runtime). Never hardcode a protocol number, stop a Herdr server, or kill
//! Herdr panes — only Yuzora-owned connector children are released/terminated.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::herdr_limits::{
    bound_agent_text, bound_optional_json, bounded_ipc, ensure_ipc_bound, parse_herdr_cli_stdout,
    read_bounded_bytes, read_bounded_ndjson_line, validate_json_complexity,
    validate_snapshot_counts, BoundedNdjsonReadError, HerdrProtocolError, MAX_AGENT_MANIFEST_COUNT,
    MAX_LAYOUT_DEPTH, MAX_NDJSON_LINE_BYTES, MAX_SESSION_COUNT, MAX_STATE_LABELS,
    MAX_WORKTREE_COUNT,
};
use crate::herdr_transport::{connect_local_stream, read_local_ndjson_line, write_local_all_until};
use crate::process_kill;

pub type OnTerminalEvent = Arc<dyn Fn(HerdrTerminalEvent) -> Result<(), String> + Send + Sync>;
pub type OnSubscriptionEvent =
    Arc<dyn Fn(HerdrSubscriptionEvent) -> Result<(), String> + Send + Sync>;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);
const BINARY_SOURCE_CONFIG_FILE: &str = "herdr-config-v1.json";
#[cfg(not(test))]
const EVENT_ACK_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const EVENT_ACK_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(test)]
const TEST_EVENT_RECV_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOCAL_IO_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_READ_MIN_LINES: u32 = 20;
const AGENT_READ_MAX_LINES: u32 = 500;
const AGENT_START_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
const AGENT_START_RETRY_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(not(test))]
const HERDR_CLI_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const HERDR_CLI_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const HERDR_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const HERDR_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(not(test))]
const HERDR_STARTUP_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const HERDR_STARTUP_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
const HERDR_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);

// ── Public DTOs (Yuzora IPC, camelCase) ─────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrCapabilities {
    pub binary_path: Option<String>,
    pub binary_version: Option<String>,
    /// Protocol advertised by the selected binary (status/schema), never hardcoded.
    pub binary_protocol: Option<u32>,
    pub channel: Option<String>,
    /// App-global binary source preference and resolution diagnostics.
    pub binary_source: HerdrBinarySourceInfo,
    pub server: HerdrServerCapability,
    pub api: HerdrApiCapability,
    pub terminal: HerdrTerminalCapability,
    pub events: HerdrEventsCapability,
}

/// App-global preference: PATH-installed vs Yuzora-managed resource binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HerdrBinarySource {
    #[default]
    Global,
    Default,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrBinarySourceInfo {
    /// Preference persisted for the next app start.
    pub configured: HerdrBinarySource,
    /// Source frozen for the current process.
    pub active: HerdrBinarySource,
    /// Active source when it resolves to an executable.
    pub resolved: Option<HerdrBinarySource>,
    /// Active-source diagnostics retained for compatibility with existing clients.
    pub available: bool,
    pub path: Option<String>,
    pub reason: Option<String>,
    pub version: Option<String>,
    pub protocol: Option<u32>,
    /// Configured-target diagnostics, which may differ until restart.
    pub configured_available: bool,
    pub configured_path: Option<String>,
    pub configured_reason: Option<String>,
    pub configured_version: Option<String>,
    pub configured_protocol: Option<u32>,
    /// Persistence load failure; missing config is not an error.
    pub configuration_error: Option<String>,
    /// True when a set-source call was persisted but needs app restart to take effect.
    pub restart_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrBinarySourceSetResult {
    pub configured: HerdrBinarySource,
    pub restart_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HerdrReadSource {
    Visible,
    Recent,
    #[serde(rename = "recent-unwrapped")]
    RecentUnwrapped,
    Detection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrReadFormat {
    Text,
    Ansi,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrAgentDetails {
    pub terminal_id: String,
    pub agent_status: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    pub revision: u64,
    pub agent: Option<String>,
    pub display_agent: Option<String>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub foreground_cwd: Option<String>,
    pub interactive_ready: Option<bool>,
    pub launch_pending: Option<bool>,
    pub state_labels: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrAgentReadResult {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub source: HerdrReadSource,
    pub format: HerdrReadFormat,
    pub text: String,
    pub revision: u64,
    pub truncated: bool,
    /// True when Yuzora refused to deliver the full agent text (over 512 KiB).
    pub too_large: bool,
}

/// One server-advertised Agent kind, enriched with advisory host PATH detection.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrAgentCatalogEntry {
    pub agent: String,
    pub source: String,
    pub source_kind: String,
    pub active_version: Option<String>,
    pub warning: Option<String>,
    pub detected_binary_path: Option<String>,
    pub bypass_flags: Vec<String>,
}

/// Transactional result of `tab.create` followed by schema-gated `agent.start`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrAgentCreateResult {
    pub name: String,
    pub kind: String,
    pub terminal_id: String,
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
}

/// Events delivered over the Tauri Channel for `herdr_events_subscribe`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HerdrSubscriptionEvent {
    #[serde(rename = "subscribed")]
    Subscribed { subscription_id: String },
    #[serde(rename = "agent_status_changed")]
    AgentStatusChanged {
        subscription_id: String,
        pane_id: String,
        workspace_id: String,
        agent_status: String,
        agent: Option<String>,
        display_agent: Option<String>,
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        execution_origin: Option<serde_json::Value>,
        state_labels: HashMap<String, String>,
    },
    #[serde(rename = "pane_exited")]
    PaneExited {
        subscription_id: String,
        pane_id: String,
        workspace_id: String,
    },
    /// Dirty signal for worktree.created/opened/removed — frontend re-lists inventory.
    #[serde(rename = "worktree_changed")]
    WorktreeChanged {
        subscription_id: String,
        kind: String,
        workspace_id: Option<String>,
    },
    /// Dirty signal for tab/workspace topology — frontend refreshes snapshot.
    #[serde(rename = "topology_changed")]
    TopologyChanged {
        subscription_id: String,
        kind: String,
        workspace_id: Option<String>,
        tab_id: Option<String>,
    },
    #[serde(rename = "error")]
    Error {
        subscription_id: String,
        message: String,
    },
    #[serde(rename = "disconnected")]
    Disconnected {
        subscription_id: String,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrServerCapability {
    pub running: bool,
    pub version: Option<String>,
    pub protocol: Option<u32>,
    pub compatible: Option<bool>,
    pub socket_path: Option<String>,
    pub capabilities: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrApiCapability {
    /// Public NDJSON socket methods we safely implement.
    pub snapshot: bool,
    pub ping: bool,
    /// `tab.create` → root pane/terminal identity (protocol-19 `tab_created`).
    pub tab_create: bool,
    /// `workspace.focus { workspace_id }` for Space activation.
    pub workspace_focus: bool,
    /// `workspace.create { cwd, label, focus }` for Rail + New Space.
    pub workspace_create: bool,
    pub workspace_rename: bool,
    pub workspace_close: bool,
    pub tab_rename: bool,
    pub tab_close: bool,
    pub tab_focus: bool,
    /// Protocol-19 `tab.move { tab_id, insert_index }`.
    pub tab_move: bool,
    pub pane_focus: bool,
    pub pane_rename: bool,
    pub pane_split: bool,
    pub pane_zoom: bool,
    pub pane_swap: bool,
    pub pane_close: bool,
    pub layout_export: bool,
    pub layout_set_split_ratio: bool,
    /// Server-advertised Agent manifest catalog.
    pub agent_manifests: bool,
    /// Starts a validated manifest kind in a freshly-created pane.
    pub agent_start: bool,
    /// Read-only `agent.get` (explicit pane/name target).
    pub agent_get: bool,
    /// Read-only `agent.read` (explicit pane/name target).
    pub agent_read: bool,
    /// Long-lived `events.subscribe` socket lane.
    pub events_subscribe: bool,
    /// Schema-gated read-only `worktree.list` (protocol 19).
    pub worktree_list: bool,
    /// Advertised method names available for the selected running session
    /// (schema-gated). Frontend menus disable honestly from this list.
    pub methods: Vec<String>,
    pub schema_protocol: Option<u32>,
    pub schema_version: Option<u32>,
    pub reason: Option<String>,
}

/// Named persistent Herdr session from `herdr session list --json`.
/// Socket paths come only from that listing — never guessed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrNamedSession {
    pub name: String,
    pub default: bool,
    pub running: bool,
    pub session_dir: String,
    pub socket_path: String,
}

/// Result of public `workspace.create`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWorkspaceCreateResult {
    pub workspace_id: String,
    pub label: String,
    pub path: Option<String>,
    pub tab_id: Option<String>,
    pub terminal_id: Option<String>,
    pub pane_id: Option<String>,
}

/// Protocol-19 `WorktreeSourceInfo` (camelCase IPC).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWorktreeSourceInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub source_checkout_path: String,
    pub source_workspace_id: Option<String>,
}

/// Protocol-19 `WorktreeInfo` (camelCase IPC).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWorktreeInfo {
    pub path: String,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
    pub is_linked_worktree: bool,
    pub label: String,
    pub open_workspace_id: Option<String>,
}

/// Result of schema-gated `worktree.list`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWorktreeListResult {
    pub source: HerdrWorktreeSourceInfo,
    pub worktrees: Vec<HerdrWorktreeInfo>,
}

/// Pane identity returned by `pane.split` / `pane.focus` (`pane_info`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrPaneIdentity {
    pub pane_id: String,
    pub terminal_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
}

/// `pane.split` direction (protocol-19 `SplitDirection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrSplitDirection {
    Right,
    Down,
}

/// `pane.zoom` mode (protocol-19 `PaneZoomMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrPaneZoomMode {
    Toggle,
    On,
    Off,
}

/// Recursive BSP node from `layout.export` / `layout.set_split_ratio`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum HerdrLayoutNode {
    #[serde(rename = "pane")]
    Pane {
        pane_id: Option<String>,
        label: Option<String>,
        cwd: Option<String>,
    },
    #[serde(rename = "split")]
    Split {
        direction: HerdrSplitDirection,
        ratio: f64,
        first: Box<HerdrLayoutNode>,
        second: Box<HerdrLayoutNode>,
    },
}

/// Protocol-19 `LayoutDescription` (camelCase IPC).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrLayoutDescription {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub focused_pane_id: String,
    pub root: HerdrLayoutNode,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrTerminalCapability {
    pub observe: bool,
    pub control: bool,
    pub takeover: bool,
    pub input: bool,
    pub resize: bool,
    pub scroll: bool,
    pub release: bool,
    pub create: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrEventsCapability {
    /// Availability of the long-lived local-socket event lane.
    pub status: HerdrEventsStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HerdrEventsStatus {
    Deferred,
    Available,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrSnapshotResult {
    pub protocol: u32,
    pub version: String,
    /// Full Herdr snapshot object (snake_case wire fields preserved).
    pub snapshot: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HerdrTerminalMode {
    Observe,
    Control,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrTerminalOpenResult {
    pub session_id: String,
    pub target: String,
    pub mode: HerdrTerminalMode,
    pub role: HerdrTerminalRole,
    pub cols: u16,
    pub rows: u16,
    pub takeover: bool,
}

/// Result of public `tab.create` — root pane + live terminal identity.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrTerminalCreateResult {
    pub terminal_id: String,
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HerdrTerminalRole {
    Observer,
    Controller,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum HerdrTerminalEvent {
    Frame {
        session_id: String,
        seq: u64,
        full: bool,
        encoding: String,
        width: u32,
        height: u32,
        /// Base64 payload as emitted by `herdr terminal session …` (ANSI).
        bytes_base64: String,
    },
    Closed {
        session_id: String,
        reason: Option<String>,
    },
    /// Contiguous-seq gap or first-frame-not-full — frontend should reopen/resync.
    Resync {
        session_id: String,
        expected_seq: Option<u64>,
        received_seq: Option<u64>,
        message: String,
    },
    Error {
        session_id: String,
        code: String,
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrScrollDirection {
    Up,
    Down,
}

// ── Wire helpers (Herdr public NDJSON / connector frames) ───────────────────

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct HerdrWireFrame {
    #[serde(rename = "type")]
    pub kind: String,
    seq: Option<u64>,
    full: Option<bool>,
    encoding: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    bytes: Option<String>,
    reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedTerminalFrame {
    pub seq: u64,
    pub full: bool,
    pub encoding: String,
    pub width: u32,
    pub height: u32,
    pub bytes_base64: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FrameDecision {
    Accept(ParsedTerminalFrame),
    IgnoreDuplicate {
        seq: u64,
    },
    Resync {
        expected_seq: Option<u64>,
        received_seq: Option<u64>,
        message: String,
    },
    Closed {
        reason: Option<String>,
    },
    Ignore,
}

#[derive(Clone, Debug, Default)]
pub struct FrameTracker {
    last_seq: Option<u64>,
    accepted_any: bool,
}

impl FrameTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frame rules: first accepted frame must be full; seq contiguous;
    /// duplicates ignored; gaps become typed resync.
    pub fn ingest_wire(&mut self, wire: &HerdrWireFrame) -> FrameDecision {
        match wire.kind.as_str() {
            "terminal.closed" => FrameDecision::Closed {
                reason: wire.reason.clone(),
            },
            "terminal.frame" => self.ingest_frame(wire),
            _ => FrameDecision::Ignore,
        }
    }

    pub fn ingest_frame(&mut self, wire: &HerdrWireFrame) -> FrameDecision {
        let Some(seq) = wire.seq else {
            return FrameDecision::Resync {
                expected_seq: self.next_expected(),
                received_seq: None,
                message: "terminal.frame missing seq".into(),
            };
        };
        let full = wire.full.unwrap_or(false);
        let encoding = wire.encoding.clone().unwrap_or_else(|| "ansi".to_string());
        let width = wire.width.unwrap_or(0);
        let height = wire.height.unwrap_or(0);
        let Some(bytes_base64) = wire.bytes.clone() else {
            return FrameDecision::Resync {
                expected_seq: self.next_expected(),
                received_seq: Some(seq),
                message: "terminal.frame missing bytes".into(),
            };
        };

        if let Some(last) = self.last_seq {
            if seq == last {
                return FrameDecision::IgnoreDuplicate { seq };
            }
            if seq != last + 1 {
                return FrameDecision::Resync {
                    expected_seq: Some(last + 1),
                    received_seq: Some(seq),
                    message: format!("terminal.frame seq gap: expected {}, got {seq}", last + 1),
                };
            }
        } else if !full {
            return FrameDecision::Resync {
                expected_seq: Some(seq),
                received_seq: Some(seq),
                message: "first terminal.frame must be full".into(),
            };
        }

        self.last_seq = Some(seq);
        self.accepted_any = true;
        FrameDecision::Accept(ParsedTerminalFrame {
            seq,
            full,
            encoding,
            width,
            height,
            bytes_base64,
        })
    }

    fn next_expected(&self) -> Option<u64> {
        self.last_seq.map(|s| s + 1)
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(tag = "type")]
pub enum TerminalControlCommand {
    #[serde(rename = "terminal.input")]
    InputText { text: String },
    #[serde(rename = "terminal.input")]
    InputBytes {
        #[serde(rename = "bytes")]
        bytes_base64: String,
    },
    #[serde(rename = "terminal.resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: HerdrScrollDirection,
        lines: u32,
    },
    #[serde(rename = "terminal.release")]
    Release,
}

impl TerminalControlCommand {
    pub fn input(text: Option<String>, bytes_base64: Option<String>) -> Result<Self, String> {
        match (text, bytes_base64) {
            (Some(text), None) => Ok(Self::InputText { text }),
            (None, Some(bytes_base64)) => Ok(Self::InputBytes { bytes_base64 }),
            (Some(_), Some(_)) => Err("terminal.input accepts text or bytes, not both".into()),
            (None, None) => Err("terminal.input requires text or bytes".into()),
        }
    }

    pub fn resize(cols: u16, rows: u16) -> Result<Self, String> {
        if cols == 0 || rows == 0 {
            return Err("terminal.resize cols and rows must be greater than 0".into());
        }
        Ok(Self::Resize { cols, rows })
    }

    pub fn scroll(direction: HerdrScrollDirection, lines: u32) -> Result<Self, String> {
        if lines == 0 {
            return Err("terminal.scroll lines must be greater than 0".into());
        }
        Ok(Self::Scroll { direction, lines })
    }

    pub fn to_json_line(&self) -> Result<String, String> {
        let mut line = serde_json::to_string(self).map_err(|e| e.to_string())?;
        line.push('\n');
        Ok(line)
    }
}

// ── Manager / connectors ────────────────────────────────────────────────────

pub struct HerdrState(pub Arc<HerdrManager>);

pub struct HerdrManager {
    sessions: Mutex<HashMap<String, Arc<ConnectorSession>>>,
    event_subscriptions: Mutex<HashMap<String, Arc<EventSubscription>>>,
    /// Optional override for tests / explicit binary selection.
    binary_override: Mutex<Option<PathBuf>>,
    /// Optional managed-resource override for tests of `default` source.
    managed_binary_override: Mutex<Option<PathBuf>>,
    socket_override: Mutex<Option<PathBuf>>,
    /// App data dir for binary-source preference persistence.
    config_dir: Mutex<Option<PathBuf>>,
    /// Tauri resource dir for Yuzora-managed default binary lookup.
    resource_dir: Mutex<Option<PathBuf>>,
    /// Preference loaded from disk / set by user (restart-required semantics).
    configured_source: Mutex<HerdrBinarySource>,
    /// Source actively used by this process (frozen after first configure).
    active_source: Mutex<HerdrBinarySource>,
    /// Diagnostic retained when the persisted preference cannot be trusted.
    binary_source_config_error: Mutex<Option<String>>,
    /// Serializes atomic preference replacement and matching in-memory updates.
    binary_source_write_lock: Mutex<()>,
    /// Capability documents are expensive to discover because they spawn the
    /// selected CLI for status + schema. Fast paths cache them only while the
    /// named-session socket, server protocol, and selected binary fingerprint
    /// still match. Session list + `ping` remain authoritative on every call.
    capability_cache: Mutex<HashMap<String, CachedCapabilities>>,
    capability_probe_lock: Mutex<()>,
}

#[derive(Clone)]
struct CachedCapabilities {
    capabilities: HerdrCapabilities,
    named_session: String,
    socket_path: String,
    binary_fingerprint: Option<String>,
}

struct ConnectorSession {
    id: String,
    mode: HerdrTerminalMode,
    cols: Mutex<u16>,
    rows: Mutex<u16>,
    child: Mutex<Option<Child>>,
    process_tree: Mutex<Option<process_kill::ProcessTreeGuard>>,
    stdin: Mutex<Option<ChildStdin>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    closed: Mutex<bool>,
}

struct EventSubscription {
    closed: Arc<AtomicBool>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl Default for HerdrManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HerdrManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            event_subscriptions: Mutex::new(HashMap::new()),
            binary_override: Mutex::new(None),
            managed_binary_override: Mutex::new(None),
            socket_override: Mutex::new(None),
            config_dir: Mutex::new(None),
            resource_dir: Mutex::new(None),
            configured_source: Mutex::new(HerdrBinarySource::Global),
            active_source: Mutex::new(HerdrBinarySource::Global),
            binary_source_config_error: Mutex::new(None),
            binary_source_write_lock: Mutex::new(()),
            capability_cache: Mutex::new(HashMap::new()),
            capability_probe_lock: Mutex::new(()),
        }
    }

    /// Wire app-data / resource directories once during Tauri setup.
    pub fn configure_paths(&self, config_dir: PathBuf, resource_dir: Option<PathBuf>) {
        *self.config_dir.lock().unwrap() = Some(config_dir.clone());
        *self.resource_dir.lock().unwrap() = resource_dir;
        let loaded = load_binary_source_preference(&config_dir);
        *self.configured_source.lock().unwrap() = loaded.source;
        *self.active_source.lock().unwrap() = loaded.source;
        *self.binary_source_config_error.lock().unwrap() = loaded.error;
        self.capability_cache.lock().unwrap().clear();
    }

    /// Start the resolved local Herdr headless server before the frontend bootstraps.
    /// The server is intentionally detached and remains independent of Yuzora's
    /// connector-child cleanup on app exit.
    pub fn ensure_server_running_on_startup(&self) -> Result<bool, String> {
        let binary = self
            .resolve_binary()
            .ok_or_else(|| "herdr binary is unavailable for startup".to_string())?;
        if query_herdr_server_running(&binary)? {
            return Ok(false);
        }

        let mut command = Command::new(&binary);
        command
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        process_kill::configure_background_process(&mut command);
        let mut child = command.spawn().map_err(|error| {
            format!(
                "failed to launch Herdr server from {}: {error}",
                binary.display()
            )
        })?;
        let deadline = Instant::now() + HERDR_STARTUP_TIMEOUT;

        loop {
            let last_probe = match query_herdr_server_running(&binary) {
                Ok(true) => return Ok(true),
                Ok(false) => "server has not reported running".to_string(),
                Err(error) => error,
            };

            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("failed to inspect Herdr server process: {error}"))?
            {
                if query_herdr_server_running(&binary).unwrap_or(false) {
                    return Ok(true);
                }
                return Err(format!(
                    "Herdr server exited before becoming ready ({status}); {last_probe}"
                ));
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "Herdr server did not become ready within {}s; {last_probe}",
                    HERDR_STARTUP_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(HERDR_STARTUP_POLL_INTERVAL);
        }
    }

    #[cfg(test)]
    pub fn with_binary(binary: PathBuf) -> Self {
        let mgr = Self::new();
        *mgr.binary_override.lock().unwrap() = Some(binary);
        mgr
    }

    #[cfg(test)]
    pub fn set_socket_override(&self, socket: Option<PathBuf>) {
        *self.socket_override.lock().unwrap() = socket;
    }

    #[cfg(test)]
    pub fn set_managed_binary_override(&self, path: Option<PathBuf>) {
        *self.managed_binary_override.lock().unwrap() = path;
    }

    #[cfg(test)]
    pub fn set_config_dir_for_test(&self, dir: PathBuf) {
        *self.config_dir.lock().unwrap() = Some(dir.clone());
        let loaded = load_binary_source_preference(&dir);
        *self.configured_source.lock().unwrap() = loaded.source;
        *self.active_source.lock().unwrap() = loaded.source;
        *self.binary_source_config_error.lock().unwrap() = loaded.error;
        self.capability_cache.lock().unwrap().clear();
    }

    pub fn binary_source_info(&self) -> HerdrBinarySourceInfo {
        let configured = *self.configured_source.lock().unwrap();
        let active = *self.active_source.lock().unwrap();
        let (active_path, resolved, active_reason) = self.resolve_binary_selection(active);
        let (configured_path, _configured_resolved, configured_reason) = if configured == active {
            (active_path.clone(), resolved, active_reason.clone())
        } else {
            self.resolve_binary_selection(configured)
        };
        let (version, protocol) = active_path
            .as_deref()
            .map(probe_binary_identity)
            .unwrap_or((None, None));
        let (configured_version, configured_protocol) = if configured == active {
            (version.clone(), protocol)
        } else {
            configured_path
                .as_deref()
                .map(probe_binary_identity)
                .unwrap_or((None, None))
        };
        HerdrBinarySourceInfo {
            configured,
            active,
            resolved,
            available: active_path.is_some(),
            path: active_path.map(|p| p.to_string_lossy().into_owned()),
            reason: active_reason,
            version,
            protocol,
            configured_available: configured_path.is_some(),
            configured_path: configured_path.map(|p| p.to_string_lossy().into_owned()),
            configured_reason,
            configured_version,
            configured_protocol,
            configuration_error: self.binary_source_config_error.lock().unwrap().clone(),
            restart_required: configured != active,
        }
    }

    pub fn get_binary_source(&self) -> HerdrBinarySource {
        *self.configured_source.lock().unwrap()
    }

    /// Persist preference. Active process keeps its current binary until restart.
    pub fn set_binary_source(
        &self,
        source: HerdrBinarySource,
    ) -> Result<HerdrBinarySourceSetResult, String> {
        let config_dir = self
            .config_dir
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "herdr config directory is not configured".to_string())?;
        let _write_guard = self.binary_source_write_lock.lock().unwrap();
        save_binary_source_preference(&config_dir, source)?;
        *self.configured_source.lock().unwrap() = source;
        *self.binary_source_config_error.lock().unwrap() = None;
        let active = *self.active_source.lock().unwrap();
        Ok(HerdrBinarySourceSetResult {
            configured: source,
            restart_required: source != active,
        })
    }

    pub fn resolve_binary(&self) -> Option<PathBuf> {
        let active = *self.active_source.lock().unwrap();
        self.resolve_binary_selection(active).0
    }

    /// Resolve a user's source preference into the executable used by this
    /// process. Global is the automatic policy: prefer PATH, then use the
    /// bundled Yuzora-managed binary. Explicit managed selection stays strict.
    fn resolve_binary_selection(
        &self,
        source: HerdrBinarySource,
    ) -> (Option<PathBuf>, Option<HerdrBinarySource>, Option<String>) {
        let has_explicit_override = self.binary_override.lock().unwrap().is_some();
        let primary = self.resolve_binary_for_source(source);
        let managed =
            if source == HerdrBinarySource::Global && primary.0.is_none() && !has_explicit_override
            {
                self.resolve_binary_for_source(HerdrBinarySource::Default)
            } else {
                (None, None)
            };
        select_binary_resolution(source, has_explicit_override, primary, managed)
    }

    /// Strict lookup for one source. Automatic fallback belongs only in
    /// `resolve_binary_selection`, keeping explicit managed diagnostics honest.
    fn resolve_binary_for_source(
        &self,
        source: HerdrBinarySource,
    ) -> (Option<PathBuf>, Option<String>) {
        if let Some(path) = self.binary_override.lock().unwrap().clone() {
            if is_executable(&path) {
                return (Some(path), None);
            }
            return (
                None,
                Some(format!(
                    "herdr binary override is not executable: {}",
                    path.display()
                )),
            );
        }
        match source {
            HerdrBinarySource::Global => match which_binary("herdr").map(PathBuf::from) {
                Some(path) => (Some(path), None),
                None => (None, Some("Herdr was not found on PATH".into())),
            },
            HerdrBinarySource::Default => {
                if let Some(path) = self.managed_binary_override.lock().unwrap().clone() {
                    if is_executable(&path) {
                        return (Some(path), None);
                    }
                    return (
                        None,
                        Some("Yuzora-managed Herdr override is not an executable file".into()),
                    );
                }
                let Some(resource_dir) = self.resource_dir.lock().unwrap().clone() else {
                    return (
                        None,
                        Some("This build does not include a managed Herdr binary".into()),
                    );
                };
                let candidate = managed_binary_path(&resource_dir);
                if is_executable(&candidate) {
                    (Some(candidate), None)
                } else {
                    (
                        None,
                        Some(format!(
                            "Yuzora-managed Herdr binary is unavailable at {}",
                            candidate.display()
                        )),
                    )
                }
            }
        }
    }

    pub fn capabilities(&self) -> HerdrCapabilities {
        self.capabilities_for_session(None)
    }

    fn capability_cache_key(session_name: Option<&str>) -> String {
        session_name.unwrap_or("live").to_string()
    }

    fn active_binary_fingerprint(&self) -> Option<String> {
        let path = self.resolve_binary()?;
        let metadata = fs::metadata(&path).ok()?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        Some(format!("{}:{}:{modified}", path.display(), metadata.len()))
    }

    /// Fresh authoritative probe used by the explicit capabilities IPC. Probes
    /// are serialized so an older concurrent discovery cannot overwrite newer
    /// cache state. Cache publication also checks live server identity after
    /// status/schema discovery, preventing a restart between probes from
    /// publishing a mixed-epoch capability document.
    pub fn capabilities_for_session(&self, session_name: Option<&str>) -> HerdrCapabilities {
        let _probe_guard = self.capability_probe_lock.lock().unwrap();
        let mut caps = self.discover_capabilities_for_session(session_name);
        let cache_key = Self::capability_cache_key(session_name);
        let should_probe = caps.server.running
            && caps.server.socket_path.is_some()
            && caps.server.compatible != Some(false);
        let cache_entry = if should_probe {
            match self.require_running_session_socket(session_name) {
                Ok((session, socket_path)) => match ping_server_identity(&socket_path) {
                    Ok(live_identity)
                        if Some(live_identity.clone())
                            == caps.server.version.clone().zip(caps.server.protocol) =>
                    {
                        Some(CachedCapabilities {
                            capabilities: caps.clone(),
                            named_session: session.name,
                            socket_path,
                            binary_fingerprint: self.active_binary_fingerprint(),
                        })
                    }
                    Ok(_) => {
                        disable_live_socket_capabilities(
                            &mut caps,
                            "herdr server identity changed during capability discovery",
                        );
                        None
                    }
                    Err(error) => {
                        disable_live_socket_capabilities(
                            &mut caps,
                            &format!("herdr local socket probe failed: {error}"),
                        );
                        None
                    }
                },
                Err(error) => {
                    disable_live_socket_capabilities(
                        &mut caps,
                        &format!("herdr running session became unavailable: {error}"),
                    );
                    None
                }
            }
        } else {
            None
        };
        let mut cache = self.capability_cache.lock().unwrap();
        if let Some(entry) = cache_entry {
            cache.insert(cache_key, entry);
        } else {
            cache.remove(&cache_key);
        }
        caps
    }

    /// Fast path after bootstrap. Session list and socket `ping` are checked on
    /// every call. A restart, default-session change, protocol change, binary
    /// replacement, or negative cache condition falls back to fresh discovery.
    fn cached_capabilities_for_session(&self, session_name: Option<&str>) -> HerdrCapabilities {
        let current_session = self.require_running_session_socket(session_name).ok();
        let cache_key = Self::capability_cache_key(session_name);
        let cached = self
            .capability_cache
            .lock()
            .unwrap()
            .get(&cache_key)
            .cloned();
        if let (Some((session, socket_path)), Some(cached)) = (current_session, cached) {
            if cached.named_session == session.name
                && cached.socket_path == socket_path
                && cached.binary_fingerprint == self.active_binary_fingerprint()
                && ping_server_identity(&socket_path).ok()
                    == cached
                        .capabilities
                        .server
                        .version
                        .clone()
                        .zip(cached.capabilities.server.protocol)
            {
                return cached.capabilities;
            }
        }
        self.capabilities_for_session(session_name)
    }

    fn cached_capabilities_without_ping(
        &self,
        session_name: Option<&str>,
        named_session: &str,
        socket_path: &str,
    ) -> HerdrCapabilities {
        let cache_key = Self::capability_cache_key(session_name);
        if let Some(cached) = self
            .capability_cache
            .lock()
            .unwrap()
            .get(&cache_key)
            .cloned()
        {
            if cached.named_session == named_session
                && cached.socket_path == socket_path
                && cached.binary_fingerprint == self.active_binary_fingerprint()
            {
                return cached.capabilities;
            }
        }
        self.discover_capabilities_for_session(session_name)
    }

    fn discover_capabilities_for_session(&self, session_name: Option<&str>) -> HerdrCapabilities {
        let binary_source = self.binary_source_info();
        let binary_path = self.resolve_binary();
        let missing_reason = binary_source
            .reason
            .clone()
            .unwrap_or_else(|| "herdr binary not found".into());
        let mut caps = HerdrCapabilities {
            binary_path: binary_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            binary_version: None,
            binary_protocol: None,
            channel: None,
            binary_source: binary_source.clone(),
            server: HerdrServerCapability {
                running: false,
                version: None,
                protocol: None,
                compatible: None,
                socket_path: None,
                capabilities: None,
            },
            api: HerdrApiCapability {
                snapshot: false,
                ping: false,
                tab_create: false,
                workspace_focus: false,
                workspace_create: false,
                workspace_rename: false,
                workspace_close: false,
                tab_rename: false,
                tab_close: false,
                tab_focus: false,
                tab_move: false,
                pane_focus: false,
                pane_rename: false,
                pane_split: false,
                pane_zoom: false,
                pane_swap: false,
                pane_close: false,
                layout_export: false,
                layout_set_split_ratio: false,
                agent_manifests: false,
                agent_start: false,
                agent_get: false,
                agent_read: false,
                events_subscribe: false,
                worktree_list: false,
                methods: Vec::new(),
                schema_protocol: None,
                schema_version: None,
                reason: Some(missing_reason.clone()),
            },
            terminal: HerdrTerminalCapability {
                observe: false,
                control: false,
                takeover: false,
                input: false,
                resize: false,
                scroll: false,
                release: false,
                create: false,
                reason: Some(missing_reason),
            },
            events: HerdrEventsCapability {
                status: HerdrEventsStatus::Unavailable,
                reason: Some("herdr events.subscribe unavailable".into()),
            },
        };

        let Some(binary) = binary_path else {
            return caps;
        };

        // Named-session metadata is authoritative for socket path + running.
        // Never invent socket paths; never start a stopped session.
        let named = match self.resolve_named_session(session_name) {
            Ok(session) => Some(session),
            Err(err) => {
                // Do not fall back to the default session's status/schema for an
                // unknown named session; that would advertise capabilities for
                // the wrong runtime namespace.
                caps.api.reason = Some(err.clone());
                caps.terminal.reason = Some(err);
                return caps;
            }
        };
        let session_env = named.as_ref().map(|s| s.name.as_str());

        match run_herdr_json_with_session(&binary, &["status", "--json"], session_env) {
            Ok(status) => apply_status_json(&mut caps, &status),
            Err(err) => {
                caps.api.reason = Some(format!("herdr status failed: {err}"));
                caps.terminal.reason = Some(format!("herdr status failed: {err}"));
            }
        }

        // Prefer session-list socket; never keep a status-derived path when the
        // list entry disagrees or the session is stopped.
        if let Some(session) = named.as_ref() {
            if session.running && !session.socket_path.trim().is_empty() {
                caps.server.socket_path = Some(session.socket_path.clone());
                caps.server.running = true;
            } else {
                caps.server.running = false;
                caps.server.socket_path = None;
            }
        }

        if let Some(socket) = self.socket_override.lock().unwrap().clone() {
            caps.server.socket_path = Some(socket.to_string_lossy().into_owned());
            caps.server.running = true;
        }

        let mut schema_value: Option<serde_json::Value> = None;
        match run_herdr_json_with_session(&binary, &["api", "schema", "--json"], session_env) {
            Ok(schema) => {
                if let Some(protocol) = schema.get("protocol").and_then(|v| v.as_u64()) {
                    caps.api.schema_protocol = Some(protocol as u32);
                    if caps.binary_protocol.is_none() {
                        caps.binary_protocol = Some(protocol as u32);
                    }
                }
                if let Some(version) = schema.get("schema_version").and_then(|v| v.as_u64()) {
                    caps.api.schema_version = Some(version as u32);
                }
                schema_value = Some(schema);
            }
            Err(err) => {
                let msg = format!("herdr api schema failed: {err}");
                caps.api.reason = Some(match caps.api.reason.take() {
                    Some(prev) if !prev.is_empty() => format!("{prev}; {msg}"),
                    _ => msg,
                });
            }
        }

        let schema_methods = schema_value
            .as_ref()
            .map(collect_schema_methods)
            .unwrap_or_default();
        let has_snapshot_method = schema_methods.contains("session.snapshot");
        let has_tab_create_method = schema_methods.contains("tab.create");
        let has_ping_method =
            schema_methods.contains("session.ping") || schema_methods.contains("ping");

        let terminal_ok = caps.binary_path.is_some();
        let incompatible = caps.server.compatible == Some(false);
        let session_stopped = named.as_ref().is_some_and(|s| !s.running);
        let socket_present = caps.server.socket_path.is_some() && caps.server.running;
        // Never claim API features when the server is explicitly incompatible.
        let socket_ready = socket_present && !incompatible && !session_stopped;

        if terminal_ok {
            // Control/takeover/input require a compatible running session;
            // observe/release stay available only when that session is running.
            let control_ok = socket_ready;
            let create_ok = socket_ready && has_tab_create_method;
            let terminal_reason = if session_stopped {
                let name = named.as_ref().map(|s| s.name.as_str()).unwrap_or("session");
                Some(format!(
                    "herdr session '{name}' is not running; start it with `herdr session attach {name}`"
                ))
            } else if incompatible {
                Some("herdr server protocol incompatible".into())
            } else if !socket_present {
                Some("herdr server not running or socket unavailable".into())
            } else if !has_tab_create_method {
                Some("selected herdr schema lacks tab.create".into())
            } else {
                None
            };
            caps.terminal = HerdrTerminalCapability {
                observe: socket_ready,
                control: control_ok,
                takeover: control_ok,
                input: control_ok,
                resize: control_ok,
                scroll: control_ok,
                release: true,
                create: create_ok,
                reason: terminal_reason,
            };
        }

        if session_stopped {
            let name = named.as_ref().map(|s| s.name.as_str()).unwrap_or("session");
            clear_api_method_flags(&mut caps.api);
            caps.api.reason = Some(format!(
                "herdr session '{name}' is not running; start it with `herdr session attach {name}`"
            ));
        } else if incompatible {
            clear_api_method_flags(&mut caps.api);
            caps.api.reason = Some("herdr server protocol incompatible".into());
        } else if socket_ready {
            apply_schema_method_flags(&mut caps.api, &schema_methods, has_ping_method);
            if has_snapshot_method && has_tab_create_method {
                // Full API surface present — drop seed/schema probe noise.
                caps.api.reason = None;
            } else if !has_snapshot_method && !has_tab_create_method {
                caps.api.reason =
                    Some("selected herdr schema lacks session.snapshot/tab.create".into());
            } else if !has_snapshot_method {
                caps.api.reason = Some("selected herdr schema lacks session.snapshot".into());
            } else {
                caps.api.reason = Some("selected herdr schema lacks tab.create".into());
            }
        } else if !caps.api.snapshot && caps.api.reason.is_none() {
            caps.api.reason = Some("herdr server not running or socket unavailable".into());
        }

        // Event subscription is advertised when the long-lived local-socket lane
        // can open against a running compatible session. Transport is Unix
        // domain sockets or Windows named pipes, not host-OS gated.
        apply_events_capability(
            &mut caps.events,
            socket_ready,
            schema_methods.contains("events.subscribe"),
            session_stopped,
        );

        caps
    }

    /// `herdr session list --json` — authoritative named-session inventory.
    pub fn list_sessions(&self) -> Result<Vec<HerdrNamedSession>, String> {
        let binary = self
            .resolve_binary()
            .ok_or_else(|| "herdr binary not found on PATH".to_string())?;
        let value = run_herdr_json(&binary, &["session", "list", "--json"])?;
        bounded_ipc(parse_session_list_json(&value)?)
    }

    /// Resolve a named session from `session list --json` only.
    /// `None` / empty / `"live"` maps to the default session entry.
    pub fn resolve_named_session(
        &self,
        session_name: Option<&str>,
    ) -> Result<HerdrNamedSession, String> {
        let sessions = self.list_sessions()?;
        if sessions.is_empty() {
            return Err("no herdr named sessions found".into());
        }
        let requested = session_name.map(str::trim).filter(|s| !s.is_empty());
        match requested {
            Some(name) if name != "live" => sessions
                .into_iter()
                .find(|s| s.name == name)
                .ok_or_else(|| format!("herdr session not found: {name}")),
            _ => {
                if let Some(default_session) = sessions.iter().find(|s| s.default).cloned() {
                    Ok(default_session)
                } else {
                    sessions
                        .into_iter()
                        .next()
                        .ok_or_else(|| "no herdr named sessions found".to_string())
                }
            }
        }
    }

    /// Running-session socket path from session list — never guessed.
    fn require_running_session_socket(
        &self,
        session_name: Option<&str>,
    ) -> Result<(HerdrNamedSession, String), String> {
        let session = self.resolve_named_session(session_name)?;
        if !session.running {
            return Err(format!(
                "herdr session '{}' is not running; start it with `herdr session attach {}`",
                session.name, session.name
            ));
        }
        let socket_from_list = session.socket_path.trim().to_string();
        if socket_from_list.is_empty() {
            return Err(format!(
                "herdr session '{}' has no socket_path from session list",
                session.name
            ));
        }
        // Test override may redirect the socket without inventing a production path.
        if let Some(over) = self.socket_override.lock().unwrap().clone() {
            return Ok((session, over.to_string_lossy().into_owned()));
        }
        Ok((session, socket_from_list))
    }

    pub fn snapshot(&self, session_name: Option<&str>) -> Result<HerdrSnapshotResult, String> {
        let caps = self.cached_capabilities_for_session(session_name);
        if !caps.api.snapshot {
            return Err(caps
                .api
                .reason
                .unwrap_or_else(|| "herdr snapshot unavailable".into()));
        }
        let (_session, socket) = self.require_running_session_socket(session_name)?;
        let response = api_request(&socket, "session.snapshot", serde_json::json!({}))?;
        bounded_ipc(parse_snapshot_response(response)?)
    }

    /// Create a new tab (and root pane/terminal) via public `tab.create`.
    /// Convenience wrapper used by the existing terminal-create IPC surface.
    pub fn create_terminal(
        &self,
        session_name: Option<&str>,
        workspace_id: Option<String>,
        title: Option<String>,
    ) -> Result<HerdrTerminalCreateResult, String> {
        self.tab_create(session_name, workspace_id, title, None, true)
    }

    /// Server-advertised Agent kinds, enriched with advisory PATH detection.
    /// PATH misses do not hide or disable a manifest: the selected Herdr server
    /// remains authoritative and validates the actual launch on `agent.start`.
    pub fn agent_catalog(
        &self,
        session_name: Option<&str>,
    ) -> Result<Vec<HerdrAgentCatalogEntry>, String> {
        let response = self.call_checked_api(
            session_name,
            |api| api.agent_manifests,
            "server.agent_manifests",
            serde_json::json!({}),
            "herdr server.agent_manifests unavailable",
        )?;
        bounded_ipc(parse_agent_manifest_response(response)?)
    }

    /// Transactional New Agent flow adapted from herdrm: create a fresh tab,
    /// start one validated manifest kind, retry only fresh-shell/name races,
    /// and close only the tab created by this failed transaction.
    pub fn agent_create(
        &self,
        session_name: Option<&str>,
        workspace_id: String,
        kind: String,
        bypass_permissions: bool,
    ) -> Result<HerdrAgentCreateResult, String> {
        if workspace_id.trim().is_empty() {
            return Err("workspace_id is required".into());
        }
        let caps = self.cached_capabilities_for_session(session_name);
        if !caps.api.tab_close {
            return Err(caps.api.reason.unwrap_or_else(|| {
                "herdr tab.close is required for failed New Agent rollback".into()
            }));
        }
        let kind = validate_agent_kind(&kind)?.to_string();
        let catalog = self.agent_catalog(session_name)?;
        if !catalog.iter().any(|entry| entry.agent == kind) {
            return Err(format!("herdr agent manifest not found: {kind}"));
        }

        let created = self.tab_create(
            session_name,
            Some(workspace_id),
            Some(kind.clone()),
            None,
            true,
        )?;
        let args = if bypass_permissions {
            agent_bypass_flags(&kind)
                .iter()
                .map(|flag| (*flag).to_string())
                .collect()
        } else {
            Vec::new()
        };

        match self.start_agent_in_fresh_pane(session_name, &created, &kind, &args) {
            Ok(name) => Ok(HerdrAgentCreateResult {
                name,
                kind,
                terminal_id: created.terminal_id,
                pane_id: created.pane_id,
                tab_id: created.tab_id,
                workspace_id: created.workspace_id,
                title: created.title,
            }),
            Err(error) => {
                let cleanup = self.tab_close(session_name, created.tab_id);
                Err(match cleanup {
                    Ok(()) => error,
                    Err(cleanup_error) => {
                        format!("{error}; failed to close the newly-created tab: {cleanup_error}")
                    }
                })
            }
        }
    }

    fn start_agent_in_fresh_pane(
        &self,
        session_name: Option<&str>,
        created: &HerdrTerminalCreateResult,
        kind: &str,
        args: &[String],
    ) -> Result<String, String> {
        let deadline = Instant::now() + AGENT_START_RETRY_TIMEOUT;
        let mut name = kind.to_string();
        let mut retried_name = false;
        loop {
            let result = self.call_checked_api(
                session_name,
                |api| api.agent_start,
                "agent.start",
                serde_json::json!({
                    "name": name,
                    "kind": kind,
                    "pane_id": created.pane_id,
                    "args": args,
                }),
                "herdr agent.start unavailable",
            );
            match result {
                Ok(response) => {
                    parse_agent_started_response(response, &created.pane_id)?;
                    return Ok(name);
                }
                Err(error) if error.starts_with("agent_pane_busy:") => {
                    if Instant::now() >= deadline
                        || !self.created_pane_shell_is_initializing(session_name, created)
                        || Instant::now() >= deadline
                    {
                        return Err(error);
                    }
                    std::thread::sleep(AGENT_START_RETRY_INTERVAL);
                }
                Err(error) if error.starts_with("agent_name_taken:") && !retried_name => {
                    retried_name = true;
                    let suffix = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed) & 0xffff;
                    name = format!("{kind}-{suffix:04x}");
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn created_pane_shell_is_initializing(
        &self,
        session_name: Option<&str>,
        created: &HerdrTerminalCreateResult,
    ) -> bool {
        let pane = self.call_checked_api(
            session_name,
            |api| api.methods.iter().any(|method| method == "pane.get"),
            "pane.get",
            serde_json::json!({ "pane_id": created.pane_id }),
            "herdr pane.get unavailable",
        );
        let Ok(pane) = pane else {
            return false;
        };
        if !pane_get_matches_created_terminal(&pane, created) {
            return false;
        }

        let process_info = self.call_checked_api(
            session_name,
            |api| {
                api.methods
                    .iter()
                    .any(|method| method == "pane.process_info")
            },
            "pane.process_info",
            serde_json::json!({ "pane_id": created.pane_id }),
            "herdr pane.process_info unavailable",
        );
        process_info.ok().is_some_and(|response| {
            pane_process_info_shows_shell_initialization(&response, &created.pane_id)
        })
    }

    /// Public `tab.create` with full protocol-19 params.
    pub fn tab_create(
        &self,
        session_name: Option<&str>,
        workspace_id: Option<String>,
        label: Option<String>,
        cwd: Option<String>,
        focus: bool,
    ) -> Result<HerdrTerminalCreateResult, String> {
        let response = self.call_checked_api(
            session_name,
            |api| api.tab_create,
            "tab.create",
            build_tab_create_params(workspace_id, label, cwd, focus),
            "herdr tab.create unavailable",
        )?;
        bounded_ipc(parse_tab_created_response(response)?)
    }

    /// Focus a Herdr Space via public `workspace.focus`.
    pub fn workspace_focus(
        &self,
        session_name: Option<&str>,
        workspace_id: String,
    ) -> Result<(), String> {
        if workspace_id.trim().is_empty() {
            return Err("workspace_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.workspace_focus,
            "workspace.focus",
            serde_json::json!({ "workspace_id": workspace_id }),
            "herdr workspace.focus unavailable",
        )?;
        Ok(())
    }

    /// Create (+ optional focus) a Herdr Space via public `workspace.create`.
    pub fn workspace_create(
        &self,
        session_name: Option<&str>,
        cwd: Option<String>,
        label: Option<String>,
        focus: bool,
    ) -> Result<HerdrWorkspaceCreateResult, String> {
        let mut params = serde_json::Map::new();
        if let Some(cwd) = cwd.filter(|s| !s.trim().is_empty()) {
            params.insert("cwd".into(), serde_json::Value::String(cwd));
        }
        if let Some(label) = label.filter(|s| !s.trim().is_empty()) {
            params.insert("label".into(), serde_json::Value::String(label));
        }
        params.insert("focus".into(), serde_json::Value::Bool(focus));
        let response = self.call_checked_api(
            session_name,
            |api| api.workspace_create,
            "workspace.create",
            serde_json::Value::Object(params),
            "herdr workspace.create unavailable",
        )?;
        bounded_ipc(parse_workspace_created_response(response)?)
    }

    /// Public `workspace.rename { workspace_id, label }`.
    pub fn workspace_rename(
        &self,
        session_name: Option<&str>,
        workspace_id: String,
        label: String,
    ) -> Result<(), String> {
        if workspace_id.trim().is_empty() {
            return Err("workspace_id is required".into());
        }
        if label.trim().is_empty() {
            return Err("label is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.workspace_rename,
            "workspace.rename",
            serde_json::json!({ "workspace_id": workspace_id, "label": label }),
            "herdr workspace.rename unavailable",
        )?;
        Ok(())
    }

    /// Public `workspace.close { workspace_id }` (destructive; confirm in UI).
    /// Read-only protocol-19 `worktree.list` against the selected running session.
    pub fn worktree_list(
        &self,
        session_name: Option<&str>,
        cwd: Option<String>,
        workspace_id: Option<String>,
    ) -> Result<HerdrWorktreeListResult, String> {
        let mut params = serde_json::Map::new();
        if let Some(cwd) = cwd.filter(|s| !s.trim().is_empty()) {
            params.insert("cwd".into(), serde_json::Value::String(cwd));
        }
        if let Some(workspace_id) = workspace_id.filter(|s| !s.trim().is_empty()) {
            params.insert(
                "workspace_id".into(),
                serde_json::Value::String(workspace_id),
            );
        }
        let response = self.call_checked_api(
            session_name,
            |api| api.worktree_list,
            "worktree.list",
            serde_json::Value::Object(params),
            "herdr worktree.list unavailable",
        )?;
        bounded_ipc(parse_worktree_list_response(response)?)
    }

    pub fn workspace_close(
        &self,
        session_name: Option<&str>,
        workspace_id: String,
    ) -> Result<(), String> {
        if workspace_id.trim().is_empty() {
            return Err("workspace_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.workspace_close,
            "workspace.close",
            serde_json::json!({ "workspace_id": workspace_id }),
            "herdr workspace.close unavailable",
        )?;
        Ok(())
    }

    pub fn tab_focus(&self, session_name: Option<&str>, tab_id: String) -> Result<(), String> {
        if tab_id.trim().is_empty() {
            return Err("tab_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.tab_focus,
            "tab.focus",
            serde_json::json!({ "tab_id": tab_id }),
            "herdr tab.focus unavailable",
        )?;
        Ok(())
    }

    pub fn tab_rename(
        &self,
        session_name: Option<&str>,
        tab_id: String,
        label: String,
    ) -> Result<(), String> {
        if tab_id.trim().is_empty() {
            return Err("tab_id is required".into());
        }
        if label.trim().is_empty() {
            return Err("label is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.tab_rename,
            "tab.rename",
            serde_json::json!({ "tab_id": tab_id, "label": label }),
            "herdr tab.rename unavailable",
        )?;
        Ok(())
    }

    /// Public `tab.close` (destructive; confirm in UI).
    pub fn tab_close(&self, session_name: Option<&str>, tab_id: String) -> Result<(), String> {
        if tab_id.trim().is_empty() {
            return Err("tab_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.tab_close,
            "tab.close",
            serde_json::json!({ "tab_id": tab_id }),
            "herdr tab.close unavailable",
        )?;
        Ok(())
    }

    /// Public `tab.move { tab_id, insert_index }` within the owning Space.
    pub fn tab_move(
        &self,
        session_name: Option<&str>,
        tab_id: String,
        insert_index: u32,
    ) -> Result<(), String> {
        if tab_id.trim().is_empty() {
            return Err("tab_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.tab_move,
            "tab.move",
            build_tab_move_params(tab_id, insert_index),
            "herdr tab.move unavailable",
        )?;
        Ok(())
    }

    pub fn pane_focus(&self, session_name: Option<&str>, pane_id: String) -> Result<(), String> {
        if pane_id.trim().is_empty() {
            return Err("pane_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.pane_focus,
            "pane.focus",
            serde_json::json!({ "pane_id": pane_id }),
            "herdr pane.focus unavailable",
        )?;
        Ok(())
    }

    /// `pane.rename` — `label = None` clears the pane name (wire null).
    pub fn pane_rename(
        &self,
        session_name: Option<&str>,
        pane_id: String,
        label: Option<String>,
    ) -> Result<(), String> {
        if pane_id.trim().is_empty() {
            return Err("pane_id is required".into());
        }
        let mut params = serde_json::Map::new();
        params.insert("pane_id".into(), serde_json::Value::String(pane_id));
        params.insert(
            "label".into(),
            label
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        let _ = self.call_checked_api(
            session_name,
            |api| api.pane_rename,
            "pane.rename",
            serde_json::Value::Object(params),
            "herdr pane.rename unavailable",
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn pane_split(
        &self,
        session_name: Option<&str>,
        direction: HerdrSplitDirection,
        target_pane_id: Option<String>,
        workspace_id: Option<String>,
        cwd: Option<String>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<HerdrPaneIdentity, String> {
        let response = self.call_checked_api(
            session_name,
            |api| api.pane_split,
            "pane.split",
            build_pane_split_params(direction, target_pane_id, workspace_id, cwd, ratio, focus),
            "herdr pane.split unavailable",
        )?;
        bounded_ipc(parse_pane_info_response(response)?)
    }

    pub fn pane_zoom(
        &self,
        session_name: Option<&str>,
        pane_id: Option<String>,
        mode: Option<HerdrPaneZoomMode>,
    ) -> Result<(), String> {
        let mut params = serde_json::Map::new();
        if let Some(pane_id) = pane_id.filter(|s| !s.trim().is_empty()) {
            params.insert("pane_id".into(), serde_json::Value::String(pane_id));
        }
        let mode = mode.unwrap_or(HerdrPaneZoomMode::Toggle);
        params.insert(
            "mode".into(),
            serde_json::to_value(mode).map_err(|e| e.to_string())?,
        );
        let _ = self.call_checked_api(
            session_name,
            |api| api.pane_zoom,
            "pane.zoom",
            serde_json::Value::Object(params),
            "herdr pane.zoom unavailable",
        )?;
        Ok(())
    }

    pub fn pane_swap(
        &self,
        session_name: Option<&str>,
        source_pane_id: Option<String>,
        target_pane_id: Option<String>,
        pane_id: Option<String>,
        direction: Option<String>,
    ) -> Result<(), String> {
        let mut params = serde_json::Map::new();
        if let Some(id) = source_pane_id.filter(|s| !s.trim().is_empty()) {
            params.insert("source_pane_id".into(), serde_json::Value::String(id));
        }
        if let Some(id) = target_pane_id.filter(|s| !s.trim().is_empty()) {
            params.insert("target_pane_id".into(), serde_json::Value::String(id));
        }
        if let Some(id) = pane_id.filter(|s| !s.trim().is_empty()) {
            params.insert("pane_id".into(), serde_json::Value::String(id));
        }
        if let Some(direction) = direction.filter(|s| !s.trim().is_empty()) {
            params.insert("direction".into(), serde_json::Value::String(direction));
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.pane_swap,
            "pane.swap",
            serde_json::Value::Object(params),
            "herdr pane.swap unavailable",
        )?;
        Ok(())
    }

    /// Public `pane.close` (destructive; confirm in UI).
    pub fn pane_close(&self, session_name: Option<&str>, pane_id: String) -> Result<(), String> {
        if pane_id.trim().is_empty() {
            return Err("pane_id is required".into());
        }
        let _ = self.call_checked_api(
            session_name,
            |api| api.pane_close,
            "pane.close",
            serde_json::json!({ "pane_id": pane_id }),
            "herdr pane.close unavailable",
        )?;
        Ok(())
    }

    /// Public `layout.export { tab_id? | pane_id? }`.
    pub fn layout_export(
        &self,
        session_name: Option<&str>,
        tab_id: Option<String>,
        pane_id: Option<String>,
    ) -> Result<HerdrLayoutDescription, String> {
        let response = self.call_checked_api(
            session_name,
            |api| api.layout_export,
            "layout.export",
            build_layout_export_params(tab_id, pane_id),
            "herdr layout.export unavailable",
        )?;
        bounded_ipc(parse_layout_export_response(response)?)
    }

    /// Public `layout.set_split_ratio` — path booleans: false=first, true=second.
    pub fn layout_set_split_ratio(
        &self,
        session_name: Option<&str>,
        tab_id: Option<String>,
        pane_id: Option<String>,
        path: Vec<bool>,
        ratio: f64,
    ) -> Result<HerdrLayoutDescription, String> {
        if !(0.0..=1.0).contains(&ratio) {
            return Err("ratio must be between 0 and 1".into());
        }
        let response = self.call_checked_api(
            session_name,
            |api| api.layout_set_split_ratio,
            "layout.set_split_ratio",
            build_layout_set_split_ratio_params(tab_id, pane_id, &path, ratio),
            "herdr layout.set_split_ratio unavailable",
        )?;
        bounded_ipc(parse_layout_set_split_ratio_response(response)?)
    }

    /// Read-only `agent.get` against an explicit pane id.
    pub fn agent_get(
        &self,
        session_name: Option<&str>,
        target: String,
    ) -> Result<HerdrAgentDetails, String> {
        validate_explicit_pane_target(&target)?;
        let response = self.call_checked_api(
            session_name,
            |api| api.agent_get,
            "agent.get",
            serde_json::json!({ "target": target.clone() }),
            "herdr agent.get unavailable",
        )?;
        let parsed = parse_agent_get_response(response)?;
        if parsed.pane_id != target {
            return Err(format!(
                "agent.get returned pane {} for requested target {target}",
                parsed.pane_id
            ));
        }
        bounded_ipc(parsed)
    }

    /// Read-only `agent.read` against an explicit pane id.
    pub fn agent_read(
        &self,
        session_name: Option<&str>,
        target: String,
        source: HerdrReadSource,
        format: Option<HerdrReadFormat>,
        lines: Option<u32>,
        strip_ansi: Option<bool>,
    ) -> Result<HerdrAgentReadResult, String> {
        validate_explicit_pane_target(&target)?;
        if let Some(lines) = lines {
            if !(AGENT_READ_MIN_LINES..=AGENT_READ_MAX_LINES).contains(&lines) {
                return Err(format!(
                    "agent.read lines must be between {AGENT_READ_MIN_LINES} and {AGENT_READ_MAX_LINES}"
                ));
            }
        }
        let format = format.unwrap_or(HerdrReadFormat::Text);
        let mut params = serde_json::Map::new();
        params.insert("target".into(), serde_json::Value::String(target.clone()));
        params.insert(
            "source".into(),
            serde_json::to_value(source).map_err(|e| e.to_string())?,
        );
        params.insert(
            "format".into(),
            serde_json::to_value(format).map_err(|e| e.to_string())?,
        );
        if let Some(lines) = lines {
            params.insert("lines".into(), serde_json::json!(lines));
        }
        if let Some(strip_ansi) = strip_ansi {
            params.insert("strip_ansi".into(), serde_json::json!(strip_ansi));
        }
        let response = self.call_checked_api(
            session_name,
            |api| api.agent_read,
            "agent.read",
            serde_json::Value::Object(params),
            "herdr agent.read unavailable",
        )?;
        let parsed = parse_agent_read_response(response)?;
        if parsed.pane_id != target {
            return Err(format!(
                "agent.read returned pane {} for requested target {target}",
                parsed.pane_id
            ));
        }
        bounded_ipc(parsed)
    }

    /// Long-lived `events.subscribe` for Agent status and pane lifecycle.
    pub fn events_subscribe(
        self: &Arc<Self>,
        session_name: Option<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String> {
        let (session, socket) = self.require_running_session_socket(session_name.as_deref())?;
        let caps =
            self.cached_capabilities_without_ping(session_name.as_deref(), &session.name, &socket);
        if !caps.api.events_subscribe || caps.events.status != HerdrEventsStatus::Available {
            return Err(caps
                .events
                .reason
                .or(caps.api.reason)
                .unwrap_or_else(|| "herdr events.subscribe unavailable".into()));
        }
        let write_deadline = Instant::now() + LOCAL_IO_TIMEOUT;
        let mut stream = connect_local_stream(&socket, write_deadline)
            .map_err(|e| format!("connect {socket} failed: {e}"))?;

        let request_id = format!(
            "yuzora:herdr:sub:{}",
            NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
        );
        let req = serde_json::json!({
            "id": request_id,
            "method": "events.subscribe",
            "params": {
                "subscriptions": [
                    { "type": "pane.agent_status_changed" },
                    { "type": "pane.exited" },
                    { "type": "worktree.created" },
                    { "type": "worktree.opened" },
                    { "type": "worktree.removed" },
                    { "type": "tab.created" },
                    { "type": "tab.closed" },
                    { "type": "tab.moved" },
                    { "type": "workspace.created" },
                    { "type": "workspace.closed" },
                    { "type": "workspace.moved" },
                    { "type": "workspace.reordered" }
                ]
            }
        });
        let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
        line.push('\n');
        write_local_all_until(&mut stream, line.as_bytes(), write_deadline)
            .map_err(|e| format!("write events.subscribe failed: {e}"))?;

        let mut pending = Vec::new();
        let response = match read_local_ndjson_line(
            &mut stream,
            &mut pending,
            Some(Instant::now() + EVENT_ACK_TIMEOUT),
            MAX_NDJSON_LINE_BYTES,
        ) {
            Ok(None) => return Err(HerdrProtocolError::EmptyResponse.into()),
            Ok(Some(response)) => response,
            Err(error) => {
                return Err(format!("events.subscribe ack read failed: {error}"));
            }
        };
        if response.trim().is_empty() {
            return Err(HerdrProtocolError::EmptyResponse.into());
        }
        let value: serde_json::Value = serde_json::from_str(response.trim())
            .map_err(|e| format!("invalid events.subscribe ack json: {e}"))?;
        if let Err(error) = validate_json_complexity(&value) {
            return Err(error.into());
        }
        if let Some(err) = value.get("error") {
            let code = err.get("code").and_then(|v| v.as_str()).unwrap_or("error");
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("{code}: {message}"));
        }
        let result_type = value
            .get("result")
            .and_then(|r| r.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if result_type != "subscription_started" {
            return Err(format!(
                "events.subscribe expected subscription_started, got {result_type}"
            ));
        }

        let subscription_id = format!(
            "herdr-sub-{}",
            NEXT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed)
        );
        let closed = Arc::new(AtomicBool::new(false));
        let closed_for_thread = Arc::clone(&closed);
        let subscription_id_for_thread = subscription_id.clone();
        let on_event_for_thread = Arc::clone(&on_event);
        let (start_tx, start_rx) = std::sync::mpsc::sync_channel::<()>(1);

        // Register ownership before the reader can emit events, so a fast
        // terminal error/release can always find and close this stream.
        let handle = std::thread::spawn(move || {
            if start_rx.recv().is_err() {
                closed_for_thread.store(true, Ordering::SeqCst);
                return;
            }
            let mut stream = stream;
            let mut pending = pending;
            loop {
                if closed_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                match read_local_ndjson_line(
                    &mut stream,
                    &mut pending,
                    Some(Instant::now() + EVENT_POLL_INTERVAL),
                    MAX_NDJSON_LINE_BYTES,
                ) {
                    Ok(None) => {
                        let _ = emit_subscription_event(
                            &on_event_for_thread,
                            HerdrSubscriptionEvent::Disconnected {
                                subscription_id: subscription_id_for_thread.clone(),
                                reason: Some("socket closed".into()),
                            },
                        );
                        break;
                    }
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match parse_subscription_event_line(&subscription_id_for_thread, trimmed) {
                            Ok(Some(event)) => {
                                let terminal =
                                    matches!(event, HerdrSubscriptionEvent::Error { .. });
                                if emit_subscription_event(&on_event_for_thread, event).is_err()
                                    || terminal
                                {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(message) => {
                                let _ = emit_subscription_event(
                                    &on_event_for_thread,
                                    HerdrSubscriptionEvent::Error {
                                        subscription_id: subscription_id_for_thread.clone(),
                                        message,
                                    },
                                );
                                break;
                            }
                        }
                    }
                    Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::TimedOut)) => {
                        continue;
                    }
                    Err(BoundedNdjsonReadError::Io(err))
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        continue;
                    }
                    Err(err) => {
                        if closed_for_thread.load(Ordering::SeqCst) {
                            break;
                        }
                        let _ = emit_subscription_event(
                            &on_event_for_thread,
                            HerdrSubscriptionEvent::Error {
                                subscription_id: subscription_id_for_thread.clone(),
                                message: format!("events.subscribe read failed: {err}"),
                            },
                        );
                        break;
                    }
                }
            }
            closed_for_thread.store(true, Ordering::SeqCst);
        });

        let subscription = Arc::new(EventSubscription {
            closed,
            reader: Mutex::new(Some(handle)),
        });
        self.event_subscriptions
            .lock()
            .unwrap()
            .insert(subscription_id.clone(), Arc::clone(&subscription));
        if let Err(error) = emit_subscription_event(
            &on_event,
            HerdrSubscriptionEvent::Subscribed {
                subscription_id: subscription_id.clone(),
            },
        ) {
            drop(start_tx);
            self.event_subscriptions
                .lock()
                .unwrap()
                .remove(&subscription_id);
            release_event_subscription(&subscription);
            return Err(error);
        }
        if start_tx.send(()).is_err() {
            self.event_subscriptions
                .lock()
                .unwrap()
                .remove(&subscription_id);
            release_event_subscription(&subscription);
            return Err("events.subscribe reader failed to start".into());
        }
        Ok(subscription_id)
    }

    pub fn events_release(&self, subscription_id: &str) -> Result<(), String> {
        let subscription = self
            .event_subscriptions
            .lock()
            .unwrap()
            .remove(subscription_id);
        if let Some(subscription) = subscription {
            release_event_subscription(&subscription);
        }
        Ok(())
    }

    pub fn release_all_event_subscriptions(&self) {
        let subscriptions: Vec<Arc<EventSubscription>> = {
            let mut map = self.event_subscriptions.lock().unwrap();
            map.drain().map(|(_, s)| s).collect()
        };
        for subscription in subscriptions {
            release_event_subscription(&subscription);
        }
    }

    /// Schema-gated API request against the selected running session's socket.
    fn call_checked_api(
        &self,
        session_name: Option<&str>,
        is_available: impl Fn(&HerdrApiCapability) -> bool,
        method: &str,
        params: serde_json::Value,
        unavailable: &str,
    ) -> Result<serde_json::Value, String> {
        let caps = self.cached_capabilities_for_session(session_name);
        if !is_available(&caps.api) {
            return Err(caps.api.reason.unwrap_or_else(|| unavailable.into()));
        }
        // Incompatible / stopped sessions clear method flags above; still refuse
        // via the authoritative session-list socket path (never guess/start).
        let (_session, socket) = self.require_running_session_socket(session_name)?;
        api_request(&socket, method, params)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_terminal(
        self: &Arc<Self>,
        target: String,
        mode: HerdrTerminalMode,
        takeover: bool,
        cols: u16,
        rows: u16,
        session_name: Option<String>,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String> {
        if cols == 0 || rows == 0 {
            return Err("cols and rows must be greater than 0".into());
        }
        if target.trim().is_empty() {
            return Err("target is required".into());
        }
        if takeover && !matches!(mode, HerdrTerminalMode::Control) {
            return Err("takeover requires control mode".into());
        }

        // Refuse connectors against stopped sessions; never secretly attach/start.
        let named = self.resolve_named_session(session_name.as_deref())?;
        if !named.running {
            return Err(format!(
                "herdr session '{}' is not running; start it with `herdr session attach {}`",
                named.name, named.name
            ));
        }

        let binary = self
            .resolve_binary()
            .ok_or_else(|| "herdr binary not found on PATH".to_string())?;

        let role = match mode {
            HerdrTerminalMode::Observe => HerdrTerminalRole::Observer,
            HerdrTerminalMode::Control => HerdrTerminalRole::Controller,
        };

        let session_id = format!(
            "herdr-term-{}",
            NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut args = vec![
            "terminal".to_string(),
            "session".to_string(),
            match mode {
                HerdrTerminalMode::Observe => "observe".to_string(),
                HerdrTerminalMode::Control => "control".to_string(),
            },
            target.clone(),
            "--cols".to_string(),
            cols.to_string(),
            "--rows".to_string(),
            rows.to_string(),
        ];
        if takeover {
            args.push("--takeover".to_string());
        }

        let mut cmd = Command::new(&binary);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Official connector child: bind to the named session via HERDR_SESSION.
        // Default session remains valid. Never guess socket paths here.
        cmd.env("HERDR_SESSION", &named.name);
        // Connector only — never a process group that could sweep Herdr panes.
        process_kill::configure_background_process(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn herdr terminal connector: {e}"))?;
        let mut process_tree = process_kill::attach_process_tree(&mut child)
            .map_err(|e| format!("failed to contain herdr terminal connector: {e}"))?;
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let cleanup = process_kill::terminate_process_tree(&mut child, &mut process_tree);
                return Err(match cleanup {
                    Ok(()) => "herdr connector missing stdout".into(),
                    Err(error) => {
                        format!("herdr connector missing stdout; cleanup failed: {error}")
                    }
                });
            }
        };
        let stderr = child.stderr.take();
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let cleanup = process_kill::terminate_process_tree(&mut child, &mut process_tree);
                return Err(match cleanup {
                    Ok(()) => "herdr connector missing stdin".into(),
                    Err(error) => format!("herdr connector missing stdin; cleanup failed: {error}"),
                });
            }
        };

        let session = Arc::new(ConnectorSession {
            id: session_id.clone(),
            mode,
            cols: Mutex::new(cols),
            rows: Mutex::new(rows),
            child: Mutex::new(Some(child)),
            process_tree: Mutex::new(Some(process_tree)),
            stdin: Mutex::new(Some(stdin)),
            reader: Mutex::new(None),
            closed: Mutex::new(false),
        });

        let reader_session = session.clone();
        let reader_on_event = on_event.clone();
        let handle = match std::thread::Builder::new()
            .name(format!("herdr-term-{}", session_id))
            .spawn(move || {
                connector_reader_loop(reader_session, stdout, stderr, reader_on_event);
            }) {
            Ok(handle) => handle,
            Err(error) => {
                let cleanup = terminate_connector_process(&session);
                return Err(match cleanup {
                    Ok(()) => format!("failed to spawn connector reader: {error}"),
                    Err(cleanup_error) => format!(
                        "failed to spawn connector reader: {error}; cleanup failed: {cleanup_error}"
                    ),
                });
            }
        };
        *session.reader.lock().unwrap() = Some(handle);

        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), session);

        Ok(HerdrTerminalOpenResult {
            session_id,
            target,
            mode,
            role,
            cols,
            rows,
            takeover,
        })
    }

    pub fn terminal_input(
        &self,
        session_id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        let cmd = TerminalControlCommand::input(text, bytes_base64)?;
        self.send_control(session_id, &cmd)
    }

    pub fn terminal_resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let cmd = TerminalControlCommand::resize(cols, rows)?;
        self.send_control(session_id, &cmd)?;
        if let Some(session) = self.sessions.lock().unwrap().get(session_id) {
            *session.cols.lock().unwrap() = cols;
            *session.rows.lock().unwrap() = rows;
        }
        Ok(())
    }

    pub fn terminal_scroll(
        &self,
        session_id: &str,
        direction: HerdrScrollDirection,
        lines: u32,
    ) -> Result<(), String> {
        let cmd = TerminalControlCommand::scroll(direction, lines)?;
        self.send_control(session_id, &cmd)
    }

    /// Release only the Yuzora-owned connector child for this session.
    pub fn terminal_release(&self, session_id: &str) -> Result<(), String> {
        let session = {
            let mut map = self.sessions.lock().unwrap();
            map.remove(session_id)
                .ok_or_else(|| format!("no herdr terminal session {session_id}"))?
        };
        release_connector(&session)
    }

    /// Shutdown path: drop every Yuzora connector child. Never stops Herdr server/panes.
    pub fn release_all_connectors(&self) {
        let sessions: Vec<Arc<ConnectorSession>> = {
            let mut map = self.sessions.lock().unwrap();
            map.drain().map(|(_, s)| s).collect()
        };
        for session in sessions {
            let _ = release_connector(&session);
        }
        self.release_all_event_subscriptions();
    }

    fn send_control(&self, session_id: &str, cmd: &TerminalControlCommand) -> Result<(), String> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("no herdr terminal session {session_id}"))?;
        if !matches!(session.mode, HerdrTerminalMode::Control) {
            return Err("terminal control commands require control mode".into());
        }
        if *session.closed.lock().unwrap() {
            return Err(format!("herdr terminal session {session_id} is closed"));
        }
        let line = cmd.to_json_line()?;
        let mut stdin_guard = session.stdin.lock().unwrap();
        let stdin = stdin_guard
            .as_mut()
            .ok_or_else(|| format!("herdr terminal session {session_id} has no stdin"))?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| format!("failed to write control command: {e}"))?;
        stdin
            .flush()
            .map_err(|e| format!("failed to flush control command: {e}"))?;
        Ok(())
    }
}

fn release_connector(session: &ConnectorSession) -> Result<(), String> {
    let first_release = {
        let mut closed = session.closed.lock().unwrap();
        let first_release = !*closed;
        *closed = true;
        first_release
    };

    if first_release {
        // Prefer graceful release on control connectors; observe has no release wire cmd.
        if matches!(session.mode, HerdrTerminalMode::Control) {
            if let Some(mut stdin) = session.stdin.lock().unwrap().take() {
                let _ = stdin.write_all(b"{\"type\":\"terminal.release\"}\n");
                let _ = stdin.flush();
            }
        } else {
            let _ = session.stdin.lock().unwrap().take();
        }
    }

    let cleanup = terminate_connector_process(session);

    if let Some(handle) = session.reader.lock().unwrap().take() {
        let _ = handle.join();
    }
    cleanup
}

fn terminate_connector_process(session: &ConnectorSession) -> Result<(), String> {
    let (child, process_tree) = {
        let mut process_tree = session.process_tree.lock().unwrap();
        let mut child = session.child.lock().unwrap();
        (child.take(), process_tree.take())
    };
    if let Some(mut child) = child {
        // Only the Yuzora-owned connector Job/process group — never the Herdr
        // server or pane processes, which are not descendants of this child.
        if let Some(mut process_tree) = process_tree {
            process_kill::terminate_process_tree(&mut child, &mut process_tree)
                .map_err(|error| format!("connector process-tree cleanup failed: {error}"))?;
        } else {
            if child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                child.kill().map_err(|error| error.to_string())?;
            }
            process_kill::reap_bounded(&mut child)
                .map_err(|error| format!("connector reap failed: {error}"))?;
        }
    }
    Ok(())
}

fn emit_subscription_event(
    on_event: &OnSubscriptionEvent,
    event: HerdrSubscriptionEvent,
) -> Result<(), String> {
    ensure_ipc_bound(&event).map_err(String::from)?;
    on_event(event)
}

fn emit_terminal_event(
    on_event: &OnTerminalEvent,
    event: HerdrTerminalEvent,
) -> Result<(), String> {
    if let Err(error) = ensure_ipc_bound(&event) {
        let session_id = match &event {
            HerdrTerminalEvent::Frame { session_id, .. }
            | HerdrTerminalEvent::Closed { session_id, .. }
            | HerdrTerminalEvent::Resync { session_id, .. }
            | HerdrTerminalEvent::Error { session_id, .. } => session_id.clone(),
        };
        let _ = on_event(HerdrTerminalEvent::Error {
            session_id,
            code: error.code().into(),
            message: error.to_string(),
        });
        return Err(error.into());
    }
    on_event(event)
}

fn connector_reader_loop<R: std::io::Read + Send + 'static>(
    session: Arc<ConnectorSession>,
    stdout: R,
    stderr: Option<impl std::io::Read + Send + 'static>,
    on_event: OnTerminalEvent,
) {
    if let Some(stderr) = stderr {
        let session_id = session.id.clone();
        let on_event_err = on_event.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            loop {
                let mut line = String::new();
                match read_bounded_ndjson_line(&mut reader, &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let _ = emit_terminal_event(
                            &on_event_err,
                            HerdrTerminalEvent::Error {
                                session_id: session_id.clone(),
                                code: "connector_stderr".into(),
                                message: trimmed.to_string(),
                            },
                        );
                    }
                    Err(error) => {
                        let _ = emit_terminal_event(
                            &on_event_err,
                            HerdrTerminalEvent::Error {
                                session_id: session_id.clone(),
                                code: match &error {
                                    BoundedNdjsonReadError::Protocol(protocol) => {
                                        protocol.code().into()
                                    }
                                    BoundedNdjsonReadError::Io(_) => "connector_stderr".into(),
                                },
                                message: error.to_string(),
                            },
                        );
                        break;
                    }
                }
            }
        });
    }

    let mut tracker = FrameTracker::new();
    let mut emitted_closed = false;
    let mut reader = BufReader::new(stdout);
    loop {
        if *session.closed.lock().unwrap() {
            break;
        }
        let mut line = String::new();
        match read_bounded_ndjson_line(&mut reader, &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Error {
                        session_id: session.id.clone(),
                        code: match &error {
                            BoundedNdjsonReadError::Protocol(protocol) => protocol.code().into(),
                            BoundedNdjsonReadError::Io(_) => "connector_read".into(),
                        },
                        message: error.to_string(),
                    },
                );
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(err) => {
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Error {
                        session_id: session.id.clone(),
                        code: "frame_parse".into(),
                        message: format!("invalid connector json: {err}"),
                    },
                );
                continue;
            }
        };
        if let Err(error) = validate_json_complexity(&value) {
            let _ = emit_terminal_event(
                &on_event,
                HerdrTerminalEvent::Error {
                    session_id: session.id.clone(),
                    code: error.code().into(),
                    message: error.to_string(),
                },
            );
            break;
        }
        let wire: HerdrWireFrame = match serde_json::from_value(value) {
            Ok(v) => v,
            Err(err) => {
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Error {
                        session_id: session.id.clone(),
                        code: "frame_parse".into(),
                        message: format!("invalid connector json: {err}"),
                    },
                );
                continue;
            }
        };

        match tracker.ingest_wire(&wire) {
            FrameDecision::Accept(frame) => {
                if emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Frame {
                        session_id: session.id.clone(),
                        seq: frame.seq,
                        full: frame.full,
                        encoding: frame.encoding,
                        width: frame.width,
                        height: frame.height,
                        bytes_base64: frame.bytes_base64,
                    },
                )
                .is_err()
                {
                    break;
                }
            }
            FrameDecision::IgnoreDuplicate { .. } | FrameDecision::Ignore => {}
            FrameDecision::Resync {
                expected_seq,
                received_seq,
                message,
            } => {
                if emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Resync {
                        session_id: session.id.clone(),
                        expected_seq,
                        received_seq,
                        message,
                    },
                )
                .is_err()
                {
                    break;
                }
            }
            FrameDecision::Closed { reason } => {
                emitted_closed = true;
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Closed {
                        session_id: session.id.clone(),
                        reason,
                    },
                );
                break;
            }
        }
    }

    let was_released = *session.closed.lock().unwrap();
    if !was_released && !emitted_closed {
        let _ = emit_terminal_event(
            &on_event,
            HerdrTerminalEvent::Closed {
                session_id: session.id.clone(),
                reason: Some("connector_eof".into()),
            },
        );
    }
    *session.closed.lock().unwrap() = true;
    let _ = session.stdin.lock().unwrap().take();
    let _ = terminate_connector_process(&session);
}

// ── Binary / API helpers ────────────────────────────────────────────────────

fn select_binary_resolution(
    source: HerdrBinarySource,
    has_explicit_override: bool,
    primary: (Option<PathBuf>, Option<String>),
    managed: (Option<PathBuf>, Option<String>),
) -> (Option<PathBuf>, Option<HerdrBinarySource>, Option<String>) {
    let (primary_path, primary_reason) = primary;
    if primary_path.is_some() {
        return (primary_path, Some(source), primary_reason);
    }
    if source != HerdrBinarySource::Global || has_explicit_override {
        return (None, None, primary_reason);
    }

    let (managed_path, managed_reason) = managed;
    if managed_path.is_some() {
        return (
            managed_path,
            Some(HerdrBinarySource::Default),
            Some("Herdr was not found on PATH; using Yuzora-managed Herdr".into()),
        );
    }
    let primary_reason =
        primary_reason.unwrap_or_else(|| "Herdr was not found on PATH".to_string());
    let managed_reason =
        managed_reason.unwrap_or_else(|| "Yuzora-managed Herdr is unavailable".to_string());
    (
        None,
        None,
        Some(format!(
            "{primary_reason}; Yuzora-managed fallback is also unavailable: {managed_reason}"
        )),
    )
}

fn managed_binary_path(resource_dir: &Path) -> PathBuf {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let file_name = if cfg!(windows) { "herdr.exe" } else { "herdr" };
    resource_dir
        .join("herdr")
        .join(format!("{os}-{arch}"))
        .join(file_name)
}

fn binary_source_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join(BINARY_SOURCE_CONFIG_FILE)
}

struct BinarySourcePreferenceLoad {
    source: HerdrBinarySource,
    error: Option<String>,
}

fn load_binary_source_preference(config_dir: &Path) -> BinarySourcePreferenceLoad {
    let path = binary_source_config_path(config_dir);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return BinarySourcePreferenceLoad {
                source: HerdrBinarySource::Global,
                error: None,
            };
        }
        Err(error) => {
            return BinarySourcePreferenceLoad {
                source: HerdrBinarySource::Global,
                error: Some(format!(
                    "failed to read Herdr binary-source preference at {}: {error}",
                    path.display()
                )),
            };
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => value,
        Err(error) => {
            return BinarySourcePreferenceLoad {
                source: HerdrBinarySource::Global,
                error: Some(format!(
                    "invalid Herdr binary-source preference at {}: {error}",
                    path.display()
                )),
            };
        }
    };
    match value.get("binarySource").and_then(|v| v.as_str()) {
        Some("global") => BinarySourcePreferenceLoad {
            source: HerdrBinarySource::Global,
            error: None,
        },
        Some("default") => BinarySourcePreferenceLoad {
            source: HerdrBinarySource::Default,
            error: None,
        },
        other => BinarySourcePreferenceLoad {
            source: HerdrBinarySource::Global,
            error: Some(format!(
                "unknown Herdr binarySource {:?} in {}",
                other,
                path.display()
            )),
        },
    }
}

fn save_binary_source_preference(
    config_dir: &Path,
    source: HerdrBinarySource,
) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|e| format!("failed to create herdr config dir: {e}"))?;
    let path = binary_source_config_path(config_dir);
    let value = serde_json::json!({
        "binarySource": match source {
            HerdrBinarySource::Global => "global",
            HerdrBinarySource::Default => "default",
        }
    });
    let body = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(config_dir)
        .map_err(|e| format!("failed to create temporary herdr config: {e}"))?;
    temp.write_all(body.as_bytes())
        .and_then(|_| temp.flush())
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| format!("failed to flush herdr config: {e}"))?;
    temp.persist(&path)
        .map_err(|e| format!("failed to atomically replace herdr config: {}", e.error))?;
    #[cfg(unix)]
    fs::File::open(config_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("failed to sync herdr config directory: {e}"))?;
    Ok(())
}

fn probe_binary_identity(binary: &Path) -> (Option<String>, Option<u32>) {
    let Ok(status) = run_herdr_json(binary, &["status", "--json"]) else {
        return (None, None);
    };
    let client = status.get("client").unwrap_or(&status);
    (
        client
            .get("version")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        client
            .get("protocol")
            .and_then(|value| value.as_u64())
            .map(|value| value as u32),
    )
}

fn release_event_subscription(subscription: &EventSubscription) {
    subscription.closed.store(true, Ordering::SeqCst);
    if let Some(handle) = subscription.reader.lock().unwrap().take() {
        let _ = handle.join();
    }
}

fn parse_subscription_event_line(
    subscription_id: &str,
    line: &str,
) -> Result<Option<HerdrSubscriptionEvent>, String> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("invalid subscription event json: {e}"))?;
    validate_json_complexity(&value).map_err(String::from)?;
    if let Some(err) = value.get("error") {
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("subscription error");
        return Ok(Some(HerdrSubscriptionEvent::Error {
            subscription_id: subscription_id.to_string(),
            message: message.to_string(),
        }));
    }
    let event_kind = value
        .get("event")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("type").and_then(|v| v.as_str()))
        .unwrap_or("");
    let data = value.get("data").unwrap_or(&value);
    let data_kind = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if event_kind == "pane.exited" || event_kind == "pane_exited" || data_kind == "pane_exited" {
        let pane_id = data
            .get("pane_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "pane_exited missing pane_id".to_string())?
            .to_string();
        let workspace_id = data
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        return Ok(Some(HerdrSubscriptionEvent::PaneExited {
            subscription_id: subscription_id.to_string(),
            pane_id,
            workspace_id,
        }));
    }
    let worktree_kind = [event_kind, data_kind]
        .into_iter()
        .find_map(|kind| match kind {
            "worktree.created" | "worktree_created" => Some("created"),
            "worktree.opened" | "worktree_opened" => Some("opened"),
            "worktree.removed" | "worktree_removed" => Some("removed"),
            _ => None,
        });
    if let Some(kind) = worktree_kind {
        let kind = kind.to_string();
        let workspace_id = data
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                data.get("workspace")
                    .and_then(|w| w.get("workspace_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        return Ok(Some(HerdrSubscriptionEvent::WorktreeChanged {
            subscription_id: subscription_id.to_string(),
            kind,
            workspace_id,
        }));
    }
    let topology_kind = [event_kind, data_kind]
        .into_iter()
        .find_map(|kind| match kind {
            "tab.created" | "tab_created" => Some("tab.created"),
            "tab.closed" | "tab_closed" => Some("tab.closed"),
            "tab.moved" | "tab_moved" => Some("tab.moved"),
            "workspace.created" | "workspace_created" => Some("workspace.created"),
            "workspace.closed" | "workspace_closed" => Some("workspace.closed"),
            "workspace.moved" | "workspace_moved" => Some("workspace.moved"),
            "workspace.reordered" | "workspace_reordered" => Some("workspace.reordered"),
            _ => None,
        });
    if let Some(kind) = topology_kind {
        let workspace_id = data
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                data.get("tab")
                    .and_then(|tab| tab.get("workspace_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .or_else(|| {
                data.get("workspace")
                    .and_then(|workspace| workspace.get("workspace_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        let tab_id = data
            .get("tab_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                data.get("tab")
                    .and_then(|tab| tab.get("tab_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        return Ok(Some(HerdrSubscriptionEvent::TopologyChanged {
            subscription_id: subscription_id.to_string(),
            kind: kind.to_string(),
            workspace_id,
            tab_id,
        }));
    }
    if event_kind != "pane.agent_status_changed"
        && event_kind != "pane_agent_status_changed"
        && data_kind != "pane_agent_status_changed"
    {
        return Ok(None);
    }
    let pane_id = data
        .get("pane_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "agent_status_changed missing pane_id".to_string())?
        .to_string();
    let workspace_id = data
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let agent_status = data
        .get("agent_status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let mut state_labels = HashMap::new();
    if let Some(obj) = data.get("state_labels").and_then(|v| v.as_object()) {
        if obj.len() > MAX_STATE_LABELS {
            return Err(HerdrProtocolError::TooComplex("state_labels").into());
        }
        for (key, value) in obj {
            if let Some(text) = value.as_str() {
                state_labels.insert(key.clone(), text.to_string());
            }
        }
    }
    Ok(Some(HerdrSubscriptionEvent::AgentStatusChanged {
        subscription_id: subscription_id.to_string(),
        pane_id,
        workspace_id,
        agent_status,
        agent: data
            .get("agent")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        display_agent: data
            .get("display_agent")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        title: data
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        execution_origin: data.get("execution_origin").cloned(),
        state_labels,
    }))
}

fn validate_explicit_pane_target(target: &str) -> Result<(), String> {
    let trimmed = target.trim();
    let explicit = trimmed == target
        && !trimmed.chars().any(char::is_control)
        && trimmed
            .split_once(":p")
            .is_some_and(|(workspace, pane)| !workspace.is_empty() && !pane.is_empty());
    if explicit {
        Ok(())
    } else {
        Err("agent target must be an explicit Herdr pane id (for example w1:p1)".into())
    }
}

fn parse_agent_get_response(response: serde_json::Value) -> Result<HerdrAgentDetails, String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: ResultBody,
    }
    #[derive(serde::Deserialize)]
    struct ResultBody {
        #[serde(rename = "type")]
        result_type: String,
        agent: AgentBody,
    }
    #[derive(serde::Deserialize)]
    struct AgentBody {
        terminal_id: String,
        agent_status: String,
        workspace_id: String,
        tab_id: String,
        pane_id: String,
        focused: bool,
        revision: u64,
        agent: Option<String>,
        display_agent: Option<String>,
        name: Option<String>,
        title: Option<String>,
        cwd: Option<String>,
        foreground_cwd: Option<String>,
        interactive_ready: Option<bool>,
        launch_pending: Option<bool>,
        #[serde(default)]
        state_labels: HashMap<String, String>,
    }

    let envelope: Envelope = serde_json::from_value(response)
        .map_err(|error| format!("invalid agent.get response: {error}"))?;
    if envelope.result.result_type != "agent_info" {
        return Err(format!(
            "unexpected agent.get result type: {}",
            envelope.result.result_type
        ));
    }
    let agent = envelope.result.agent;
    if agent.state_labels.len() > MAX_STATE_LABELS {
        return Err(HerdrProtocolError::TooComplex("state_labels").into());
    }
    Ok(HerdrAgentDetails {
        terminal_id: agent.terminal_id,
        agent_status: agent.agent_status,
        workspace_id: agent.workspace_id,
        tab_id: agent.tab_id,
        pane_id: agent.pane_id,
        focused: agent.focused,
        revision: agent.revision,
        agent: agent.agent,
        display_agent: agent.display_agent,
        name: agent.name,
        title: agent.title,
        cwd: agent.cwd,
        foreground_cwd: agent.foreground_cwd,
        interactive_ready: agent.interactive_ready,
        launch_pending: agent.launch_pending,
        state_labels: agent.state_labels,
    })
}

fn parse_agent_read_response(response: serde_json::Value) -> Result<HerdrAgentReadResult, String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: ResultBody,
    }
    #[derive(serde::Deserialize)]
    struct ResultBody {
        #[serde(rename = "type")]
        result_type: String,
        read: ReadBody,
    }
    #[derive(serde::Deserialize)]
    struct ReadBody {
        pane_id: String,
        workspace_id: String,
        tab_id: String,
        source: HerdrReadSource,
        format: HerdrReadFormat,
        text: String,
        revision: u64,
        truncated: bool,
    }

    let envelope: Envelope = serde_json::from_value(response)
        .map_err(|error| format!("invalid agent.read response: {error}"))?;
    if envelope.result.result_type != "pane_read" {
        return Err(format!(
            "unexpected agent.read result type: {}",
            envelope.result.result_type
        ));
    }
    let read = envelope.result.read;
    let (text, too_large) = bound_agent_text(read.text);
    Ok(HerdrAgentReadResult {
        pane_id: read.pane_id,
        workspace_id: read.workspace_id,
        tab_id: read.tab_id,
        source: read.source,
        format: read.format,
        text,
        revision: read.revision,
        truncated: read.truncated || too_large,
        too_large,
    })
}

fn parse_agent_manifest_response(
    response: serde_json::Value,
) -> Result<Vec<HerdrAgentCatalogEntry>, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "agent manifest response missing result".to_string())?;
    let result_type = required_wire_str(result, "type")?;
    if result_type != "agent_manifest_status" {
        return Err(format!(
            "unexpected server.agent_manifests result type: {result_type}"
        ));
    }
    let manifests = result
        .get("manifests")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "agent manifest response missing manifests".to_string())?;
    if manifests.len() > MAX_AGENT_MANIFEST_COUNT {
        return Err(HerdrProtocolError::TooComplex("agent_manifests").into());
    }

    let mut entries = Vec::with_capacity(manifests.len());
    let mut seen = HashSet::new();
    for manifest in manifests {
        let agent = required_wire_str(manifest, "agent")?;
        validate_agent_kind(&agent)?;
        if !seen.insert(agent.clone()) {
            return Err(format!("duplicate herdr agent manifest: {agent}"));
        }
        let source = required_wire_str(manifest, "source")?;
        let source_kind = required_wire_str(manifest, "source_kind")?;
        let detected_binary_path = which_binary(agent_binary_name(&agent));
        entries.push(HerdrAgentCatalogEntry {
            bypass_flags: agent_bypass_flags(&agent)
                .iter()
                .map(|flag| (*flag).to_string())
                .collect(),
            agent,
            source,
            source_kind,
            active_version: manifest
                .get("active_version")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            warning: manifest
                .get("warning")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            detected_binary_path,
        });
    }
    Ok(entries)
}

fn parse_agent_started_response(
    response: serde_json::Value,
    expected_pane_id: &str,
) -> Result<(), String> {
    let result = response
        .get("result")
        .ok_or_else(|| "agent.start response missing result".to_string())?;
    let result_type = required_wire_str(result, "type")?;
    if result_type != "agent_started" {
        return Err(format!("unexpected agent.start result type: {result_type}"));
    }
    let agent = result
        .get("agent")
        .ok_or_else(|| "agent.start response missing agent".to_string())?;
    let pane_id = required_wire_str(agent, "pane_id")?;
    if pane_id != expected_pane_id {
        return Err(format!(
            "agent.start returned pane {pane_id} for requested pane {expected_pane_id}"
        ));
    }
    Ok(())
}

fn pane_get_matches_created_terminal(
    response: &serde_json::Value,
    created: &HerdrTerminalCreateResult,
) -> bool {
    let Some(result) = response.get("result") else {
        return false;
    };
    if result.get("type").and_then(|value| value.as_str()) != Some("pane_info") {
        return false;
    }
    let Some(pane) = result.get("pane") else {
        return false;
    };
    pane.get("pane_id").and_then(|value| value.as_str()) == Some(created.pane_id.as_str())
        && pane.get("terminal_id").and_then(|value| value.as_str())
            == Some(created.terminal_id.as_str())
        && pane.get("tab_id").and_then(|value| value.as_str()) == Some(created.tab_id.as_str())
        && pane.get("workspace_id").and_then(|value| value.as_str())
            == Some(created.workspace_id.as_str())
}

fn pane_process_info_shows_shell_initialization(
    response: &serde_json::Value,
    expected_pane_id: &str,
) -> bool {
    let Some(result) = response.get("result") else {
        return false;
    };
    if result.get("type").and_then(|value| value.as_str()) != Some("pane_process_info") {
        return false;
    }
    let Some(process_info) = result.get("process_info") else {
        return false;
    };
    if process_info.get("pane_id").and_then(|value| value.as_str()) != Some(expected_pane_id) {
        return false;
    }
    let Some(shell_pid) = process_info
        .get("shell_pid")
        .and_then(|value| value.as_u64())
    else {
        return false;
    };
    if process_info
        .get("foreground_process_group_id")
        .and_then(|value| value.as_u64())
        != Some(shell_pid)
    {
        return false;
    }
    process_info
        .get("foreground_processes")
        .and_then(|value| value.as_array())
        .is_some_and(|processes| {
            processes.iter().any(|process| {
                if process.get("pid").and_then(|value| value.as_u64()) != Some(shell_pid) {
                    return false;
                }
                process
                    .get("name")
                    .and_then(|value| value.as_str())
                    .is_some_and(is_pane_shell_process_name)
                    || process
                        .get("argv")
                        .and_then(|value| value.as_array())
                        .and_then(|argv| argv.first())
                        .and_then(|value| value.as_str())
                        .is_some_and(is_pane_shell_process_name)
            })
        })
}

fn is_pane_shell_process_name(value: &str) -> bool {
    let name = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .trim_start_matches('-')
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    matches!(
        name,
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "mksh"
            | "csh"
            | "tcsh"
            | "elvish"
            | "xonsh"
            | "nu"
            | "pwsh"
            | "powershell"
            | "cmd"
    )
}

fn validate_agent_kind(kind: &str) -> Result<&str, String> {
    let kind = kind.trim();
    if kind.is_empty() || kind.len() > 64 {
        return Err("agent kind must contain 1 to 64 characters".into());
    }
    if !kind
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("agent kind contains unsupported characters".into());
    }
    Ok(kind)
}

fn agent_binary_name(kind: &str) -> &str {
    if kind == "cursor" {
        "cursor-agent"
    } else {
        kind
    }
}

fn agent_bypass_flags(kind: &str) -> &'static [&'static str] {
    match kind {
        "claude" => &["--dangerously-skip-permissions"],
        "codex" => &["--dangerously-bypass-approvals-and-sandbox"],
        "grok" => &["--always-approve"],
        "gemini" => &["--yolo"],
        "opencode" => &["--auto"],
        "cursor" => &["--force"],
        "copilot" => &["--allow-all-tools"],
        _ => &[],
    }
}

#[cfg(any(windows, test))]
fn windows_executable_extensions(raw: Option<&str>) -> Vec<String> {
    let raw = raw
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(".EXE;.CMD;.BAT;.COM");
    let mut seen = HashSet::new();
    raw.split(';')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                return None;
            }
            let normalized = if trimmed.starts_with('.') {
                trimmed.to_string()
            } else {
                format!(".{trimmed}")
            };
            if normalized.len() < 2
                || normalized.len() > 16
                || !normalized[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric())
            {
                return None;
            }
            let key = normalized.to_ascii_lowercase();
            seen.insert(key).then_some(normalized)
        })
        .collect()
}

fn which_binary(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(command);
        if is_executable(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var("PATHEXT").ok();
            for ext in windows_executable_extensions(pathext.as_deref()) {
                let candidate = dir.join(format!("{command}{ext}"));
                if is_executable(&candidate) {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn run_herdr_json(binary: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    run_herdr_json_with_session(binary, args, None)
}

fn run_herdr_json_with_session(
    binary: &Path,
    args: &[&str],
    session_name: Option<&str>,
) -> Result<serde_json::Value, String> {
    run_herdr_json_with_session_timeout(binary, args, session_name, HERDR_CLI_TIMEOUT)
}

fn run_herdr_json_with_session_timeout(
    binary: &Path,
    args: &[&str],
    session_name: Option<&str>,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new(binary);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    process_kill::configure_background_process(&mut cmd);
    if let Some(name) = session_name.filter(|s| !s.trim().is_empty()) {
        cmd.env("HERDR_SESSION", name);
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let mut process_tree = process_kill::attach_process_tree(&mut child)
        .map_err(|e| format!("process containment failed: {e}"))?;
    let (stdout, stderr, status) = wait_bounded_child(
        &mut child,
        &mut process_tree,
        timeout,
        MAX_NDJSON_LINE_BYTES,
    )?;
    let stderr = String::from_utf8(stderr).map_err(|_| HerdrProtocolError::InvalidUtf8)?;
    if !status.success() {
        return Err(format!(
            "exit {}: {}",
            status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    parse_herdr_cli_stdout(&stdout).map_err(String::from)
}

fn wait_bounded_child(
    child: &mut Child,
    process_tree: &mut process_kill::ProcessTreeGuard,
    timeout: Duration,
    max_bytes: usize,
) -> Result<(Vec<u8>, Vec<u8>, std::process::ExitStatus), String> {
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let cleanup = process_kill::terminate_process_tree(child, process_tree);
            return Err(match cleanup {
                Ok(()) => "herdr stdout pipe missing".into(),
                Err(error) => format!("herdr stdout pipe missing; cleanup failed: {error}"),
            });
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let cleanup = process_kill::terminate_process_tree(child, process_tree);
            return Err(match cleanup {
                Ok(()) => "herdr stderr pipe missing".into(),
                Err(error) => format!("herdr stderr pipe missing; cleanup failed: {error}"),
            });
        }
    };
    let too_large = Arc::new(AtomicBool::new(false));
    let stdout_flag = Arc::clone(&too_large);
    let stderr_flag = Arc::clone(&too_large);
    let stdout_thread = std::thread::spawn(move || {
        let mut reader = stdout;
        let result = read_bounded_bytes(&mut reader, max_bytes);
        if matches!(
            result,
            Err(BoundedNdjsonReadError::Protocol(
                HerdrProtocolError::ResponseTooLarge
            ))
        ) {
            stdout_flag.store(true, Ordering::SeqCst);
        }
        result
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut reader = stderr;
        let result = read_bounded_bytes(&mut reader, max_bytes);
        if matches!(
            result,
            Err(BoundedNdjsonReadError::Protocol(
                HerdrProtocolError::ResponseTooLarge
            ))
        ) {
            stderr_flag.store(true, Ordering::SeqCst);
        }
        result
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut wait_error = None;
    let mut cleanup_error = None;
    loop {
        if too_large.load(Ordering::SeqCst) {
            cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
            break;
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                wait_error = Some(error);
                cleanup_error = process_kill::terminate_process_tree(child, process_tree).err();
                break;
            }
        }
    }
    let status = process_kill::reap_process_tree(child, process_tree);
    let stdout = stdout_thread
        .join()
        .map_err(|_| "stdout reader panicked".to_string())?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "stderr reader panicked".to_string())?;

    if let Some(error) = cleanup_error {
        return Err(format!("process-tree cleanup failed: {error}"));
    }
    let status = status.map_err(|error| format!("reap failed: {error}"))?;
    if let Some(error) = wait_error {
        return Err(format!("wait failed: {error}"));
    }
    let stdout = match stdout {
        Ok(bytes) => bytes,
        Err(BoundedNdjsonReadError::Protocol(error)) => return Err(error.into()),
        Err(BoundedNdjsonReadError::Io(error)) => {
            return Err(format!("stdout read failed: {error}"));
        }
    };
    let stderr = match stderr {
        Ok(bytes) => bytes,
        Err(BoundedNdjsonReadError::Protocol(error)) => return Err(error.into()),
        Err(BoundedNdjsonReadError::Io(error)) => {
            return Err(format!("stderr read failed: {error}"));
        }
    };
    if timed_out {
        return Err(HerdrProtocolError::TimedOut.into());
    }
    Ok((stdout, stderr, status))
}

fn parse_session_list_json(value: &serde_json::Value) -> Result<Vec<HerdrNamedSession>, String> {
    let sessions = value
        .get("sessions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "session list missing sessions array".to_string())?;
    if sessions.len() > MAX_SESSION_COUNT {
        return Err(HerdrProtocolError::TooComplex("sessions").into());
    }
    let mut out = Vec::with_capacity(sessions.len());
    for item in sessions {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "session list entry missing name".to_string())?
            .to_string();
        let default = item
            .get("default")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let running = item
            .get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let session_dir = item
            .get("session_dir")
            .or_else(|| item.get("sessionDir"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Only accept socket paths from the listing itself — never synthesize.
        let socket_path = item
            .get("socket_path")
            .or_else(|| item.get("socketPath"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(HerdrNamedSession {
            name,
            default,
            running,
            session_dir,
            socket_path,
        });
    }
    Ok(out)
}

fn parse_workspace_created_response(
    response: serde_json::Value,
) -> Result<HerdrWorkspaceCreateResult, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "workspace.create response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "workspace_created" {
        return Err(format!(
            "unexpected workspace.create result type: {result_type}"
        ));
    }
    let workspace = result
        .get("workspace")
        .ok_or_else(|| "workspace_created missing workspace".to_string())?;
    let workspace_id = workspace
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "workspace_created missing workspace_id".to_string())?
        .to_string();
    let label = workspace
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(workspace_id.as_str())
        .to_string();
    let path = workspace
        .get("worktree")
        .and_then(|w| w.get("checkout_path"))
        .and_then(|v| v.as_str())
        .or_else(|| workspace.get("path").and_then(|v| v.as_str()))
        .or_else(|| workspace.get("cwd").and_then(|v| v.as_str()))
        .map(str::to_string);
    let tab_id = result
        .get("tab")
        .and_then(|t| t.get("tab_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let root_pane = result.get("root_pane");
    let terminal_id = root_pane
        .and_then(|p| p.get("terminal_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let pane_id = root_pane
        .and_then(|p| p.get("pane_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok(HerdrWorkspaceCreateResult {
        workspace_id,
        label,
        path,
        tab_id,
        terminal_id,
        pane_id,
    })
}

fn parse_worktree_list_response(
    response: serde_json::Value,
) -> Result<HerdrWorktreeListResult, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "worktree.list response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "worktree_list" {
        return Err(format!(
            "unexpected worktree.list result type: {result_type}"
        ));
    }
    let source_val = result
        .get("source")
        .ok_or_else(|| "worktree_list missing source".to_string())?;
    let required_str = |obj: &serde_json::Value, key: &str| -> Result<String, String> {
        obj.get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("worktree_list source missing {key}"))
    };
    let source = HerdrWorktreeSourceInfo {
        repo_key: required_str(source_val, "repo_key")?,
        repo_name: required_str(source_val, "repo_name")?,
        repo_root: required_str(source_val, "repo_root")?,
        source_checkout_path: required_str(source_val, "source_checkout_path")?,
        source_workspace_id: source_val
            .get("source_workspace_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    };
    let worktrees_val = result
        .get("worktrees")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "worktree_list missing worktrees".to_string())?;
    if worktrees_val.len() > MAX_WORKTREE_COUNT {
        return Err(HerdrProtocolError::TooComplex("worktrees").into());
    }
    let mut worktrees = Vec::with_capacity(worktrees_val.len());
    for item in worktrees_val {
        let path = item
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "worktree missing path".to_string())?
            .to_string();
        let label = item
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let required_bool = |key: &str| -> Result<bool, String> {
            item.get(key)
                .and_then(|v| v.as_bool())
                .ok_or_else(|| format!("worktree missing {key}"))
        };
        worktrees.push(HerdrWorktreeInfo {
            path,
            branch: item
                .get("branch")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            is_bare: required_bool("is_bare")?,
            is_detached: required_bool("is_detached")?,
            is_prunable: required_bool("is_prunable")?,
            is_linked_worktree: required_bool("is_linked_worktree")?,
            label,
            open_workspace_id: item
                .get("open_workspace_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    Ok(HerdrWorktreeListResult { source, worktrees })
}

/// Method names Yuzora implements and exposes for menu/capability gating.
const IMPLEMENTED_API_METHODS: &[&str] = &[
    "session.snapshot",
    "ping",
    "workspace.focus",
    "workspace.create",
    "workspace.rename",
    "workspace.close",
    "tab.create",
    "tab.rename",
    "tab.close",
    "tab.focus",
    "tab.move",
    "pane.focus",
    "pane.rename",
    "pane.split",
    "pane.zoom",
    "pane.swap",
    "pane.close",
    "pane.get",
    "pane.process_info",
    "layout.export",
    "layout.set_split_ratio",
    "server.agent_manifests",
    "agent.start",
    "agent.get",
    "agent.read",
    "events.subscribe",
    "worktree.list",
];

fn disable_live_socket_capabilities(caps: &mut HerdrCapabilities, reason: &str) {
    clear_api_method_flags(&mut caps.api);
    caps.api.reason = Some(reason.into());
    caps.terminal.observe = false;
    caps.terminal.control = false;
    caps.terminal.takeover = false;
    caps.terminal.input = false;
    caps.terminal.resize = false;
    caps.terminal.scroll = false;
    caps.terminal.create = false;
    caps.terminal.reason = Some(reason.into());
    caps.events = HerdrEventsCapability {
        status: HerdrEventsStatus::Unavailable,
        reason: Some(reason.into()),
    };
}

fn clear_api_method_flags(api: &mut HerdrApiCapability) {
    api.snapshot = false;
    api.ping = false;
    api.tab_create = false;
    api.workspace_focus = false;
    api.workspace_create = false;
    api.workspace_rename = false;
    api.workspace_close = false;
    api.tab_rename = false;
    api.tab_close = false;
    api.tab_focus = false;
    api.tab_move = false;
    api.pane_focus = false;
    api.pane_rename = false;
    api.pane_split = false;
    api.pane_zoom = false;
    api.pane_swap = false;
    api.pane_close = false;
    api.layout_export = false;
    api.layout_set_split_ratio = false;
    api.agent_manifests = false;
    api.agent_start = false;
    api.agent_get = false;
    api.agent_read = false;
    api.events_subscribe = false;
    api.worktree_list = false;
    api.methods.clear();
}

fn apply_events_capability(
    events: &mut HerdrEventsCapability,
    socket_ready: bool,
    has_events_subscribe: bool,
    session_stopped: bool,
) {
    if socket_ready && has_events_subscribe {
        *events = HerdrEventsCapability {
            status: HerdrEventsStatus::Available,
            reason: None,
        };
    } else if !has_events_subscribe {
        *events = HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some("selected herdr schema lacks events.subscribe".into()),
        };
    } else if session_stopped {
        *events = HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some("herdr session is not running".into()),
        };
    } else {
        *events = HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some("herdr events.subscribe requires a running compatible session".into()),
        };
    }
}

fn apply_schema_method_flags(
    api: &mut HerdrApiCapability,
    schema_methods: &HashSet<String>,
    has_ping_method: bool,
) {
    let has = |name: &str| schema_methods.contains(name);
    let has_snapshot = has("session.snapshot");
    api.snapshot = has_snapshot;
    api.ping = if schema_methods.is_empty() {
        false
    } else if has_ping_method {
        true
    } else {
        has_snapshot
    };
    api.tab_create = has("tab.create");
    api.workspace_focus = has("workspace.focus");
    api.workspace_create = has("workspace.create");
    api.workspace_rename = has("workspace.rename");
    api.workspace_close = has("workspace.close");
    api.tab_rename = has("tab.rename");
    api.tab_close = has("tab.close");
    api.tab_focus = has("tab.focus");
    api.tab_move = has("tab.move");
    api.pane_focus = has("pane.focus");
    api.pane_rename = has("pane.rename");
    api.pane_split = has("pane.split");
    api.pane_zoom = has("pane.zoom");
    api.pane_swap = has("pane.swap");
    api.pane_close = has("pane.close");
    api.layout_export = has("layout.export");
    api.layout_set_split_ratio = has("layout.set_split_ratio");
    api.agent_manifests = has("server.agent_manifests");
    api.agent_start = has("agent.start");
    api.agent_get = has("agent.get");
    api.agent_read = has("agent.read");
    api.events_subscribe = has("events.subscribe");
    api.worktree_list = has("worktree.list");

    let mut methods = Vec::new();
    for name in IMPLEMENTED_API_METHODS {
        let available = match *name {
            "ping" => api.ping,
            "session.snapshot" => api.snapshot,
            other => has(other),
        };
        if available {
            methods.push((*name).to_string());
        }
    }
    methods.sort();
    api.methods = methods;
}

/// Collect public API method names advertised by `herdr api schema --json`.
/// Looks at:
/// - top-level `methods: [...]`
/// - `schemas` object keys that look like `namespace.method`
/// - JSON Schema `const` values under request unions / subcommands
fn collect_schema_methods(schema: &serde_json::Value) -> HashSet<String> {
    let mut methods = HashSet::new();

    if let Some(arr) = schema.get("methods").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(name) = item.as_str() {
                if looks_like_api_method(name) {
                    methods.insert(name.to_string());
                }
            }
        }
    }

    if let Some(obj) = schema.get("schemas").and_then(|v| v.as_object()) {
        for key in obj.keys() {
            if looks_like_api_method(key) {
                methods.insert(key.clone());
            }
        }
        for key in ["request", "Request", "requests", "methods"] {
            if let Some(node) = obj.get(key) {
                collect_method_consts(node, &mut methods);
            }
        }
    }

    for key in ["request", "Request", "requests"] {
        if let Some(node) = schema.get(key) {
            collect_method_consts(node, &mut methods);
        }
    }

    // Walk the whole document for method-like `const` values (request unions).
    collect_method_consts(schema, &mut methods);
    methods
}

fn looks_like_api_method(name: &str) -> bool {
    let mut parts = name.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(ns), Some(method), None) => {
            !ns.is_empty()
                && !method.is_empty()
                && ns.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && method
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

fn collect_method_consts(value: &serde_json::Value, out: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("const") {
                if looks_like_api_method(s) {
                    out.insert(s.clone());
                }
            }
            if let Some(serde_json::Value::String(s)) = map.get("method") {
                if looks_like_api_method(s) {
                    out.insert(s.clone());
                }
            }
            for child in map.values() {
                collect_method_consts(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                collect_method_consts(child, out);
            }
        }
        serde_json::Value::String(s) if looks_like_api_method(s) => {
            out.insert(s.clone());
        }
        _ => {}
    }
}

fn query_herdr_server_running(binary: &Path) -> Result<bool, String> {
    let status = run_herdr_json_with_session_timeout(
        binary,
        &["status", "--json"],
        None,
        HERDR_STARTUP_STATUS_TIMEOUT,
    )?;
    Ok(status_reports_server_running(&status))
}

fn status_reports_server_running(status: &serde_json::Value) -> bool {
    let server = status.get("server").unwrap_or(status);
    server
        .get("running")
        .and_then(|value| value.as_bool())
        .or_else(|| {
            server
                .get("status")
                .and_then(|value| value.as_str())
                .map(|value| value == "running")
        })
        .unwrap_or(false)
}

fn apply_status_json(caps: &mut HerdrCapabilities, status: &serde_json::Value) {
    if let Some(client) = status.get("client") {
        if let Some(v) = client.get("version").and_then(|v| v.as_str()) {
            caps.binary_version = Some(v.to_string());
        }
        if let Some(p) = client.get("protocol").and_then(|v| v.as_u64()) {
            caps.binary_protocol = Some(p as u32);
        }
        if let Some(c) = client.get("channel").and_then(|v| v.as_str()) {
            caps.channel = Some(c.to_string());
        }
        if let Some(b) = client.get("binary").and_then(|v| v.as_str()) {
            caps.binary_path = Some(b.to_string());
        }
    } else {
        // `herdr status client --json` shape
        if let Some(v) = status.get("version").and_then(|v| v.as_str()) {
            caps.binary_version = Some(v.to_string());
        }
        if let Some(p) = status.get("protocol").and_then(|v| v.as_u64()) {
            caps.binary_protocol = Some(p as u32);
        }
    }

    let server = status.get("server").unwrap_or(status);
    caps.server.running = status_reports_server_running(status);
    caps.server.version = server
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    caps.server.protocol = server
        .get("protocol")
        .and_then(|v| v.as_u64())
        .map(|p| p as u32);
    caps.server.compatible = server.get("compatible").and_then(|v| v.as_bool());
    caps.server.socket_path = server
        .get("socket")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    caps.server.capabilities = server
        .get("capabilities")
        .cloned()
        .and_then(bound_optional_json);
}

fn ping_server_identity(socket_path: &str) -> Result<(String, u32), String> {
    let response = api_request(socket_path, "ping", serde_json::json!({}))?;
    let result = response
        .get("result")
        .ok_or_else(|| "ping response missing result".to_string())?;
    let version = result
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "ping response missing version".to_string())?;
    let protocol = result
        .get("protocol")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "ping response missing protocol".to_string())?;
    Ok((version.to_string(), protocol as u32))
}

fn api_request(
    socket_path: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + LOCAL_IO_TIMEOUT;
    let mut stream = connect_local_stream(socket_path, deadline)
        .map_err(|e| format!("connect {socket_path} failed: {e}"))?;
    let id = format!(
        "yuzora:herdr:{}",
        NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    );
    let req = serde_json::json!({
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    line.push('\n');
    write_local_all_until(&mut stream, line.as_bytes(), deadline)
        .map_err(|e| format!("write failed: {e}"))?;

    let mut pending = Vec::new();
    let response = match read_local_ndjson_line(
        &mut stream,
        &mut pending,
        Some(deadline),
        MAX_NDJSON_LINE_BYTES,
    ) {
        Ok(None) => return Err(HerdrProtocolError::EmptyResponse.into()),
        Ok(Some(response)) => response,
        Err(BoundedNdjsonReadError::Protocol(protocol)) => return Err(protocol.into()),
        Err(BoundedNdjsonReadError::Io(io_error)) => {
            return Err(format!("read failed: {io_error}"))
        }
    };
    if response.trim().is_empty() {
        return Err(HerdrProtocolError::EmptyResponse.into());
    }
    let value: serde_json::Value =
        serde_json::from_str(response.trim()).map_err(|e| format!("invalid api json: {e}"))?;
    if let Err(error) = validate_json_complexity(&value) {
        return Err(error.into());
    }
    if let Some(err) = value.get("error") {
        let code = err.get("code").and_then(|v| v.as_str()).unwrap_or("error");
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        return Err(format!("{code}: {message}"));
    }
    Ok(value)
}

fn parse_snapshot_response(response: serde_json::Value) -> Result<HerdrSnapshotResult, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "snapshot response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "session_snapshot" && result.get("snapshot").is_none() {
        return Err(format!("unexpected snapshot result type: {result_type}"));
    }
    let snapshot = result
        .get("snapshot")
        .cloned()
        .ok_or_else(|| "snapshot result missing snapshot".to_string())?;
    validate_json_complexity(&snapshot).map_err(String::from)?;
    validate_snapshot_counts(&snapshot).map_err(String::from)?;
    let protocol = snapshot
        .get("protocol")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "snapshot missing protocol".to_string())? as u32;
    let version = snapshot
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(HerdrSnapshotResult {
        protocol,
        version,
        snapshot,
    })
}

fn parse_tab_created_response(
    response: serde_json::Value,
) -> Result<HerdrTerminalCreateResult, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "tab.create response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "tab_created" {
        return Err(format!("unexpected tab.create result type: {result_type}"));
    }
    let root_pane = result
        .get("root_pane")
        .ok_or_else(|| "tab_created missing root_pane".to_string())?;
    let tab = result
        .get("tab")
        .ok_or_else(|| "tab_created missing tab".to_string())?;
    let terminal_id = root_pane
        .get("terminal_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created root_pane missing terminal_id".to_string())?
        .to_string();
    let pane_id = root_pane
        .get("pane_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created root_pane missing pane_id".to_string())?
        .to_string();
    let tab_id = tab
        .get("tab_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created missing tab_id".to_string())?;
    let root_tab_id = root_pane
        .get("tab_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created root_pane missing tab_id".to_string())?;
    if root_tab_id != tab_id {
        return Err("tab_created root_pane tab_id does not match tab".into());
    }
    let tab_workspace_id = tab
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created tab missing workspace_id".to_string())?;
    let root_workspace_id = root_pane
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tab_created root_pane missing workspace_id".to_string())?;
    if root_workspace_id != tab_workspace_id {
        return Err("tab_created root_pane workspace_id does not match tab".into());
    }
    let tab_id = tab_id.to_string();
    let workspace_id = root_workspace_id.to_string();
    let title = root_pane
        .get("title")
        .or_else(|| root_pane.get("label"))
        .or_else(|| tab.get("label"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok(HerdrTerminalCreateResult {
        terminal_id,
        pane_id,
        tab_id,
        workspace_id,
        title,
    })
}

fn build_tab_create_params(
    workspace_id: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    focus: bool,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(workspace_id) = workspace_id.filter(|s| !s.trim().is_empty()) {
        params.insert(
            "workspace_id".into(),
            serde_json::Value::String(workspace_id),
        );
    }
    if let Some(label) = label.filter(|s| !s.trim().is_empty()) {
        params.insert("label".into(), serde_json::Value::String(label));
    }
    if let Some(cwd) = cwd.filter(|s| !s.trim().is_empty()) {
        params.insert("cwd".into(), serde_json::Value::String(cwd));
    }
    params.insert("focus".into(), serde_json::Value::Bool(focus));
    serde_json::Value::Object(params)
}

fn build_tab_move_params(tab_id: String, insert_index: u32) -> serde_json::Value {
    serde_json::json!({ "tab_id": tab_id, "insert_index": insert_index })
}

fn build_pane_split_params(
    direction: HerdrSplitDirection,
    target_pane_id: Option<String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
    ratio: Option<f64>,
    focus: bool,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    params.insert(
        "direction".into(),
        serde_json::to_value(direction).unwrap_or(serde_json::Value::Null),
    );
    if let Some(target_pane_id) = target_pane_id.filter(|s| !s.trim().is_empty()) {
        params.insert(
            "target_pane_id".into(),
            serde_json::Value::String(target_pane_id),
        );
    }
    if let Some(workspace_id) = workspace_id.filter(|s| !s.trim().is_empty()) {
        params.insert(
            "workspace_id".into(),
            serde_json::Value::String(workspace_id),
        );
    }
    if let Some(cwd) = cwd.filter(|s| !s.trim().is_empty()) {
        params.insert("cwd".into(), serde_json::Value::String(cwd));
    }
    if let Some(ratio) = ratio {
        params.insert("ratio".into(), serde_json::json!(ratio));
    }
    params.insert("focus".into(), serde_json::Value::Bool(focus));
    serde_json::Value::Object(params)
}

fn build_layout_export_params(
    tab_id: Option<String>,
    pane_id: Option<String>,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(tab_id) = tab_id.filter(|s| !s.trim().is_empty()) {
        params.insert("tab_id".into(), serde_json::Value::String(tab_id));
    }
    if let Some(pane_id) = pane_id.filter(|s| !s.trim().is_empty()) {
        params.insert("pane_id".into(), serde_json::Value::String(pane_id));
    }
    serde_json::Value::Object(params)
}

fn build_layout_set_split_ratio_params(
    tab_id: Option<String>,
    pane_id: Option<String>,
    path: &[bool],
    ratio: f64,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(tab_id) = tab_id.filter(|s| !s.trim().is_empty()) {
        params.insert("tab_id".into(), serde_json::Value::String(tab_id));
    }
    if let Some(pane_id) = pane_id.filter(|s| !s.trim().is_empty()) {
        params.insert("pane_id".into(), serde_json::Value::String(pane_id));
    }
    params.insert(
        "path".into(),
        serde_json::Value::Array(path.iter().map(|b| serde_json::Value::Bool(*b)).collect()),
    );
    params.insert("ratio".into(), serde_json::json!(ratio));
    serde_json::Value::Object(params)
}

fn required_wire_str(obj: &serde_json::Value, key: &str) -> Result<String, String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing {key}"))
}

fn parse_layout_node(node: &serde_json::Value, depth: usize) -> Result<HerdrLayoutNode, String> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(HerdrProtocolError::TooComplex("layout depth").into());
    }
    let kind = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "pane" => Ok(HerdrLayoutNode::Pane {
            pane_id: node
                .get("pane_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            label: node
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            cwd: node.get("cwd").and_then(|v| v.as_str()).map(str::to_string),
        }),
        "split" => {
            let direction = match node.get("direction").and_then(|v| v.as_str()).unwrap_or("") {
                "right" => HerdrSplitDirection::Right,
                "down" => HerdrSplitDirection::Down,
                other => return Err(format!("unknown split direction: {other}")),
            };
            let ratio = node
                .get("ratio")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "split missing ratio".to_string())?;
            let first = node
                .get("first")
                .ok_or_else(|| "split missing first".to_string())?;
            let second = node
                .get("second")
                .ok_or_else(|| "split missing second".to_string())?;
            Ok(HerdrLayoutNode::Split {
                direction,
                ratio,
                first: Box::new(parse_layout_node(first, depth + 1)?),
                second: Box::new(parse_layout_node(second, depth + 1)?),
            })
        }
        other => Err(format!("unknown layout node type: {other}")),
    }
}

fn parse_layout_description(layout: &serde_json::Value) -> Result<HerdrLayoutDescription, String> {
    let root = layout
        .get("root")
        .ok_or_else(|| "layout missing root".to_string())?;
    Ok(HerdrLayoutDescription {
        workspace_id: required_wire_str(layout, "workspace_id")?,
        tab_id: required_wire_str(layout, "tab_id")?,
        zoomed: layout
            .get("zoomed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        focused_pane_id: required_wire_str(layout, "focused_pane_id")?,
        root: parse_layout_node(root, 0)?,
    })
}

fn parse_layout_export_response(
    response: serde_json::Value,
) -> Result<HerdrLayoutDescription, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "layout.export response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "layout_export" {
        return Err(format!(
            "unexpected layout.export result type: {result_type}"
        ));
    }
    let layout = result
        .get("layout")
        .ok_or_else(|| "layout_export missing layout".to_string())?;
    parse_layout_description(layout)
}

fn parse_layout_set_split_ratio_response(
    response: serde_json::Value,
) -> Result<HerdrLayoutDescription, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "layout.set_split_ratio response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "layout_split_ratio_set" {
        return Err(format!(
            "unexpected layout.set_split_ratio result type: {result_type}"
        ));
    }
    let layout = result
        .get("layout")
        .ok_or_else(|| "layout_split_ratio_set missing layout".to_string())?;
    parse_layout_description(layout)
}

fn parse_pane_info_response(response: serde_json::Value) -> Result<HerdrPaneIdentity, String> {
    let result = response
        .get("result")
        .ok_or_else(|| "pane response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "pane_info" && result.get("pane").is_none() {
        return Err(format!("unexpected pane result type: {result_type}"));
    }
    let pane = result
        .get("pane")
        .ok_or_else(|| "pane result missing pane".to_string())?;
    Ok(HerdrPaneIdentity {
        pane_id: required_wire_str(pane, "pane_id")?,
        terminal_id: required_wire_str(pane, "terminal_id")?,
        tab_id: required_wire_str(pane, "tab_id")?,
        workspace_id: required_wire_str(pane, "workspace_id")?,
        title: pane
            .get("title")
            .or_else(|| pane.get("label"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn herdr_sessions(
    state: tauri::State<'_, HerdrState>,
) -> Result<Vec<HerdrNamedSession>, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.list_sessions())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_capabilities(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrCapabilities, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        bounded_ipc(manager.capabilities_for_session(session_name.as_deref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_snapshot(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrSnapshotResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.snapshot(session_name.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn herdr_terminal_open(
    state: tauri::State<'_, HerdrState>,
    target: String,
    mode: Option<HerdrTerminalMode>,
    takeover: Option<bool>,
    cols: u16,
    rows: u16,
    session_name: Option<String>,
    on_event: tauri::ipc::Channel<HerdrTerminalEvent>,
) -> Result<HerdrTerminalOpenResult, String> {
    let manager = state.0.clone();
    let mode = mode.unwrap_or(HerdrTerminalMode::Observe);
    let takeover = takeover.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_event;
        let on_event: OnTerminalEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.open_terminal(target, mode, takeover, cols, rows, session_name, on_event)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_input(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    text: Option<String>,
    bytes_base64: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.terminal_input(&session_id, text, bytes_base64)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_resize(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.terminal_resize(&session_id, cols, rows))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_scroll(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    direction: HerdrScrollDirection,
    lines: u32,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.terminal_scroll(&session_id, direction, lines)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_release(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.terminal_release(&session_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: Option<String>,
    title: Option<String>,
) -> Result<HerdrTerminalCreateResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.create_terminal(session_name.as_deref(), workspace_id, title)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_catalog(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<Vec<HerdrAgentCatalogEntry>, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.agent_catalog(session_name.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    kind: String,
    bypass_permissions: Option<bool>,
) -> Result<HerdrAgentCreateResult, String> {
    let manager = state.0.clone();
    let bypass_permissions = bypass_permissions.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        manager.agent_create(
            session_name.as_deref(),
            workspace_id,
            kind,
            bypass_permissions,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_focus(session_name.as_deref(), workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    label: Option<String>,
    focus: Option<bool>,
) -> Result<HerdrWorkspaceCreateResult, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_create(session_name.as_deref(), cwd, label, focus)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    label: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_rename(session_name.as_deref(), workspace_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_close(session_name.as_deref(), workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_worktree_list(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    workspace_id: Option<String>,
) -> Result<HerdrWorktreeListResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.worktree_list(session_name.as_deref(), cwd, workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    focus: Option<bool>,
) -> Result<HerdrTerminalCreateResult, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_create(session_name.as_deref(), workspace_id, label, cwd, focus)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.tab_focus(session_name.as_deref(), tab_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    label: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_rename(session_name.as_deref(), tab_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.tab_close(session_name.as_deref(), tab_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_move(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    insert_index: u32,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_move(session_name.as_deref(), tab_id, insert_index)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_focus(session_name.as_deref(), pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
    label: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_rename(session_name.as_deref(), pane_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn herdr_pane_split(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    direction: HerdrSplitDirection,
    target_pane_id: Option<String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
    ratio: Option<f64>,
    focus: Option<bool>,
) -> Result<HerdrPaneIdentity, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_split(
            session_name.as_deref(),
            direction,
            target_pane_id,
            workspace_id,
            cwd,
            ratio,
            focus,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_zoom(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: Option<String>,
    mode: Option<HerdrPaneZoomMode>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_zoom(session_name.as_deref(), pane_id, mode)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_swap(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    source_pane_id: Option<String>,
    target_pane_id: Option<String>,
    pane_id: Option<String>,
    direction: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_swap(
            session_name.as_deref(),
            source_pane_id,
            target_pane_id,
            pane_id,
            direction,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_close(session_name.as_deref(), pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_layout_export(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
) -> Result<HerdrLayoutDescription, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.layout_export(session_name.as_deref(), tab_id, pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_layout_set_split_ratio(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    path: Vec<bool>,
    ratio: f64,
) -> Result<HerdrLayoutDescription, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.layout_set_split_ratio(session_name.as_deref(), tab_id, pane_id, path, ratio)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_binary_source_get(
    state: tauri::State<'_, HerdrState>,
) -> Result<HerdrBinarySourceInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.binary_source_info())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn herdr_binary_source_set(
    state: tauri::State<'_, HerdrState>,
    source: HerdrBinarySource,
) -> Result<HerdrBinarySourceSetResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_binary_source(source))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_get(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    target: String,
) -> Result<HerdrAgentDetails, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.agent_get(session_name.as_deref(), target))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_read(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    target: String,
    source: HerdrReadSource,
    format: Option<HerdrReadFormat>,
    lines: Option<u32>,
    strip_ansi: Option<bool>,
) -> Result<HerdrAgentReadResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.agent_read(
            session_name.as_deref(),
            target,
            source,
            format,
            lines,
            strip_ansi,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_events_subscribe(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    on_event: tauri::ipc::Channel<HerdrSubscriptionEvent>,
) -> Result<String, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_event;
        let on_event: OnSubscriptionEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.events_subscribe(session_name, on_event)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_events_release(
    state: tauri::State<'_, HerdrState>,
    subscription_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.events_release(&subscription_id))
        .await
        .map_err(|e| e.to_string())?
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::io::BufRead;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;
    #[cfg(unix)]
    use std::time::Instant;

    fn frame(seq: u64, full: bool) -> HerdrWireFrame {
        HerdrWireFrame {
            kind: "terminal.frame".into(),
            seq: Some(seq),
            full: Some(full),
            encoding: Some("ansi".into()),
            width: Some(40),
            height: Some(10),
            bytes: Some("AAA=".into()),
            reason: None,
        }
    }

    #[test]
    fn frame_tracker_requires_first_full_frame() {
        let mut tracker = FrameTracker::new();
        match tracker.ingest_frame(&frame(1, false)) {
            FrameDecision::Resync { message, .. } => {
                assert!(message.contains("first terminal.frame must be full"));
            }
            other => panic!("expected resync, got {other:?}"),
        }
    }

    #[test]
    fn frame_tracker_accepts_contiguous_and_ignores_duplicates() {
        let mut tracker = FrameTracker::new();
        assert!(matches!(
            tracker.ingest_frame(&frame(1, true)),
            FrameDecision::Accept(_)
        ));
        assert!(matches!(
            tracker.ingest_frame(&frame(1, true)),
            FrameDecision::IgnoreDuplicate { seq: 1 }
        ));
        assert!(matches!(
            tracker.ingest_frame(&frame(2, false)),
            FrameDecision::Accept(ParsedTerminalFrame {
                seq: 2,
                full: false,
                ..
            })
        ));
    }

    #[test]
    fn frame_tracker_gap_becomes_resync() {
        let mut tracker = FrameTracker::new();
        assert!(matches!(
            tracker.ingest_frame(&frame(1, true)),
            FrameDecision::Accept(_)
        ));
        match tracker.ingest_frame(&frame(4, false)) {
            FrameDecision::Resync {
                expected_seq: Some(2),
                received_seq: Some(4),
                ..
            } => {}
            other => panic!("expected gap resync, got {other:?}"),
        }
    }

    #[test]
    fn control_command_json_matches_herdr_wire() {
        let input = TerminalControlCommand::input(Some("hi".into()), None).unwrap();
        assert_eq!(
            serde_json::to_string(&input).unwrap(),
            r#"{"type":"terminal.input","text":"hi"}"#
        );
        let bytes = TerminalControlCommand::input(None, Some("eA==".into())).unwrap();
        assert_eq!(
            serde_json::to_string(&bytes).unwrap(),
            r#"{"type":"terminal.input","bytes":"eA=="}"#
        );
        let resize = TerminalControlCommand::resize(80, 24).unwrap();
        assert_eq!(
            serde_json::to_string(&resize).unwrap(),
            r#"{"type":"terminal.resize","cols":80,"rows":24}"#
        );
        let scroll = TerminalControlCommand::scroll(HerdrScrollDirection::Up, 3).unwrap();
        assert_eq!(
            serde_json::to_string(&scroll).unwrap(),
            r#"{"type":"terminal.scroll","direction":"up","lines":3}"#
        );
        assert_eq!(
            serde_json::to_string(&TerminalControlCommand::Release).unwrap(),
            r#"{"type":"terminal.release"}"#
        );
    }

    #[test]
    fn control_command_rejects_invalid_combos() {
        assert!(TerminalControlCommand::input(Some("a".into()), Some("eA==".into())).is_err());
        assert!(TerminalControlCommand::input(None, None).is_err());
        assert!(TerminalControlCommand::resize(0, 24).is_err());
        assert!(TerminalControlCommand::scroll(HerdrScrollDirection::Down, 0).is_err());
    }

    #[test]
    fn parse_snapshot_response_reads_protocol_from_payload() {
        let response = serde_json::json!({
            "id": "1",
            "result": {
                "type": "session_snapshot",
                "snapshot": {
                    "version": "0.8.0",
                    "protocol": 19,
                    "workspaces": [],
                    "tabs": [],
                    "panes": [],
                    "layouts": [],
                    "agents": []
                }
            }
        });
        let parsed = parse_snapshot_response(response).unwrap();
        assert_eq!(parsed.protocol, 19);
        assert_eq!(parsed.version, "0.8.0");
        assert_eq!(parsed.snapshot["protocol"], 19);
    }

    #[test]
    fn parse_tab_created_response_reads_root_pane_identity() {
        let response = serde_json::json!({
            "id": "1",
            "result": {
                "type": "tab_created",
                "tab": {
                    "tab_id": "tab_9",
                    "workspace_id": "ws_1",
                    "number": 2,
                    "label": "Shell",
                    "focused": true,
                    "pane_count": 1,
                    "agent_status": "idle"
                },
                "root_pane": {
                    "pane_id": "pane_9",
                    "terminal_id": "term_9",
                    "workspace_id": "ws_1",
                    "tab_id": "tab_9",
                    "focused": true,
                    "agent_status": "idle",
                    "revision": 1,
                    "title": "Shell"
                }
            }
        });
        let parsed = parse_tab_created_response(response).unwrap();
        assert_eq!(parsed.terminal_id, "term_9");
        assert_eq!(parsed.pane_id, "pane_9");
        assert_eq!(parsed.tab_id, "tab_9");
        assert_eq!(parsed.workspace_id, "ws_1");
        assert_eq!(parsed.title.as_deref(), Some("Shell"));
    }

    #[test]
    fn parse_tab_created_response_rejects_cross_tab_or_workspace_identity() {
        let response = |root_tab_id: &str, root_workspace_id: &str| {
            serde_json::json!({
                "id": "1",
                "result": {
                    "type": "tab_created",
                    "tab": {
                        "tab_id": "tab_9",
                        "workspace_id": "ws_1",
                        "label": "Shell"
                    },
                    "root_pane": {
                        "pane_id": "pane_9",
                        "terminal_id": "term_9",
                        "workspace_id": root_workspace_id,
                        "tab_id": root_tab_id
                    }
                }
            })
        };
        assert_eq!(
            parse_tab_created_response(response("tab_existing", "ws_1")).unwrap_err(),
            "tab_created root_pane tab_id does not match tab"
        );
        assert_eq!(
            parse_tab_created_response(response("tab_9", "ws_other")).unwrap_err(),
            "tab_created root_pane workspace_id does not match tab"
        );
    }

    #[test]
    fn pane_process_info_requires_the_created_pane_shell_in_foreground() {
        let response = |foreground_process_group_id: u64, name: &str| {
            serde_json::json!({
                "result": {
                    "type": "pane_process_info",
                    "process_info": {
                        "pane_id": "pane_9",
                        "shell_pid": 42,
                        "foreground_process_group_id": foreground_process_group_id,
                        "foreground_processes": [{
                            "pid": foreground_process_group_id,
                            "name": name,
                            "argv": [name]
                        }]
                    }
                }
            })
        };
        assert!(pane_process_info_shows_shell_initialization(
            &response(42, "pwsh.exe"),
            "pane_9"
        ));
        assert!(!pane_process_info_shows_shell_initialization(
            &response(99, "vim"),
            "pane_9"
        ));
        assert!(!pane_process_info_shows_shell_initialization(
            &response(42, "opencode"),
            "pane_9"
        ));
        assert!(!pane_process_info_shows_shell_initialization(
            &response(42, "pwsh.exe"),
            "pane_other"
        ));
    }

    #[test]
    fn apply_status_json_gates_server_fields() {
        let mut caps = HerdrCapabilities {
            binary_path: Some("/bin/herdr".into()),
            binary_version: None,
            binary_protocol: None,
            channel: None,
            binary_source: HerdrBinarySourceInfo {
                configured: HerdrBinarySource::Global,
                active: HerdrBinarySource::Global,
                resolved: Some(HerdrBinarySource::Global),
                available: true,
                path: Some("/bin/herdr".into()),
                reason: None,
                version: None,
                protocol: None,
                configured_available: true,
                configured_path: Some("/bin/herdr".into()),
                configured_reason: None,
                configured_version: None,
                configured_protocol: None,
                configuration_error: None,
                restart_required: false,
            },
            server: HerdrServerCapability {
                running: false,
                version: None,
                protocol: None,
                compatible: None,
                socket_path: None,
                capabilities: None,
            },
            api: HerdrApiCapability {
                snapshot: false,
                ping: false,
                tab_create: false,
                workspace_focus: false,
                workspace_create: false,
                workspace_rename: false,
                workspace_close: false,
                tab_rename: false,
                tab_close: false,
                tab_focus: false,
                tab_move: false,
                pane_focus: false,
                pane_rename: false,
                pane_split: false,
                pane_zoom: false,
                pane_swap: false,
                pane_close: false,
                layout_export: false,
                layout_set_split_ratio: false,
                agent_manifests: false,
                agent_start: false,
                agent_get: false,
                agent_read: false,
                events_subscribe: false,
                worktree_list: false,
                methods: Vec::new(),
                schema_protocol: None,
                schema_version: None,
                reason: None,
            },
            terminal: HerdrTerminalCapability {
                observe: false,
                control: false,
                takeover: false,
                input: false,
                resize: false,
                scroll: false,
                release: false,
                create: false,
                reason: None,
            },
            events: HerdrEventsCapability {
                status: HerdrEventsStatus::Unavailable,
                reason: None,
            },
        };
        let status = serde_json::json!({
            "client": {
                "version": "0.8.0",
                "channel": "stable",
                "protocol": 19,
                "binary": "/Users/me/.local/bin/herdr"
            },
            "server": {
                "status": "running",
                "running": true,
                "version": "0.8.0",
                "protocol": 19,
                "compatible": true,
                "socket": "/tmp/herdr.sock",
                "capabilities": { "live_handoff": true }
            }
        });
        apply_status_json(&mut caps, &status);
        assert_eq!(caps.binary_protocol, Some(19));
        assert_eq!(caps.binary_version.as_deref(), Some("0.8.0"));
        assert!(caps.server.running);
        assert_eq!(caps.server.socket_path.as_deref(), Some("/tmp/herdr.sock"));
        assert_eq!(caps.server.protocol, Some(19));
    }

    #[test]
    fn events_capability_is_unavailable_when_binary_missing() {
        let mgr = HerdrManager::new();
        // Force missing binary so we don't depend on the host install for this assertion.
        *mgr.binary_override.lock().unwrap() = Some(PathBuf::from("/nonexistent/herdr-binary"));
        let caps = mgr.capabilities();
        assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
        assert!(caps.events.reason.is_some());
        assert!(!caps.binary_source.available);
        assert!(!caps
            .events
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("only supported on unix hosts"));
    }

    #[test]
    fn events_capability_is_available_for_compatible_running_schema() {
        let mut events = HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some("unset".into()),
        };
        apply_events_capability(&mut events, true, true, false);
        assert_eq!(events.status, HerdrEventsStatus::Available);
        assert!(events.reason.is_none());

        apply_events_capability(&mut events, true, false, false);
        assert_eq!(events.status, HerdrEventsStatus::Unavailable);
        assert_eq!(
            events.reason.as_deref(),
            Some("selected herdr schema lacks events.subscribe")
        );

        apply_events_capability(&mut events, false, true, true);
        assert_eq!(events.status, HerdrEventsStatus::Unavailable);
        assert_eq!(
            events.reason.as_deref(),
            Some("herdr session is not running")
        );
    }

    #[test]
    fn api_request_roundtrip_uses_local_stream_transport() {
        use crate::herdr_transport::{bind_local_listener, unique_local_socket_path};
        use interprocess::local_socket::traits::Listener as _;

        let path = unique_local_socket_path("api-roundtrip");
        let listener = bind_local_listener(&path).unwrap();
        let advertised = path.to_string_lossy().into_owned();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            let mut pending = Vec::new();
            let _ = read_local_ndjson_line(
                &mut stream,
                &mut pending,
                Some(Instant::now() + Duration::from_secs(2)),
                MAX_NDJSON_LINE_BYTES,
            );
            write_local_all_until(
                &mut stream,
                b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n",
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
        });
        let value = api_request(&advertised, "ping", serde_json::json!({})).unwrap();
        assert_eq!(value["result"]["protocol"], 19);
        server.join().unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn binary_source_preference_persists_without_hot_swap() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = HerdrManager::new();
        mgr.set_config_dir_for_test(dir.path().to_path_buf());
        assert_eq!(mgr.get_binary_source(), HerdrBinarySource::Global);
        let result = mgr.set_binary_source(HerdrBinarySource::Default).unwrap();
        assert_eq!(result.configured, HerdrBinarySource::Default);
        assert!(result.restart_required);
        // Active process remains on Global until restart/reconfigure.
        assert_eq!(
            *mgr.active_source.lock().unwrap(),
            HerdrBinarySource::Global
        );
        let reloaded = HerdrManager::new();
        reloaded.set_config_dir_for_test(dir.path().to_path_buf());
        assert_eq!(reloaded.get_binary_source(), HerdrBinarySource::Default);
        assert_eq!(
            *reloaded.active_source.lock().unwrap(),
            HerdrBinarySource::Default
        );
    }

    #[test]
    fn corrupt_binary_source_preference_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(binary_source_config_path(dir.path()), "{not-json").unwrap();
        let mgr = HerdrManager::new();
        mgr.set_config_dir_for_test(dir.path().to_path_buf());
        let info = mgr.binary_source_info();
        assert_eq!(info.configured, HerdrBinarySource::Global);
        assert!(info
            .configuration_error
            .as_deref()
            .unwrap_or("")
            .contains("invalid Herdr binary-source preference"));
    }

    #[test]
    fn configured_default_reports_target_unavailable_before_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = HerdrManager::new();
        mgr.set_config_dir_for_test(dir.path().to_path_buf());
        mgr.set_binary_source(HerdrBinarySource::Default).unwrap();
        let info = mgr.binary_source_info();
        assert_eq!(info.active, HerdrBinarySource::Global);
        assert_eq!(info.configured, HerdrBinarySource::Default);
        assert!(info.restart_required);
        assert!(!info.configured_available);
        assert!(info
            .configured_reason
            .as_deref()
            .unwrap_or("")
            .contains("managed"));
    }

    #[test]
    fn missing_global_binary_falls_back_to_yuzora_managed() {
        let managed = PathBuf::from("/bundled/herdr");
        let (path, resolved, reason) = select_binary_resolution(
            HerdrBinarySource::Global,
            false,
            (None, Some("Herdr was not found on PATH".into())),
            (Some(managed.clone()), None),
        );
        assert_eq!(path, Some(managed));
        assert_eq!(resolved, Some(HerdrBinarySource::Default));
        assert!(reason.unwrap().contains("using Yuzora-managed"));
    }

    #[test]
    fn missing_global_and_managed_binaries_report_both_failures() {
        let (path, resolved, reason) = select_binary_resolution(
            HerdrBinarySource::Global,
            false,
            (None, Some("Herdr was not found on PATH".into())),
            (None, Some("managed binary missing".into())),
        );
        assert!(path.is_none());
        assert!(resolved.is_none());
        let reason = reason.unwrap();
        assert!(reason.contains("not found on PATH"), "{reason}");
        assert!(reason.contains("managed binary missing"), "{reason}");
    }

    #[test]
    fn default_binary_source_does_not_silently_fall_back_to_global() {
        let mgr = HerdrManager::new();
        *mgr.active_source.lock().unwrap() = HerdrBinarySource::Default;
        *mgr.configured_source.lock().unwrap() = HerdrBinarySource::Default;
        let (path, reason) = mgr.resolve_binary_for_source(HerdrBinarySource::Default);
        assert!(path.is_none());
        assert!(reason.unwrap().contains("managed"));
    }

    #[test]
    fn managed_binary_override_resolves_only_the_default_source() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("herdr-managed");
        fs::write(&binary, "managed Herdr fixture").unwrap();
        #[cfg(unix)]
        {
            let mut permissions = fs::metadata(&binary).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&binary, permissions).unwrap();
        }
        let mgr = HerdrManager::new();
        mgr.set_managed_binary_override(Some(binary.clone()));
        let (resolved, reason) = mgr.resolve_binary_for_source(HerdrBinarySource::Default);
        assert_eq!(resolved.as_deref(), Some(binary.as_path()));
        assert!(reason.is_none());
    }

    #[test]
    fn parse_agent_get_and_read_response_shapes() {
        let get = parse_agent_get_response(serde_json::json!({
            "result": {
                "type": "agent_info",
                "agent": {
                    "terminal_id": "term-1",
                    "agent_status": "blocked",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "pane_id": "w1:p1",
                    "focused": true,
                    "revision": 3,
                    "cwd": "/tmp/proj",
                    "interactive_ready": true,
                    "state_labels": { "mode": "approval" }
                }
            }
        }))
        .unwrap();
        assert_eq!(get.pane_id, "w1:p1");
        assert_eq!(get.agent_status, "blocked");
        assert_eq!(
            get.state_labels.get("mode").map(String::as_str),
            Some("approval")
        );

        let read = parse_agent_read_response(serde_json::json!({
            "result": {
                "type": "pane_read",
                "read": {
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "source": "recent-unwrapped",
                    "format": "text",
                    "text": "hello",
                    "revision": 9,
                    "truncated": false
                }
            }
        }))
        .unwrap();
        assert_eq!(read.text, "hello");
        assert_eq!(read.source, HerdrReadSource::RecentUnwrapped);
        assert!(!read.too_large);
        assert!(!read.truncated);
    }

    #[test]
    fn agent_manifest_parser_enriches_catalog_and_rejects_duplicates() {
        let catalog = parse_agent_manifest_response(serde_json::json!({
            "result": {
                "type": "agent_manifest_status",
                "manifests": [
                    {
                        "agent": "codex",
                        "source": "bundled",
                        "source_kind": "bundled",
                        "active_version": "2026.07.18.1"
                    },
                    {
                        "agent": "cursor",
                        "source": "bundled",
                        "source_kind": "bundled",
                        "warning": "preview"
                    }
                ]
            }
        }))
        .unwrap();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].agent, "codex");
        assert_eq!(
            catalog[0].bypass_flags,
            vec!["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert_eq!(catalog[1].warning.as_deref(), Some("preview"));
        assert_eq!(agent_binary_name("cursor"), "cursor-agent");

        let duplicate = parse_agent_manifest_response(serde_json::json!({
            "result": {
                "type": "agent_manifest_status",
                "manifests": [
                    { "agent": "pi", "source": "bundled", "source_kind": "bundled" },
                    { "agent": "pi", "source": "local", "source_kind": "local" }
                ]
            }
        }));
        assert!(duplicate.unwrap_err().contains("duplicate"));
        assert!(validate_agent_kind("../codex").is_err());
    }

    #[test]
    fn agent_started_parser_pins_the_created_pane() {
        let response = serde_json::json!({
            "result": {
                "type": "agent_started",
                "agent": { "pane_id": "w1:p1" },
                "argv": ["codex"]
            }
        });
        assert!(parse_agent_started_response(response.clone(), "w1:p1").is_ok());
        assert!(parse_agent_started_response(response, "w1:p2")
            .unwrap_err()
            .contains("requested pane"));
    }

    #[test]
    fn windows_pathext_parser_keeps_safe_unique_extensions() {
        assert_eq!(
            windows_executable_extensions(Some(".EXE;.CMD;.exe;BAT;.;.PS1-evil")),
            vec![".EXE", ".CMD", ".BAT"]
        );
        assert_eq!(
            windows_executable_extensions(None),
            vec![".EXE", ".CMD", ".BAT", ".COM"]
        );
    }

    #[test]
    fn schema_flags_expose_only_advertised_agent_creation_methods() {
        let mut api = HerdrApiCapability {
            snapshot: false,
            ping: false,
            tab_create: false,
            workspace_focus: false,
            workspace_create: false,
            workspace_rename: false,
            workspace_close: false,
            tab_rename: false,
            tab_close: false,
            tab_focus: false,
            tab_move: false,
            pane_focus: false,
            pane_rename: false,
            pane_split: false,
            pane_zoom: false,
            pane_swap: false,
            pane_close: false,
            layout_export: false,
            layout_set_split_ratio: false,
            agent_manifests: false,
            agent_start: false,
            agent_get: false,
            agent_read: false,
            events_subscribe: false,
            worktree_list: false,
            methods: Vec::new(),
            schema_protocol: None,
            schema_version: None,
            reason: None,
        };
        let methods = HashSet::from([
            "session.snapshot".to_string(),
            "tab.create".to_string(),
            "server.agent_manifests".to_string(),
            "agent.start".to_string(),
        ]);
        apply_schema_method_flags(&mut api, &methods, false);
        assert!(api.agent_manifests);
        assert!(api.agent_start);
        assert!(api.methods.contains(&"server.agent_manifests".to_string()));
        assert!(api.methods.contains(&"agent.start".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn agent_create_uses_manifest_tab_and_start_as_one_bounded_transaction() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent-create.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            for _ in 0..7 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(request.trim()).unwrap();
                let method = request["method"].as_str().unwrap().to_string();
                methods.push(method.clone());
                let response = match method.as_str() {
                    "ping" => serde_json::json!({
                        "id": request["id"],
                        "result": { "type": "pong", "version": "0.8.0", "protocol": 19 }
                    }),
                    "server.agent_manifests" => serde_json::json!({
                        "id": request["id"],
                        "result": {
                            "type": "agent_manifest_status",
                            "manifests": [
                                { "agent": "codex", "source": "bundled", "source_kind": "bundled" }
                            ]
                        }
                    }),
                    "tab.create" => serde_json::json!({
                        "id": request["id"],
                        "result": {
                            "type": "tab_created",
                            "tab": {
                                "tab_id": "w1:t2",
                                "workspace_id": "w1",
                                "label": "codex"
                            },
                            "root_pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-2",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                    "agent.start" => serde_json::json!({
                        "id": request["id"],
                        "result": {
                            "type": "agent_started",
                            "agent": { "pane_id": "w1:p2" },
                            "argv": ["codex"]
                        }
                    }),
                    other => panic!("unexpected method: {other}"),
                };
                writeln!(stream, "{response}").unwrap();
            }
            methods
        });

        let binary = dir.path().join("herdr");
        let script = format!(
            r#"#!/bin/sh
set -e
if [ "$1" = "session" ]; then
  printf '%s\n' '{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"{}"}}]}}'
  exit 0
fi
if [ "$1" = "status" ]; then
  printf '%s\n' '{{"client":{{"version":"0.8.0","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"{}"}}}}'
  exit 0
fi
printf '%s\n' '{{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","tab.close","server.agent_manifests","agent.start"]}}'
"#,
            socket.display(),
            socket.display()
        );
        fs::write(&binary, script).unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();

        let manager = HerdrManager::with_binary(binary);
        let created = manager
            .agent_create(Some("default"), "w1".into(), "codex".into(), false)
            .unwrap();
        assert_eq!(created.name, "codex");
        assert_eq!(created.pane_id, "w1:p2");
        assert_eq!(created.terminal_id, "term-2");
        assert_eq!(
            server.join().unwrap(),
            vec![
                "ping",
                "ping",
                "server.agent_manifests",
                "ping",
                "tab.create",
                "ping",
                "agent.start"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn agent_create_closes_only_the_new_tab_when_agent_start_fails() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent-create-rollback.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let mut methods = Vec::new();
            let mut closed_tab_id = None;
            for _ in 0..9 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(request.trim()).unwrap();
                let method = request["method"].as_str().unwrap().to_string();
                methods.push(method.clone());
                let response = match method.as_str() {
                    "ping" => serde_json::json!({
                        "id": request["id"],
                        "result": { "type": "pong", "version": "0.8.0", "protocol": 19 }
                    }),
                    "server.agent_manifests" => serde_json::json!({
                        "id": request["id"],
                        "result": {
                            "type": "agent_manifest_status",
                            "manifests": [
                                { "agent": "codex", "source": "bundled", "source_kind": "bundled" }
                            ]
                        }
                    }),
                    "tab.create" => serde_json::json!({
                        "id": request["id"],
                        "result": {
                            "type": "tab_created",
                            "tab": {
                                "tab_id": "w1:t2",
                                "workspace_id": "w1",
                                "label": "codex"
                            },
                            "root_pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-2",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                    "agent.start" => serde_json::json!({
                        "id": request["id"],
                        "error": {
                            "code": "agent_start_failed",
                            "message": "codex exited before startup"
                        }
                    }),
                    "tab.close" => {
                        closed_tab_id = request["params"]["tab_id"].as_str().map(ToOwned::to_owned);
                        serde_json::json!({
                            "id": request["id"],
                            "result": { "type": "tab_closed" }
                        })
                    }
                    other => panic!("unexpected method: {other}"),
                };
                writeln!(stream, "{response}").unwrap();
            }
            (methods, closed_tab_id)
        });

        let status = format!(
            r#"{{"client":{{"version":"0.8.0","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"{}"}}}}"#,
            socket.display()
        );
        let sessions = format!(
            r#"{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"{}"}}]}}"#,
            socket.display()
        );
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            &status,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","tab.close","server.agent_manifests","agent.start"]}"#,
            &sessions,
        );

        let manager = HerdrManager::with_binary(binary);
        let error = manager
            .agent_create(Some("default"), "w1".into(), "codex".into(), false)
            .unwrap_err();
        assert_eq!(error, "agent_start_failed: codex exited before startup");
        let (methods, closed_tab_id) = server.join().unwrap();
        assert_eq!(closed_tab_id.as_deref(), Some("w1:t2"));
        assert_eq!(
            methods,
            vec![
                "ping",
                "ping",
                "server.agent_manifests",
                "ping",
                "tab.create",
                "ping",
                "agent.start",
                "ping",
                "tab.close"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn agent_create_retries_busy_only_for_the_pinned_initializing_shell_and_one_name_collision() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent-create-retry.sock");
        let pong = || {
            serde_json::json!({
                "result": { "type": "pong", "version": "0.8.0", "protocol": 19 }
            })
        };
        let server = spawn_scripted_api_server(
            &socket,
            vec![
                ("ping", pong()),
                ("ping", pong()),
                (
                    "server.agent_manifests",
                    serde_json::json!({
                        "result": {
                            "type": "agent_manifest_status",
                            "manifests": [
                                { "agent": "codex", "source": "bundled", "source_kind": "bundled" }
                            ]
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "tab.create",
                    serde_json::json!({
                        "result": {
                            "type": "tab_created",
                            "tab": { "tab_id": "w1:t2", "workspace_id": "w1", "label": "codex" },
                            "root_pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-2",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "agent.start",
                    serde_json::json!({
                        "error": { "code": "agent_pane_busy", "message": "shell is initializing" }
                    }),
                ),
                ("ping", pong()),
                (
                    "pane.get",
                    serde_json::json!({
                        "result": {
                            "type": "pane_info",
                            "pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-2",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "pane.process_info",
                    serde_json::json!({
                        "result": {
                            "type": "pane_process_info",
                            "process_info": {
                                "pane_id": "w1:p2",
                                "shell_pid": 42,
                                "foreground_process_group_id": 42,
                                "foreground_processes": [{
                                    "pid": 42,
                                    "name": "pwsh.exe",
                                    "argv": ["C:\\Program Files\\PowerShell\\7\\pwsh.exe"]
                                }]
                            }
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "agent.start",
                    serde_json::json!({
                        "error": { "code": "agent_name_taken", "message": "name already exists" }
                    }),
                ),
                ("ping", pong()),
                (
                    "agent.start",
                    serde_json::json!({
                        "result": {
                            "type": "agent_started",
                            "agent": { "pane_id": "w1:p2" },
                            "argv": ["codex"]
                        }
                    }),
                ),
            ],
        );
        let binary = write_agent_test_binary(dir.path(), &socket, true);

        let manager = HerdrManager::with_binary(binary);
        let created = manager
            .agent_create(Some("default"), "w1".into(), "codex".into(), false)
            .unwrap();
        assert!(created.name.starts_with("codex-"));
        let requests = server.join().unwrap();
        let start_names = requests
            .iter()
            .filter(|request| request["method"] == "agent.start")
            .map(|request| request["params"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(start_names, vec!["codex", "codex", created.name.as_str()]);
        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "ping",
                "ping",
                "server.agent_manifests",
                "ping",
                "tab.create",
                "ping",
                "agent.start",
                "ping",
                "pane.get",
                "ping",
                "pane.process_info",
                "ping",
                "agent.start",
                "ping",
                "agent.start"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn agent_create_busy_fails_closed_when_the_created_terminal_identity_changes() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent-create-replaced-terminal.sock");
        let pong = || {
            serde_json::json!({
                "result": { "type": "pong", "version": "0.8.0", "protocol": 19 }
            })
        };
        let server = spawn_scripted_api_server(
            &socket,
            vec![
                ("ping", pong()),
                ("ping", pong()),
                (
                    "server.agent_manifests",
                    serde_json::json!({
                        "result": {
                            "type": "agent_manifest_status",
                            "manifests": [
                                { "agent": "codex", "source": "bundled", "source_kind": "bundled" }
                            ]
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "tab.create",
                    serde_json::json!({
                        "result": {
                            "type": "tab_created",
                            "tab": { "tab_id": "w1:t2", "workspace_id": "w1", "label": "codex" },
                            "root_pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-2",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "agent.start",
                    serde_json::json!({
                        "error": { "code": "agent_pane_busy", "message": "pane is occupied" }
                    }),
                ),
                ("ping", pong()),
                (
                    "pane.get",
                    serde_json::json!({
                        "result": {
                            "type": "pane_info",
                            "pane": {
                                "pane_id": "w1:p2",
                                "terminal_id": "term-replaced",
                                "tab_id": "w1:t2",
                                "workspace_id": "w1"
                            }
                        }
                    }),
                ),
                ("ping", pong()),
                (
                    "tab.close",
                    serde_json::json!({
                        "result": { "type": "tab_closed" }
                    }),
                ),
            ],
        );
        let binary = write_agent_test_binary(dir.path(), &socket, true);

        let manager = HerdrManager::with_binary(binary);
        let error = manager
            .agent_create(Some("default"), "w1".into(), "codex".into(), false)
            .unwrap_err();
        assert_eq!(error, "agent_pane_busy: pane is occupied");
        let requests = server.join().unwrap();
        assert!(!requests
            .iter()
            .any(|request| request["method"] == "pane.process_info"));
        let close = requests
            .iter()
            .find(|request| request["method"] == "tab.close")
            .unwrap();
        assert_eq!(close["params"]["tab_id"], "w1:t2");
    }

    #[test]
    fn shared_ndjson_helper_rejects_hostile_frames_before_parse() {
        use crate::herdr_limits::{
            read_bounded_ndjson_line, BoundedNdjsonReadError, HerdrProtocolError,
            MAX_NDJSON_LINE_BYTES,
        };
        use std::io::Cursor;

        let mut output = String::new();
        let oversized = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        match read_bounded_ndjson_line(&mut Cursor::new(oversized), &mut output) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::UnterminatedOverLimit)) => {}
            other => panic!("expected unterminated over-limit, got {other:?}"),
        }

        let mut terminated = vec![b'x'; MAX_NDJSON_LINE_BYTES + 1];
        terminated.push(b'\n');
        match read_bounded_ndjson_line(&mut Cursor::new(terminated), &mut output) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::LineTooLarge)) => {}
            other => panic!("expected line too large, got {other:?}"),
        }

        match read_bounded_ndjson_line(&mut Cursor::new([0xff, 0xfe, b'\n']), &mut output) {
            Err(BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8)) => {}
            other => panic!("expected invalid UTF-8, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_read_caps_text_and_sets_too_large() {
        use crate::herdr_limits::MAX_AGENT_TEXT_BYTES;
        let over = "a".repeat(MAX_AGENT_TEXT_BYTES + 1);
        let read = parse_agent_read_response(serde_json::json!({
            "result": {
                "type": "pane_read",
                "read": {
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "source": "recent",
                    "format": "text",
                    "text": over,
                    "revision": 1,
                    "truncated": false
                }
            }
        }))
        .unwrap();
        assert_eq!(read.text.len(), MAX_AGENT_TEXT_BYTES);
        assert!(read.too_large);
        assert!(read.truncated);

        let exact = "b".repeat(MAX_AGENT_TEXT_BYTES);
        let read = parse_agent_read_response(serde_json::json!({
            "result": {
                "type": "pane_read",
                "read": {
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "source": "recent",
                    "format": "text",
                    "text": exact,
                    "revision": 1,
                    "truncated": false
                }
            }
        }))
        .unwrap();
        assert_eq!(read.text.len(), MAX_AGENT_TEXT_BYTES);
        assert!(!read.too_large);
        assert!(!read.truncated);
    }

    #[test]
    fn parse_snapshot_rejects_excessive_pane_array() {
        use crate::herdr_limits::MAX_PANE_COUNT;
        let mut panes = Vec::with_capacity(MAX_PANE_COUNT + 1);
        for index in 0..=MAX_PANE_COUNT {
            panes.push(serde_json::json!({ "pane_id": format!("p{index}") }));
        }
        let error = parse_snapshot_response(serde_json::json!({
            "result": {
                "type": "session_snapshot",
                "snapshot": {
                    "protocol": 19,
                    "version": "0.8.0",
                    "panes": panes
                }
            }
        }))
        .unwrap_err();
        assert!(error.contains("tooComplex"), "{error}");
        assert!(error.contains("pane"), "{error}");
    }

    #[test]
    fn parse_snapshot_accepts_pane_count_at_limit() {
        use crate::herdr_limits::MAX_PANE_COUNT;
        let panes = vec![serde_json::json!({ "pane_id": "p" }); MAX_PANE_COUNT];
        let parsed = parse_snapshot_response(serde_json::json!({
            "result": {
                "type": "session_snapshot",
                "snapshot": {
                    "protocol": 19,
                    "version": "0.8.0",
                    "panes": panes
                }
            }
        }))
        .unwrap();
        assert_eq!(parsed.protocol, 19);
        assert_eq!(
            parsed.snapshot["panes"].as_array().unwrap().len(),
            MAX_PANE_COUNT
        );
    }

    #[test]
    fn agent_response_parsers_reject_missing_identity_and_unknown_enums() {
        let missing_pane = parse_agent_get_response(serde_json::json!({
            "result": {
                "type": "agent_info",
                "agent": {
                    "terminal_id": "term-1",
                    "agent_status": "blocked",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "focused": false,
                    "revision": 1
                }
            }
        }));
        assert!(missing_pane.is_err());

        let unknown_source = parse_agent_read_response(serde_json::json!({
            "result": {
                "type": "pane_read",
                "read": {
                    "pane_id": "w1:p1",
                    "workspace_id": "w1",
                    "tab_id": "w1:t1",
                    "source": "invented",
                    "format": "text",
                    "text": "hello",
                    "revision": 1,
                    "truncated": false
                }
            }
        }));
        assert!(unknown_source.is_err());
        assert!(validate_explicit_pane_target("agent-name").is_err());
        assert!(validate_explicit_pane_target("w1:p1").is_ok());
    }

    #[test]
    fn agent_read_rejects_out_of_range_line_count_before_ipc() {
        let mgr = HerdrManager::new();
        let error = mgr
            .agent_read(
                None,
                "w1:p1".into(),
                HerdrReadSource::Recent,
                Some(HerdrReadFormat::Text),
                Some(501),
                Some(true),
            )
            .unwrap_err();
        assert!(error.contains("between 20 and 500"));
    }

    #[test]
    fn parse_subscription_event_line_reads_pane_exited() {
        let event = parse_subscription_event_line(
            "sub-exit",
            r#"{"event":"pane.exited","data":{"pane_id":"w1:p2","workspace_id":"w1"}}"#,
        )
        .unwrap()
        .unwrap();
        match event {
            HerdrSubscriptionEvent::PaneExited {
                subscription_id,
                pane_id,
                workspace_id,
            } => {
                assert_eq!(subscription_id, "sub-exit");
                assert_eq!(pane_id, "w1:p2");
                assert_eq!(workspace_id, "w1");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_subscription_event_line_reads_tab_topology() {
        let created = parse_subscription_event_line(
            "sub-tab",
            r#"{"event":"tab.created","data":{"tab":{"tab_id":"t2","workspace_id":"w1"}}}"#,
        )
        .unwrap()
        .unwrap();
        match created {
            HerdrSubscriptionEvent::TopologyChanged {
                kind,
                workspace_id,
                tab_id,
                ..
            } => {
                assert_eq!(kind, "tab.created");
                assert_eq!(workspace_id.as_deref(), Some("w1"));
                assert_eq!(tab_id.as_deref(), Some("t2"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let moved = parse_subscription_event_line(
            "sub-tab",
            r#"{"event":"tab.moved","data":{"tab_id":"t2","workspace_id":"w1","insert_index":1}}"#,
        )
        .unwrap()
        .unwrap();
        match moved {
            HerdrSubscriptionEvent::TopologyChanged { kind, .. } => {
                assert_eq!(kind, "tab.moved");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let closed = parse_subscription_event_line(
            "sub-tab",
            r#"{"event":"tab.closed","data":{"tab_id":"t2","workspace_id":"w1"}}"#,
        )
        .unwrap()
        .unwrap();
        match closed {
            HerdrSubscriptionEvent::TopologyChanged {
                kind,
                workspace_id,
                tab_id,
                ..
            } => {
                assert_eq!(kind, "tab.closed");
                assert_eq!(workspace_id.as_deref(), Some("w1"));
                assert_eq!(tab_id.as_deref(), Some("t2"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_subscription_event_line_reads_workspace_topology() {
        for (line, expected) in [
            (
                r#"{"event":"workspace.created","data":{"workspace":{"workspace_id":"w2"}}}"#,
                "workspace.created",
            ),
            (
                r#"{"event":"workspace.closed","data":{"workspace_id":"w2"}}"#,
                "workspace.closed",
            ),
            (
                r#"{"event":"workspace.moved","data":{"workspace_id":"w2","insert_index":1}}"#,
                "workspace.moved",
            ),
            (
                r#"{"event":"workspace.reordered","data":{"workspace_ids":["w1","w2"]}}"#,
                "workspace.reordered",
            ),
        ] {
            let event = parse_subscription_event_line("sub-ws", line)
                .unwrap()
                .unwrap();
            match event {
                HerdrSubscriptionEvent::TopologyChanged { kind, .. } => {
                    assert_eq!(kind, expected);
                }
                other => panic!("unexpected event for {expected}: {other:?}"),
            }
        }
    }

    #[test]
    fn parse_subscription_event_line_reads_agent_status_changed() {
        let event = parse_subscription_event_line(
            "sub-1",
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"done","title":"Review","execution_origin":{"kind":"wsl","distribution":"Ubuntu"}}}"#,
        )
        .unwrap()
        .unwrap();
        match event {
            HerdrSubscriptionEvent::AgentStatusChanged {
                pane_id,
                agent_status,
                title,
                execution_origin,
                ..
            } => {
                assert_eq!(pane_id, "w1:p1");
                assert_eq!(agent_status, "done");
                assert_eq!(title.as_deref(), Some("Review"));
                assert_eq!(
                    execution_origin,
                    Some(serde_json::json!({"kind": "wsl", "distribution": "Ubuntu"}))
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn subscription_event_serializes_agent_status_changed_with_camel_case_fields() {
        let event = HerdrSubscriptionEvent::AgentStatusChanged {
            subscription_id: "sub-1".to_owned(),
            pane_id: "w1:p1".to_owned(),
            workspace_id: "w1".to_owned(),
            agent_status: "working".to_owned(),
            agent: Some("pi".to_owned()),
            display_agent: Some("Pi".to_owned()),
            title: Some("Review".to_owned()),
            execution_origin: Some(serde_json::json!({
                "kind": "wsl",
                "distribution": "Ubuntu"
            })),
            state_labels: HashMap::from([("working".to_owned(), "Working".to_owned())]),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "agent_status_changed",
                "subscriptionId": "sub-1",
                "paneId": "w1:p1",
                "workspaceId": "w1",
                "agentStatus": "working",
                "agent": "pi",
                "displayAgent": "Pi",
                "title": "Review",
                "executionOrigin": { "kind": "wsl", "distribution": "Ubuntu" },
                "stateLabels": { "working": "Working" }
            })
        );
    }

    #[test]
    fn collect_schema_methods_reads_methods_array_and_request_union() {
        let schema = serde_json::json!({
            "protocol": 19,
            "schema_version": 1,
            "methods": ["session.snapshot", "tab.create", "session.ping"],
            "schemas": {
                "request": {
                    "oneOf": [
                        { "properties": { "method": { "const": "session.snapshot" } } },
                        { "properties": { "method": { "const": "tab.create" } } }
                    ]
                }
            }
        });
        let methods = collect_schema_methods(&schema);
        assert!(methods.contains("session.snapshot"));
        assert!(methods.contains("tab.create"));
        assert!(methods.contains("session.ping"));
    }

    #[test]
    fn collect_schema_methods_empty_schemas_reports_no_methods() {
        let schema = serde_json::json!({
            "protocol": 19,
            "schema_version": 1,
            "schemas": {}
        });
        assert!(collect_schema_methods(&schema).is_empty());
    }

    #[cfg(unix)]
    fn write_fake_herdr_with(dir: &Path, status_json: &str, schema_json: &str) -> PathBuf {
        write_fake_herdr_with_sessions(
            dir,
            status_json,
            schema_json,
            r#"{"sessions":[{"name":"default","default":true,"running":false,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}"#,
        )
    }

    #[cfg(unix)]
    fn write_fake_herdr_with_sessions(
        dir: &Path,
        status_json: &str,
        schema_json: &str,
        sessions_json: &str,
    ) -> PathBuf {
        let path = dir.join("herdr");
        let script = format!(
            r#"#!/bin/sh
set -e
if [ "$1" = "session" ] && [ "$2" = "list" ] && [ "$3" = "--json" ]; then
  cat <<'JSON'
{sessions}
JSON
  exit 0
fi
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  cat <<'JSON'
{status}
JSON
  exit 0
fi
if [ "$1" = "api" ] && [ "$2" = "schema" ] && [ "$3" = "--json" ]; then
  cat <<'JSON'
{schema}
JSON
  exit 0
fi
if [ "$1" = "terminal" ] && [ "$2" = "session" ]; then
  mode="$3"
  # Echo HERDR_SESSION to a side channel file when present (tests inspect env).
  if [ -n "${{HERDR_SESSION:-}}" ] && [ -n "${{HERDR_TEST_ENV_FILE:-}}" ]; then
    printf '%s\n' "$HERDR_SESSION" > "$HERDR_TEST_ENV_FILE"
  fi
  printf '%s\n' '{{"type":"terminal.frame","seq":1,"full":true,"encoding":"ansi","width":40,"height":10,"bytes":"AAA="}}'
  if [ "$mode" = "control" ]; then
    while IFS= read -r line; do
      case "$line" in
        *terminal.release*)
          printf '%s\n' '{{"type":"terminal.closed","reason":"detached"}}'
          exit 0
          ;;
        *terminal.resize*)
          printf '%s\n' '{{"type":"terminal.frame","seq":2,"full":false,"encoding":"ansi","width":40,"height":10,"bytes":"AQE="}}'
          ;;
      esac
    done
  else
    sleep 2
  fi
  exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
            sessions = sessions_json,
            status = status_json,
            schema = schema_json,
        );
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_fake_herdr_startup(dir: &Path) -> PathBuf {
        let path = dir.join("herdr");
        fs::write(
            &path,
            r#"#!/bin/sh
set -e
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ready="$base/server.ready"
invoked="$base/server.invoked"
if [ "$1" = "status" ] && [ "$2" = "--json" ]; then
  if [ -f "$ready" ]; then
    printf '%s\n' '{"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"}}'
  else
    printf '%s\n' '{"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null}}'
  fi
  exit 0
fi
if [ "$1" = "server" ] && [ -z "${2:-}" ]; then
  : > "$invoked"
  : > "$ready"
  exit 0
fi
echo "unexpected args: $*" >&2
exit 2
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn startup_launches_resolved_headless_server_and_waits_until_ready() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_startup(dir.path());
        let manager = HerdrManager::with_binary(binary);

        let started = manager.ensure_server_running_on_startup().unwrap();

        assert!(started, "a stopped resolved Herdr server must be launched");
        assert!(
            dir.path().join("server.invoked").is_file(),
            "startup must invoke the resolved binary as `herdr server`"
        );
        assert!(
            dir.path().join("server.ready").is_file(),
            "startup must not return before Herdr reports running"
        );
    }

    #[test]
    #[cfg(unix)]
    fn startup_keeps_an_existing_herdr_server() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_startup(dir.path());
        fs::write(dir.path().join("server.ready"), "").unwrap();
        let manager = HerdrManager::with_binary(binary);

        let started = manager.ensure_server_running_on_startup().unwrap();

        assert!(!started, "an already-running Herdr server must be reused");
        assert!(!dir.path().join("server.invoked").exists());
    }

    #[cfg(unix)]
    fn write_agent_test_binary(dir: &Path, socket: &Path, probe_methods: bool) -> PathBuf {
        let socket_path = socket.to_string_lossy();
        let status = serde_json::json!({
            "client": { "version": "0.8.0", "protocol": 19, "binary": "FAKE" },
            "server": {
                "status": "running",
                "running": true,
                "version": "0.8.0",
                "protocol": 19,
                "compatible": true,
                "socket": socket_path
            }
        })
        .to_string();
        let mut methods = vec![
            "session.snapshot",
            "tab.create",
            "tab.close",
            "server.agent_manifests",
            "agent.start",
        ];
        if probe_methods {
            methods.extend(["pane.get", "pane.process_info"]);
        }
        let schema = serde_json::json!({
            "protocol": 19,
            "schema_version": 1,
            "methods": methods
        })
        .to_string();
        let sessions = serde_json::json!({
            "sessions": [{
                "name": "default",
                "default": true,
                "running": true,
                "session_dir": "/tmp/default",
                "socket_path": socket_path
            }]
        })
        .to_string();
        write_fake_herdr_with_sessions(dir, &status, &schema, &sessions)
    }

    #[cfg(unix)]
    fn spawn_scripted_api_server(
        socket: &Path,
        script: Vec<(&'static str, serde_json::Value)>,
    ) -> std::thread::JoinHandle<Vec<serde_json::Value>> {
        use std::os::unix::net::UnixListener;

        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let mut requests = Vec::with_capacity(script.len());
            for (expected_method, mut response) in script {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let request: serde_json::Value = serde_json::from_str(request.trim()).unwrap();
                assert_eq!(request["method"].as_str(), Some(expected_method));
                response
                    .as_object_mut()
                    .unwrap()
                    .insert("id".into(), request["id"].clone());
                writeln!(stream, "{response}").unwrap();
                requests.push(request);
            }
            requests
        })
    }

    #[cfg(unix)]
    fn write_fake_herdr(dir: &Path) -> PathBuf {
        write_fake_herdr_with(
            dir,
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"schemas":{}}"#,
        )
    }

    #[cfg(unix)]
    fn write_fake_herdr_running_session(dir: &Path) -> PathBuf {
        write_fake_herdr_with_sessions(
            dir,
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr.sock"}]}"#,
        )
    }

    #[cfg(unix)]
    fn write_fake_herdr_event_session(dir: &Path, socket: &Path) -> PathBuf {
        let socket = socket.to_string_lossy();
        let status = serde_json::json!({
            "client": {
                "version": "0.0.0-fake",
                "channel": "test",
                "protocol": 19,
                "binary": "FAKE"
            },
            "server": {
                "status": "running",
                "running": true,
                "version": "0.0.0-fake",
                "protocol": 19,
                "compatible": true,
                "socket": socket
            }
        })
        .to_string();
        let schema = serde_json::json!({
            "protocol": 19,
            "schema_version": 1,
            "methods": ["session.snapshot", "tab.create", "events.subscribe"]
        })
        .to_string();
        let sessions = serde_json::json!({
            "sessions": [{
                "name": "default",
                "default": true,
                "running": true,
                "session_dir": "/tmp/herdr-default",
                "socket_path": socket
            }]
        })
        .to_string();
        write_fake_herdr_with_sessions(dir, &status, &schema, &sessions)
    }

    #[cfg(unix)]
    #[test]
    fn event_subscription_release_interrupts_idle_socket_reader() {
        use std::io::Read;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("events.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream
                .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
                .unwrap();
            let mut remaining = String::new();
            let _ = BufReader::new(stream).read_to_string(&mut remaining);
            request
        });
        let binary = write_fake_herdr_event_session(dir.path(), &socket);
        let manager = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel();
        let callback: OnSubscriptionEvent =
            Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
        let id = manager
            .events_subscribe(Some("default".into()), callback)
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Subscribed { .. }
        ));
        let started = Instant::now();
        manager.events_release(&id).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        let request: serde_json::Value =
            serde_json::from_str(server.join().unwrap().trim()).unwrap();
        assert_eq!(request["method"], "events.subscribe");
        let selectors = request["params"]["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["type"].as_str())
            .collect::<Vec<_>>();
        assert!(selectors.contains(&"tab.closed"));
        assert!(selectors.contains(&"workspace.moved"));
        assert!(selectors.contains(&"workspace.reordered"));
    }

    #[cfg(unix)]
    #[test]
    fn event_subscription_ack_is_bounded() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("events-ack.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            std::thread::sleep(Duration::from_millis(1_200));
        });
        let binary = write_fake_herdr_event_session(dir.path(), &socket);
        let manager = Arc::new(HerdrManager::with_binary(binary));
        let callback: OnSubscriptionEvent = Arc::new(|_| Ok(()));
        let started = Instant::now();
        let error = manager
            .events_subscribe(Some("default".into()), callback)
            .unwrap_err();
        assert!(error.contains("ack read failed"));
        assert!(started.elapsed() < Duration::from_secs(5));
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn api_request_rejects_oversized_unterminated_and_invalid_utf8() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("api-hostile.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            // Over-limit unterminated payload, no newline.
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let _ = stream.write_all(&vec![b'x'; crate::herdr_limits::MAX_NDJSON_LINE_BYTES + 2]);

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let _ = stream.write_all(&[0xff, 0xfe, b'\n']);
        });

        let error =
            api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
        assert!(error.contains("tooLarge"), "{error}");
        assert!(
            error.contains("unterminated") || error.contains("1 MiB"),
            "{error}"
        );

        let error =
            api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
        assert!(error.contains("invalidUtf8"), "{error}");
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn api_request_accepts_line_at_byte_cap_and_rejects_one_byte_over() {
        use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
        use std::os::unix::net::UnixListener;

        let prefix =
            b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19,\"pad\":\"";
        let suffix = b"\"}}";
        let pad = MAX_NDJSON_LINE_BYTES - prefix.len() - suffix.len();
        let mut at_cap = Vec::with_capacity(MAX_NDJSON_LINE_BYTES + 1);
        at_cap.extend_from_slice(prefix);
        at_cap.extend(std::iter::repeat(b'a').take(pad));
        at_cap.extend_from_slice(suffix);
        assert_eq!(at_cap.len(), MAX_NDJSON_LINE_BYTES);
        at_cap.push(b'\n');

        let mut over = at_cap.clone();
        over.insert(over.len() - 1, b'b');

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("api-cap.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream.write_all(&at_cap).unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream.write_all(&over).unwrap();
        });

        let ok = api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap();
        assert_eq!(ok["result"]["protocol"], 19);

        let error =
            api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
        assert!(error.contains("tooLarge"), "{error}");
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn api_request_rejects_empty_response_and_excessive_depth() {
        use crate::herdr_limits::MAX_JSON_DEPTH;
        use std::os::unix::net::UnixListener;

        let mut deep = serde_json::json!(1);
        for _ in 0..=MAX_JSON_DEPTH {
            deep = serde_json::json!([deep]);
        }
        let hostile = serde_json::json!({ "result": deep });
        let mut deep_line = serde_json::to_vec(&hostile).unwrap();
        deep_line.push(b'\n');

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("api-empty.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream.write_all(b"\n").unwrap();

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream.write_all(&deep_line).unwrap();
        });

        let empty =
            api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
        assert!(empty.contains("emptyResponse"), "{empty}");

        let deep_err =
            api_request(socket.to_str().unwrap(), "ping", serde_json::json!({})).unwrap_err();
        assert!(deep_err.contains("tooComplex"), "{deep_err}");
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn event_subscription_oversized_line_is_terminal() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("events-oversize.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream
                .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
                .unwrap();
            let _ = stream.write_all(&vec![b'x'; crate::herdr_limits::MAX_NDJSON_LINE_BYTES + 2]);
            std::thread::sleep(Duration::from_millis(200));
        });
        let binary = write_fake_herdr_event_session(dir.path(), &socket);
        let manager = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel();
        let callback: OnSubscriptionEvent =
            Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
        let id = manager
            .events_subscribe(Some("default".into()), callback)
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Subscribed { .. }
        ));
        match rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap() {
            HerdrSubscriptionEvent::Error { message, .. } => {
                assert!(message.contains("tooLarge"), "{message}");
            }
            other => panic!("expected oversized error, got {other:?}"),
        }
        manager.events_release(&id).unwrap();
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn malformed_event_is_terminal_and_releasable() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("events-malformed.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream
                .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n{not-json}\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        let binary = write_fake_herdr_event_session(dir.path(), &socket);
        let manager = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel();
        let callback: OnSubscriptionEvent =
            Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
        let id = manager
            .events_subscribe(Some("default".into()), callback)
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Subscribed { .. }
        ));
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Error { .. }
        ));
        manager.events_release(&id).unwrap();
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn event_subscription_delivers_duplicate_events_then_disconnects() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("events-stream.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            let event = b"{\"event\":\"pane.agent_status_changed\",\"data\":{\"pane_id\":\"w1:p1\",\"workspace_id\":\"w1\",\"agent_status\":\"done\"}}\n";
            stream
                .write_all(b"{\"result\":{\"type\":\"subscription_started\"}}\n")
                .unwrap();
            stream.write_all(event).unwrap();
            stream.write_all(event).unwrap();
        });
        let binary = write_fake_herdr_event_session(dir.path(), &socket);
        let manager = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel();
        let callback: OnSubscriptionEvent =
            Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
        let id = manager
            .events_subscribe(Some("default".into()), callback)
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Subscribed { .. }
        ));
        for _ in 0..2 {
            assert!(matches!(
                rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
                HerdrSubscriptionEvent::AgentStatusChanged { .. }
            ));
        }
        assert!(matches!(
            rx.recv_timeout(TEST_EVENT_RECV_TIMEOUT).unwrap(),
            HerdrSubscriptionEvent::Disconnected { .. }
        ));
        manager.events_release(&id).unwrap();
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fake_binary_capabilities_use_discovered_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr(dir.path());
        let mgr = HerdrManager::with_binary(binary.clone());
        let caps = mgr.capabilities();
        assert_eq!(caps.binary_protocol, Some(19));
        assert_eq!(caps.api.schema_protocol, Some(19));
        assert_eq!(caps.binary_version.as_deref(), Some("0.0.0-fake"));
        assert!(!caps.terminal.observe); // named session stopped in fixture
        assert!(!caps.terminal.control);
        assert!(!caps.api.snapshot); // server not running in fixture
        assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
        // status overwrites binary path from fixture client.binary
        assert_eq!(caps.binary_path.as_deref(), Some("FAKE"));
        let _ = binary;
    }

    #[cfg(unix)]
    #[test]
    fn mutation_capability_cache_avoids_reprobing_status_and_schema() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("cache-ping.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                stream
                    .write_all(b"{\"id\":\"cache\",\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n")
                    .unwrap();
            }
        });
        let count_file = dir.path().join("probe-count.txt");
        let binary = dir.path().join("herdr");
        let script = format!(
            r#"#!/bin/sh
set -e
if [ "$1" = "session" ]; then
  printf '%s\n' '{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"{}"}}]}}'
  exit 0
fi
printf '%s\n' x >> '{}'
if [ "$1" = "status" ]; then
  printf '%s\n' '{{"client":{{"version":"0.8.0","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"/tmp/default.sock"}}}}'
  exit 0
fi
printf '%s\n' '{{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus"]}}'
"#,
            socket.display(),
            count_file.display()
        );
        fs::write(&binary, script).unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();
        let mgr = HerdrManager::with_binary(binary);

        assert!(
            mgr.capabilities_for_session(Some("default"))
                .api
                .workspace_focus
        );
        let after_first = fs::read_to_string(&count_file).unwrap().lines().count();
        assert!(
            mgr.cached_capabilities_for_session(Some("default"))
                .api
                .workspace_focus
        );
        let after_second = fs::read_to_string(&count_file).unwrap().lines().count();

        assert_eq!(after_first, after_second);
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn capabilities_do_not_claim_api_when_server_incompatible() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":18,"compatible":false,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.capabilities();
        assert_eq!(caps.server.compatible, Some(false));
        assert!(!caps.api.snapshot);
        assert!(!caps.api.tab_create);
        assert!(!caps.api.ping);
        assert!(!caps.terminal.control);
        assert!(!caps.terminal.create);
        assert!(!caps.terminal.observe);
        assert!(caps
            .api
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("incompatible"));
    }

    #[cfg(unix)]
    #[test]
    fn capabilities_do_not_claim_methods_missing_from_schema() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
            // Running + compatible, but schema advertises neither required method.
            r#"{"protocol":19,"schema_version":1,"methods":["session.ping"],"schemas":{}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.discover_capabilities_for_session(None);
        assert!(caps.server.running);
        assert_eq!(caps.server.compatible, Some(true));
        assert!(!caps.api.snapshot);
        assert!(!caps.api.tab_create);
        assert!(!caps.terminal.create);
        assert!(caps.terminal.control); // running + compatible
        let reason = caps.api.reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("session.snapshot") || reason.contains("tab.create"),
            "unexpected reason: {reason}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn capabilities_claim_methods_present_in_schema_when_compatible() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","tab.move","workspace.focus","workspace.create","session.ping","events.subscribe"],"schemas":{"session.snapshot":{},"tab.create":{},"tab.move":{},"workspace.focus":{},"workspace.create":{},"events.subscribe":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.discover_capabilities_for_session(None);
        assert!(caps.api.snapshot);
        assert!(caps.api.tab_create);
        assert!(caps.api.tab_move);
        assert!(caps.api.methods.iter().any(|method| method == "tab.move"));
        assert!(caps.api.workspace_focus);
        assert!(caps.api.workspace_create);
        assert!(caps.api.ping);
        assert!(caps.terminal.create);
        assert!(caps.terminal.control);
        assert!(caps.api.events_subscribe);
        assert_eq!(caps.events.status, HerdrEventsStatus::Available);
        assert!(caps.events.reason.is_none());
        assert!(caps.api.reason.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn capabilities_downgrade_when_live_socket_probe_fails() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("missing-herdr.sock");
        let status = format!(
            r#"{{"client":{{"version":"0.8.0","channel":"test","protocol":19,"binary":"FAKE"}},"server":{{"status":"running","running":true,"version":"0.8.0","protocol":19,"compatible":true,"socket":"{}"}},"update":{{"restart_needed":false}}}}"#,
            socket.display()
        );
        let sessions = format!(
            r#"{{"sessions":[{{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"{}"}}]}}"#,
            socket.display()
        );
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            &status,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping","events.subscribe"],"schemas":{"session.snapshot":{},"tab.create":{},"events.subscribe":{}}}"#,
            &sessions,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.capabilities();

        assert!(caps.server.running);
        assert!(!caps.api.snapshot);
        assert!(!caps.api.tab_create);
        assert!(!caps.api.events_subscribe);
        assert!(!caps.terminal.observe);
        assert!(!caps.terminal.control);
        assert!(!caps.terminal.create);
        assert_eq!(caps.events.status, HerdrEventsStatus::Unavailable);
        assert!(caps
            .api
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("local socket probe failed"));
    }

    #[test]
    fn connector_reader_rejects_oversized_line_and_does_not_emit_frame() {
        use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
        let session = Arc::new(ConnectorSession {
            id: "herdr-term-oversize".into(),
            mode: HerdrTerminalMode::Observe,
            cols: Mutex::new(40),
            rows: Mutex::new(10),
            child: Mutex::new(None),
            process_tree: Mutex::new(None),
            stdin: Mutex::new(None),
            reader: Mutex::new(None),
            closed: Mutex::new(false),
        });
        let (tx, rx) = mpsc::channel();
        let on_event: OnTerminalEvent =
            Arc::new(move |event| tx.send(event).map_err(|error| error.to_string()));
        let mut stdout = Vec::new();
        stdout.extend(std::iter::repeat(b'x').take(MAX_NDJSON_LINE_BYTES + 1));
        connector_reader_loop(
            session,
            std::io::Cursor::new(stdout),
            None::<std::io::Cursor<Vec<u8>>>,
            on_event,
        );
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        match event {
            HerdrTerminalEvent::Error { code, message, .. } => {
                assert_eq!(code, "tooLarge");
                assert!(message.contains("tooLarge"), "{message}");
            }
            other => panic!("expected tooLarge error, got {other:?}"),
        }
        assert!(rx
            .try_iter()
            .all(|event| !matches!(event, HerdrTerminalEvent::Frame { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn fake_connector_observe_emits_first_full_frame_and_release_kills_only_child() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_running_session(dir.path());
        let mgr = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel();
        let on_event: OnTerminalEvent = Arc::new(move |event| {
            let _ = tx.send(event);
            Ok(())
        });

        let opened = mgr
            .open_terminal(
                "term_test".into(),
                HerdrTerminalMode::Observe,
                false,
                40,
                10,
                None,
                on_event,
            )
            .unwrap();
        assert_eq!(opened.role, HerdrTerminalRole::Observer);
        assert_eq!(opened.mode, HerdrTerminalMode::Observe);

        let event = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("expected first frame");
        match event {
            HerdrTerminalEvent::Frame {
                session_id,
                seq,
                full,
                ..
            } => {
                assert_eq!(session_id, opened.session_id);
                assert_eq!(seq, 1);
                assert!(full);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let closed = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("expected clean EOF to emit closed");
        assert!(matches!(
            closed,
            HerdrTerminalEvent::Closed {
                reason: Some(ref reason),
                ..
            } if reason == "connector_eof"
        ));

        mgr.terminal_release(&opened.session_id).unwrap();
        assert!(mgr.sessions.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn fake_connector_control_resize_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_running_session(dir.path());
        let mgr = Arc::new(HerdrManager::with_binary(binary));
        let (tx, rx) = mpsc::channel::<HerdrTerminalEvent>();
        let on_event: OnTerminalEvent = Arc::new(move |event| {
            let _ = tx.send(event);
            Ok(())
        });

        let opened = mgr
            .open_terminal(
                "term_test".into(),
                HerdrTerminalMode::Control,
                false,
                40,
                10,
                None,
                on_event,
            )
            .unwrap();
        assert_eq!(opened.role, HerdrTerminalRole::Controller);
        {
            let sessions = mgr.sessions.lock().unwrap();
            let session = sessions.get(&opened.session_id).unwrap();
            assert!(session.child.lock().unwrap().is_some());
            assert!(session.process_tree.lock().unwrap().is_some());
        }

        let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            first,
            HerdrTerminalEvent::Frame {
                seq: 1,
                full: true,
                ..
            }
        ));

        mgr.terminal_resize(&opened.session_id, 40, 12).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_seq2 = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(HerdrTerminalEvent::Frame {
                    seq: 2,
                    full: false,
                    ..
                }) => {
                    saw_seq2 = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        assert!(saw_seq2, "expected seq=2 frame after resize");

        mgr.terminal_release(&opened.session_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_closed = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(HerdrTerminalEvent::Closed { .. }) => {
                    saw_closed = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(saw_closed || mgr.sessions.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn release_all_connectors_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_running_session(dir.path());
        let mgr = Arc::new(HerdrManager::with_binary(binary));
        let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
        let _ = mgr
            .open_terminal(
                "term_a".into(),
                HerdrTerminalMode::Observe,
                false,
                20,
                10,
                None,
                on_event.clone(),
            )
            .unwrap();
        let _ = mgr
            .open_terminal(
                "term_b".into(),
                HerdrTerminalMode::Control,
                false,
                20,
                10,
                None,
                on_event,
            )
            .unwrap();
        assert_eq!(mgr.sessions.lock().unwrap().len(), 2);
        mgr.release_all_connectors();
        assert!(mgr.sessions.lock().unwrap().is_empty());
        mgr.release_all_connectors();
    }
    #[test]
    fn parse_session_list_json_rejects_excessive_sessions() {
        let mut sessions = Vec::with_capacity(MAX_SESSION_COUNT + 1);
        for index in 0..=MAX_SESSION_COUNT {
            sessions.push(serde_json::json!({
                "name": format!("s{index}"),
                "default": false,
                "running": false,
                "session_dir": "/tmp",
                "socket_path": "/tmp/s.sock"
            }));
        }
        let error =
            parse_session_list_json(&serde_json::json!({ "sessions": sessions })).unwrap_err();
        assert!(error.contains("tooComplex"), "{error}");
    }

    #[test]
    fn parse_session_list_json_reads_exact_dto_fields() {
        let value = serde_json::json!({
            "sessions": [
                {
                    "name": "default",
                    "default": true,
                    "running": true,
                    "session_dir": "/tmp/herdr-default",
                    "socket_path": "/tmp/herdr-default.sock"
                },
                {
                    "name": "work",
                    "default": false,
                    "running": false,
                    "session_dir": "/tmp/herdr-work",
                    "socket_path": "/tmp/herdr-work.sock"
                }
            ]
        });
        let sessions = parse_session_list_json(&value).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "default");
        assert!(sessions[0].default);
        assert!(sessions[0].running);
        assert_eq!(sessions[0].session_dir, "/tmp/herdr-default");
        assert_eq!(sessions[0].socket_path, "/tmp/herdr-default.sock");
        assert_eq!(sessions[1].name, "work");
        assert!(!sessions[1].running);
    }

    #[cfg(unix)]
    #[test]
    fn list_sessions_uses_session_list_json_only() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/ignored.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot"],"schemas":{"session.snapshot":{}}}"#,
            r#"{"sessions":[{"name":"work","default":false,"running":true,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":false,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let sessions = mgr.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].socket_path, "/tmp/work.sock");
        let default = mgr.resolve_named_session(None).unwrap();
        assert_eq!(default.name, "default");
        assert!(!default.running);
        let work = mgr.resolve_named_session(Some("work")).unwrap();
        assert_eq!(work.socket_path, "/tmp/work.sock");
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_and_mutations_reject_stopped_session_without_starting_server() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
            r#"{"sessions":[{"name":"work","default":true,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"}]}"#,
        );
        let mgr = Arc::new(HerdrManager::with_binary(binary));
        let caps = mgr.capabilities_for_session(Some("work"));
        assert!(!caps.api.snapshot);
        assert!(!caps.api.tab_create);
        assert!(!caps.api.workspace_focus);
        assert!(!caps.api.workspace_create);
        assert!(!caps.terminal.create);
        assert!(!caps.terminal.observe);
        assert!(caps
            .api
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("not running"));

        let err = mgr.snapshot(Some("work")).unwrap_err();
        assert!(err.contains("not running"), "{err}");
        let err = mgr
            .workspace_focus(Some("work"), "ws-1".into())
            .unwrap_err();
        assert!(err.contains("not running"), "{err}");
        let err = mgr
            .workspace_create(Some("work"), Some("/tmp/x".into()), Some("X".into()), true)
            .unwrap_err();
        assert!(err.contains("not running"), "{err}");
        let err = mgr
            .create_terminal(Some("work"), Some("ws-1".into()), None)
            .unwrap_err();
        assert!(err.contains("not running"), "{err}");
        let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
        let err = mgr
            .open_terminal(
                "term".into(),
                HerdrTerminalMode::Observe,
                false,
                40,
                10,
                Some("work".into()),
                on_event,
            )
            .unwrap_err();
        assert!(err.contains("not running"), "{err}");
        // Fake binary has no session attach / server start subcommands — ensuring we never call them.
    }

    #[cfg(unix)]
    #[test]
    fn session_specific_socket_routes_api_and_sets_herdr_session_env() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("herdr-session-env");
        let sessions = r#"{"sessions":[{"name":"work","default":false,"running":true,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#;
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
            sessions,
        );
        let mgr = Arc::new(HerdrManager::with_binary(binary));
        let caps = mgr.discover_capabilities_for_session(Some("work"));
        assert_eq!(caps.server.socket_path.as_deref(), Some("/tmp/work.sock"));
        assert!(caps.api.snapshot);
        assert!(caps.api.workspace_focus);
        assert!(caps.api.workspace_create);

        // Socket override only for API path verification without a real unix socket.
        // workspace_focus payload uses require_running_session_socket → override.
        // We validate payload builders via parse helpers + env for connectors.

        let workspace_created = parse_workspace_created_response(serde_json::json!({
            "id": "1",
            "result": {
                "type": "workspace_created",
                "workspace": {
                    "workspace_id": "ws-9",
                    "number": 1,
                    "label": "feature-x",
                    "focused": true,
                    "pane_count": 1,
                    "tab_count": 1,
                    "active_tab_id": "tab-9",
                    "agent_status": "idle",
                    "worktree": {
                        "checkout_path": "/tmp/feature-x",
                        "is_linked_worktree": true,
                        "repo_key": "k",
                        "repo_name": "r",
                        "repo_root": "/tmp/r"
                    }
                },
                "tab": { "tab_id": "tab-9", "workspace_id": "ws-9" },
                "root_pane": {
                    "pane_id": "pane-9",
                    "terminal_id": "term-9",
                    "workspace_id": "ws-9"
                }
            }
        }))
        .unwrap();
        assert_eq!(workspace_created.workspace_id, "ws-9");
        assert_eq!(workspace_created.label, "feature-x");
        assert_eq!(workspace_created.path.as_deref(), Some("/tmp/feature-x"));
        assert_eq!(workspace_created.terminal_id.as_deref(), Some("term-9"));

        // Connector must export HERDR_SESSION=<name>.
        std::env::set_var("HERDR_TEST_ENV_FILE", &env_file);
        let on_event: OnTerminalEvent = Arc::new(|_| Ok(()));
        let opened = mgr
            .open_terminal(
                "term_work".into(),
                HerdrTerminalMode::Observe,
                false,
                40,
                10,
                Some("work".into()),
                on_event,
            )
            .unwrap();
        // Wait briefly for shell to write env file.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw = None;
        while Instant::now() < deadline {
            if env_file.exists() {
                saw = Some(fs::read_to_string(&env_file).unwrap());
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        mgr.terminal_release(&opened.session_id).unwrap();
        std::env::remove_var("HERDR_TEST_ENV_FILE");
        assert_eq!(saw.as_deref().map(str::trim), Some("work"));
    }

    #[cfg(unix)]
    #[test]
    fn unknown_named_session_never_inherits_default_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.focus","workspace.create"],"schemas":{"session.snapshot":{},"tab.create":{},"workspace.focus":{},"workspace.create":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.capabilities_for_session(Some("missing"));
        assert!(!caps.server.running);
        assert!(!caps.api.snapshot);
        assert!(!caps.api.tab_create);
        assert!(!caps.api.workspace_focus);
        assert!(!caps.api.workspace_create);
        assert!(!caps.terminal.observe);
        assert!(caps
            .api
            .reason
            .as_deref()
            .unwrap_or("")
            .contains("session not found"));
    }

    #[cfg(unix)]
    #[test]
    fn live_alias_resolves_to_default_named_session() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot"],"schemas":{"session.snapshot":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let live = mgr.resolve_named_session(Some("live")).unwrap();
        assert_eq!(live.name, "default");
        assert_eq!(live.socket_path, "/tmp/default.sock");
    }

    #[test]
    fn protocol19_request_payloads_match_installed_schema() {
        let tab = build_tab_create_params(
            Some("ws_1".into()),
            Some("Shell".into()),
            Some("/tmp/proj".into()),
            true,
        );
        assert_eq!(
            tab,
            serde_json::json!({
                "workspace_id": "ws_1",
                "label": "Shell",
                "cwd": "/tmp/proj",
                "focus": true
            })
        );

        let moved = build_tab_move_params("tab_1".into(), 2);
        assert_eq!(
            moved,
            serde_json::json!({ "tab_id": "tab_1", "insert_index": 2 })
        );

        let split = build_pane_split_params(
            HerdrSplitDirection::Right,
            Some("pane_1".into()),
            Some("ws_1".into()),
            None,
            Some(0.4),
            true,
        );
        assert_eq!(split["direction"], "right");
        assert_eq!(split["target_pane_id"], "pane_1");
        assert_eq!(split["ratio"], 0.4);
        assert_eq!(split["focus"], true);

        let export = build_layout_export_params(Some("tab_1".into()), None);
        assert_eq!(export, serde_json::json!({ "tab_id": "tab_1" }));

        // false = first, true = second along the BSP path.
        let ratio =
            build_layout_set_split_ratio_params(Some("tab_1".into()), None, &[false, true], 0.6);
        assert_eq!(
            ratio,
            serde_json::json!({
                "tab_id": "tab_1",
                "path": [false, true],
                "ratio": 0.6
            })
        );
    }

    #[test]
    fn parse_layout_export_reads_recursive_root_and_boolean_path_semantics() {
        let response = serde_json::json!({
            "id": "1",
            "result": {
                "type": "layout_export",
                "layout": {
                    "workspace_id": "ws_1",
                    "tab_id": "tab_1",
                    "zoomed": false,
                    "focused_pane_id": "pane_2",
                    "root": {
                        "type": "split",
                        "direction": "right",
                        "ratio": 0.6,
                        "first": {
                            "type": "pane",
                            "pane_id": "pane_1",
                            "label": "A",
                            "cwd": "/tmp/a"
                        },
                        "second": {
                            "type": "split",
                            "direction": "down",
                            "ratio": 0.5,
                            "first": {
                                "type": "pane",
                                "pane_id": "pane_2"
                            },
                            "second": {
                                "type": "pane",
                                "pane_id": "pane_3"
                            }
                        }
                    }
                }
            }
        });
        let layout = parse_layout_export_response(response).unwrap();
        assert_eq!(layout.workspace_id, "ws_1");
        assert_eq!(layout.tab_id, "tab_1");
        assert!(!layout.zoomed);
        assert_eq!(layout.focused_pane_id, "pane_2");
        let ipc_layout = serde_json::to_value(&layout).unwrap();
        assert_eq!(
            ipc_layout
                .pointer("/root/first/paneId")
                .and_then(|v| v.as_str()),
            Some("pane_1")
        );
        assert!(ipc_layout.pointer("/root/first/pane_id").is_none());
        match layout.root {
            HerdrLayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                assert_eq!(direction, HerdrSplitDirection::Right);
                assert!((ratio - 0.6).abs() < f64::EPSILON);
                match *first {
                    HerdrLayoutNode::Pane {
                        pane_id,
                        label,
                        cwd,
                    } => {
                        assert_eq!(pane_id.as_deref(), Some("pane_1"));
                        assert_eq!(label.as_deref(), Some("A"));
                        assert_eq!(cwd.as_deref(), Some("/tmp/a"));
                    }
                    other => panic!("expected first pane, got {other:?}"),
                }
                // path [true, false] => second then first => pane_2
                match *second {
                    HerdrLayoutNode::Split {
                        direction,
                        first,
                        second,
                        ..
                    } => {
                        assert_eq!(direction, HerdrSplitDirection::Down);
                        match (*first, *second) {
                            (
                                HerdrLayoutNode::Pane {
                                    pane_id: Some(a), ..
                                },
                                HerdrLayoutNode::Pane {
                                    pane_id: Some(b), ..
                                },
                            ) => {
                                assert_eq!(a, "pane_2");
                                assert_eq!(b, "pane_3");
                            }
                            other => panic!("expected nested panes, got {other:?}"),
                        }
                    }
                    other => panic!("expected nested split, got {other:?}"),
                }
            }
            other => panic!("expected root split, got {other:?}"),
        }

        let ratio_set = parse_layout_set_split_ratio_response(serde_json::json!({
            "id": "2",
            "result": {
                "type": "layout_split_ratio_set",
                "layout": {
                    "workspace_id": "ws_1",
                    "tab_id": "tab_1",
                    "zoomed": false,
                    "focused_pane_id": "pane_1",
                    "root": {
                        "type": "pane",
                        "pane_id": "pane_1"
                    }
                }
            }
        }))
        .unwrap();
        assert_eq!(ratio_set.focused_pane_id, "pane_1");
    }

    #[test]
    fn parse_layout_export_rejects_excessive_recursion() {
        let mut node = serde_json::json!({
            "type": "pane",
            "pane_id": "pane_leaf"
        });
        for _ in 0..=MAX_LAYOUT_DEPTH {
            node = serde_json::json!({
                "type": "split",
                "direction": "right",
                "ratio": 0.5,
                "first": node,
                "second": { "type": "pane", "pane_id": "pane_r" }
            });
        }
        let error = parse_layout_export_response(serde_json::json!({
            "result": {
                "type": "layout_export",
                "layout": {
                    "workspace_id": "ws_1",
                    "tab_id": "tab_1",
                    "zoomed": false,
                    "focused_pane_id": "pane_leaf",
                    "root": node
                }
            }
        }))
        .unwrap_err();
        assert!(error.contains("tooComplex"), "{error}");
    }

    #[test]
    fn parse_pane_info_response_reads_split_identity() {
        let parsed = parse_pane_info_response(serde_json::json!({
            "id": "1",
            "result": {
                "type": "pane_info",
                "pane": {
                    "pane_id": "pane_new",
                    "terminal_id": "term_new",
                    "workspace_id": "ws_1",
                    "tab_id": "tab_1",
                    "focused": true,
                    "agent_status": "idle",
                    "revision": 2,
                    "title": "Split"
                }
            }
        }))
        .unwrap();
        assert_eq!(parsed.pane_id, "pane_new");
        assert_eq!(parsed.terminal_id, "term_new");
        assert_eq!(parsed.tab_id, "tab_1");
        assert_eq!(parsed.workspace_id, "ws_1");
        assert_eq!(parsed.title.as_deref(), Some("Split"));
    }

    #[cfg(unix)]
    #[test]
    fn native_interaction_methods_gate_on_schema_and_stopped_session() {
        let dir = tempfile::tempdir().unwrap();
        // Running + compatible, but schema omits interaction methods.
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/herdr-fake.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","session.ping"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/herdr-default","socket_path":"/tmp/herdr-fake.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.discover_capabilities_for_session(None);
        assert!(caps.api.snapshot);
        assert!(caps.api.tab_create);
        assert!(!caps.api.workspace_rename);
        assert!(!caps.api.workspace_close);
        assert!(!caps.api.tab_focus);
        assert!(!caps.api.tab_rename);
        assert!(!caps.api.tab_close);
        assert!(!caps.api.pane_focus);
        assert!(!caps.api.pane_rename);
        assert!(!caps.api.pane_split);
        assert!(!caps.api.pane_zoom);
        assert!(!caps.api.pane_swap);
        assert!(!caps.api.pane_close);
        assert!(!caps.api.layout_export);
        assert!(!caps.api.layout_set_split_ratio);
        assert!(!caps.api.methods.iter().any(|m| m == "pane.split"));

        let err = mgr
            .layout_export(None, Some("tab_1".into()), None)
            .unwrap_err();
        assert!(
            err.contains("unavailable")
                || err.contains("lacks")
                || err.contains("not running")
                || err.contains("socket probe failed"),
            "{err}"
        );

        let dir2 = tempfile::tempdir().unwrap();
        let stopped = write_fake_herdr_with_sessions(
            dir2.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"not_running","running":false,"version":null,"protocol":null,"compatible":null,"socket":null},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create","workspace.rename","workspace.close","tab.focus","tab.rename","tab.close","pane.focus","pane.rename","pane.split","pane.zoom","pane.swap","pane.close","layout.export","layout.set_split_ratio"],"schemas":{}}"#,
            r#"{"sessions":[{"name":"work","default":true,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"}]}"#,
        );
        let mgr2 = HerdrManager::with_binary(stopped);
        let caps2 = mgr2.capabilities_for_session(Some("work"));
        assert!(!caps2.api.layout_export);
        assert!(!caps2.api.pane_split);
        assert!(!caps2.api.workspace_rename);
        for err in [
            mgr2.workspace_rename(Some("work"), "ws".into(), "X".into())
                .unwrap_err(),
            mgr2.workspace_close(Some("work"), "ws".into()).unwrap_err(),
            mgr2.tab_focus(Some("work"), "tab".into()).unwrap_err(),
            mgr2.tab_rename(Some("work"), "tab".into(), "T".into())
                .unwrap_err(),
            mgr2.tab_close(Some("work"), "tab".into()).unwrap_err(),
            mgr2.pane_focus(Some("work"), "pane".into()).unwrap_err(),
            mgr2.pane_rename(Some("work"), "pane".into(), Some("P".into()))
                .unwrap_err(),
            mgr2.pane_split(
                Some("work"),
                HerdrSplitDirection::Down,
                Some("pane".into()),
                None,
                None,
                None,
                true,
            )
            .unwrap_err(),
            mgr2.pane_zoom(Some("work"), Some("pane".into()), None)
                .unwrap_err(),
            mgr2.pane_swap(Some("work"), Some("a".into()), Some("b".into()), None, None)
                .unwrap_err(),
            mgr2.pane_close(Some("work"), "pane".into()).unwrap_err(),
            mgr2.layout_export(Some("work"), Some("tab".into()), None)
                .unwrap_err(),
            mgr2.layout_set_split_ratio(Some("work"), Some("tab".into()), None, vec![false], 0.5)
                .unwrap_err(),
        ] {
            assert!(err.contains("not running"), "{err}");
        }
    }

    #[test]
    fn parse_worktree_list_response_accepts_protocol19_fields() {
        let parsed = parse_worktree_list_response(serde_json::json!({
            "id": "1",
            "result": {
                "type": "worktree_list",
                "source": {
                    "repo_key": "/repo/.git",
                    "repo_name": "repo",
                    "repo_root": "/repo",
                    "source_checkout_path": "/repo",
                    "source_workspace_id": "w1"
                },
                "worktrees": [
                    {
                        "path": "/repo",
                        "branch": "main",
                        "is_bare": false,
                        "is_detached": false,
                        "is_prunable": false,
                        "is_linked_worktree": false,
                        "label": "repo",
                        "open_workspace_id": "w1"
                    },
                    {
                        "path": "/repo-feature",
                        "branch": null,
                        "is_bare": false,
                        "is_detached": true,
                        "is_prunable": true,
                        "is_linked_worktree": true,
                        "label": "feature",
                        "open_workspace_id": "w2"
                    },
                    {
                        "path": "\\\\?\\C:\\src\\yuzora",
                        "branch": "main",
                        "is_bare": false,
                        "is_detached": false,
                        "is_prunable": false,
                        "is_linked_worktree": false,
                        "label": "win",
                        "open_workspace_id": "w3"
                    }
                ]
            }
        }))
        .unwrap();
        assert_eq!(parsed.source.repo_name, "repo");
        assert_eq!(parsed.source.source_workspace_id.as_deref(), Some("w1"));
        assert_eq!(parsed.worktrees.len(), 3);
        assert_eq!(parsed.worktrees[0].branch.as_deref(), Some("main"));
        assert!(!parsed.worktrees[0].is_linked_worktree);
        assert!(parsed.worktrees[1].is_detached);
        assert!(parsed.worktrees[1].is_linked_worktree);
        assert!(parsed.worktrees[1].branch.is_none());
        assert_eq!(parsed.worktrees[2].path, "\\\\?\\C:\\src\\yuzora");
    }

    #[test]
    fn parse_worktree_list_rejects_wrong_type() {
        let err = parse_worktree_list_response(serde_json::json!({
            "id": "1",
            "result": { "type": "workspace_list", "workspaces": [] }
        }))
        .unwrap_err();
        assert!(
            err.contains("unexpected worktree.list result type"),
            "{err}"
        );
    }

    #[test]
    fn parse_subscription_event_worktree_dirty_signals() {
        let cases = [
            (
                r#"{"event":"worktree_created","data":{"type":"worktree_created","workspace":{"workspace_id":"w9"},"worktree":{"path":"/tmp/x"}}}"#,
                "created",
            ),
            (
                r#"{"event":"worktree_opened","data":{"type":"worktree_opened","workspace":{"workspace_id":"w9"},"worktree":{"path":"/tmp/x"},"already_open":false}}"#,
                "opened",
            ),
            (
                r#"{"event":"worktree_removed","data":{"type":"worktree_removed","workspace_id":"w9","worktree":{"path":"/tmp/x"},"forced":false}}"#,
                "removed",
            ),
        ];
        for (line, expected_kind) in cases {
            let event = parse_subscription_event_line("sub-1", line)
                .unwrap()
                .unwrap();
            assert_eq!(
                event,
                HerdrSubscriptionEvent::WorktreeChanged {
                    subscription_id: "sub-1".into(),
                    kind: expected_kind.into(),
                    workspace_id: Some("w9".into()),
                }
            );
        }
    }

    #[test]
    fn parse_subscription_event_accepts_dotted_worktree_selector_envelopes() {
        let event = parse_subscription_event_line(
            "sub-1",
            r#"{"event":"worktree.created","data":{"workspace":{"workspace_id":"w9"}}}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            event,
            HerdrSubscriptionEvent::WorktreeChanged {
                subscription_id: "sub-1".into(),
                kind: "created".into(),
                workspace_id: Some("w9".into()),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_list_refuses_stopped_named_session() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"stopped","running":false,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/work.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","worktree.list"],"schemas":{"session.snapshot":{},"worktree.list":{}}}"#,
            r#"{"sessions":[{"name":"work","default":false,"running":false,"session_dir":"/tmp/work","socket_path":"/tmp/work.sock"},{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let err = mgr
            .worktree_list(Some("work"), None, Some("w1".into()))
            .unwrap_err();
        assert!(err.contains("not running"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn worktree_list_capability_is_schema_gated() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","workspace.focus"],"schemas":{"session.snapshot":{},"workspace.focus":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.discover_capabilities_for_session(Some("default"));
        assert!(!caps.api.worktree_list);
        assert!(!caps.api.methods.iter().any(|m| m == "worktree.list"));
        let err = mgr.worktree_list(Some("default"), None, None).unwrap_err();
        // Schema-gated false method surfaces the capability reason (never invents success).
        assert!(
            err.contains("unavailable")
                || err.contains("not")
                || err.contains("lacks")
                || err.contains("worktree.list")
                || err.contains("socket probe failed"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tab_move_capability_is_schema_gated() {
        let dir = tempfile::tempdir().unwrap();
        let binary = write_fake_herdr_with_sessions(
            dir.path(),
            r#"{"client":{"version":"0.0.0-fake","channel":"test","protocol":19,"binary":"FAKE"},"server":{"status":"running","running":true,"version":"0.0.0-fake","protocol":19,"compatible":true,"socket":"/tmp/default.sock"},"update":{"restart_needed":false}}"#,
            r#"{"protocol":19,"schema_version":1,"methods":["session.snapshot","tab.create"],"schemas":{"session.snapshot":{},"tab.create":{}}}"#,
            r#"{"sessions":[{"name":"default","default":true,"running":true,"session_dir":"/tmp/default","socket_path":"/tmp/default.sock"}]}"#,
        );
        let mgr = HerdrManager::with_binary(binary);
        let caps = mgr.discover_capabilities_for_session(Some("default"));
        assert!(!caps.api.tab_move);
        assert!(!caps.api.methods.iter().any(|m| m == "tab.move"));
        let err = mgr
            .tab_move(Some("default"), "tab_1".into(), 1)
            .unwrap_err();
        assert!(
            err.contains("unavailable")
                || err.contains("not")
                || err.contains("lacks")
                || err.contains("tab.move")
                || err.contains("socket probe failed"),
            "{err}"
        );
    }

    #[test]
    fn native_interaction_commands_are_registered_in_lib() {
        let source = include_str!("lib.rs");
        for cmd in [
            "herdr_service::herdr_workspace_rename",
            "herdr_service::herdr_workspace_close",
            "herdr_service::herdr_worktree_list",
            "herdr_service::herdr_tab_create",
            "herdr_service::herdr_tab_focus",
            "herdr_service::herdr_tab_rename",
            "herdr_service::herdr_tab_close",
            "herdr_service::herdr_tab_move",
            "herdr_service::herdr_pane_focus",
            "herdr_service::herdr_pane_rename",
            "herdr_service::herdr_pane_split",
            "herdr_service::herdr_pane_zoom",
            "herdr_service::herdr_pane_swap",
            "herdr_service::herdr_pane_close",
            "herdr_service::herdr_layout_export",
            "herdr_service::herdr_layout_set_split_ratio",
            "herdr_service::herdr_binary_source_get",
            "herdr_service::herdr_binary_source_set",
            "herdr_service::herdr_agent_get",
            "herdr_service::herdr_agent_read",
            "herdr_service::herdr_events_subscribe",
            "herdr_service::herdr_events_release",
        ] {
            assert!(source.contains(cmd), "missing command registration: {cmd}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_oversized_stdout_is_killed_and_too_large() {
        let script = format!(
            "import sys; sys.stdout.buffer.write(b'x' * {})",
            MAX_NDJSON_LINE_BYTES + 8
        );
        let mut child = Command::new("python3")
            .args(["-c", &script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("python3");
        let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
        let err = wait_bounded_child(
            &mut child,
            &mut process_tree,
            Duration::from_secs(5),
            MAX_NDJSON_LINE_BYTES,
        )
        .expect_err("oversized stdout");
        assert!(err.contains("tooLarge"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_invalid_utf8_is_protocol_error() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), [0xff, 0xfe]).unwrap();
        let err = run_herdr_json_with_session_timeout(
            Path::new("/bin/cat"),
            &[tmp.path().to_str().unwrap()],
            None,
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(err.contains("invalidUtf8"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_normal_json_is_parsed() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), br#"{"ok":true}"#).unwrap();
        let value = run_herdr_json_with_session_timeout(
            Path::new("/bin/cat"),
            &[tmp.path().to_str().unwrap()],
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(value["ok"], true);
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_timeout_kills_and_reaps_child() {
        let started = Instant::now();
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let mut process_tree = process_kill::attach_process_tree(&mut child).unwrap();
        let err = wait_bounded_child(
            &mut child,
            &mut process_tree,
            Duration::from_millis(80),
            MAX_NDJSON_LINE_BYTES,
        )
        .expect_err("timeout");
        assert!(err.contains("timeout"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "30s child was not reaped within the configured timeout"
        );
        assert!(!unix_pid_exists(pid), "child {pid} should be reaped");
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_success_with_invalid_utf8_stderr_is_protocol_error() {
        let script = r#"
import sys
sys.stdout.buffer.write(b'{"ok":true}')
sys.stderr.buffer.write(b"\xff\xfe")
"#;
        let err = run_herdr_json_with_session_timeout(
            Path::new("python3"),
            &["-c", script],
            None,
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(err.contains("invalidUtf8"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn herdr_cli_long_child_is_reaped_within_timeout() {
        let started = Instant::now();
        let err = run_herdr_json_with_session_timeout(
            Path::new("sleep"),
            &["30"],
            None,
            Duration::from_millis(120),
        )
        .unwrap_err();
        assert!(err.contains("timeout"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "long child was not reaped within timeout"
        );
    }

    fn unix_pid_exists(pid: u32) -> bool {
        #[cfg(unix)]
        {
            let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if rc == 0 {
                return true;
            }
            std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }
}
