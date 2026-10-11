//! Herdr runtime lane: public NDJSON API + official terminal session connectors.
//!
//! Authority is the selected installed `herdr` binary (discover/interrogate at
//! runtime). Never hardcode a protocol number, stop an existing Herdr server,
//! or kill Herdr panes — only Yuzora-owned connector children and a failed
//! server child started by this process are released/terminated.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(unix)]
use std::sync::Weak;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::herdr_backend::{HerdrMetadata, HerdrRemoteBackend};
use crate::herdr_limits::{
    bound_optional_json, bounded_ipc, ensure_ipc_bound, ensure_raw_ipc_bound,
    parse_herdr_cli_stdout, read_bounded_bytes, read_bounded_ndjson_line, validate_json_complexity,
    validate_snapshot_counts, BoundedNdjsonReadError, HerdrProtocolError, MAX_IPC_BYTES,
    MAX_JSON_ARRAY_LEN, MAX_JSON_DEPTH, MAX_JSON_OBJECT_KEYS, MAX_LAYOUT_DEPTH,
    MAX_NDJSON_LINE_BYTES, MAX_PANE_COUNT, MAX_SESSION_COUNT, MAX_STATE_LABELS, MAX_WORKTREE_COUNT,
};
#[cfg(unix)]
use crate::herdr_transport::LocalStream;
use crate::herdr_transport::{
    connect_local_stream, read_local_ndjson_line, read_local_ndjson_line_with,
    write_local_all_until, LocalWaitProfile,
};
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
#[cfg(all(test, unix))]
const TEST_EVENT_RECV_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOCAL_IO_TIMEOUT: Duration = Duration::from_secs(5);
/// Reuse expensive CLI inventory and binary fingerprint checks on hot paths.
/// Explicit lists stay fresh; failures and lifecycle changes invalidate caches.
/// Socket paths still come only from the authoritative session list.
const RUNTIME_VALIDATION_TTL: Duration = Duration::from_secs(10);
/// Background Session-list polls reuse the inventory this long; explicit
/// refreshes and every failure/lifecycle invalidation still read the CLI.
/// Worst-case latency to notice a Session started/stopped outside Yuzora.
const SESSION_POLL_TTL: Duration = Duration::from_secs(12);
/// Probe lock map size above which idle entries are pruned.
const CAPABILITY_PROBE_LOCK_LIMIT: usize = 64;
/// Cheap socket identity checks retain their faster restart-detection cadence.
const SOCKET_IDENTITY_TTL: Duration = Duration::from_secs(1);
/// Error code shown (translated by the UI) when no `herdr` is on PATH.
pub const HERDR_PATH_BINARY_NOT_FOUND: &str = "herdr-path-binary-not-found";
#[cfg(not(test))]
const HERDR_CLI_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const HERDR_CLI_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const HERDR_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const HERDR_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(test))]
const HERDR_STARTUP_STATUS_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const HERDR_STARTUP_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const HERDR_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

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
    Global,
    #[default]
    Default,
    Custom,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrBinarySourceInfo {
    #[serde(default)]
    pub custom_path: Option<String>,
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
    /// `workspace.move { workspace_id, insert_index }` for Space ordering.
    pub workspace_move: bool,
    pub workspace_move_block: bool,
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
pub struct HerdrWorkspaceOrderResult {
    pub workspace_ids: Vec<String>,
}

fn parse_workspace_order(response: serde_json::Value) -> Result<HerdrWorkspaceOrderResult, String> {
    if response["result"]["type"] != "workspace_list" {
        return Err("unexpected workspace reorder response".into());
    }
    let workspaces = response["result"]["workspaces"]
        .as_array()
        .ok_or("workspace list missing")?;
    if workspaces.len() > crate::herdr_limits::MAX_WORKSPACE_COUNT {
        return Err("workspace list exceeds limit".into());
    }
    let mut ids = Vec::with_capacity(workspaces.len());
    let mut seen = std::collections::HashSet::new();
    for workspace in workspaces {
        let id = workspace["workspace_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("workspace identity missing")?;
        if !seen.insert(id) {
            return Err("duplicate workspace identity".into());
        }
        ids.push(id.to_owned());
    }
    bounded_ipc(HerdrWorkspaceOrderResult { workspace_ids: ids })
}

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

/// Left-button pointer action for the connector `terminal.mouse` (HERDR 0.9.2+).
/// `Move` is hover without a button; HERDR forwards it only to children that enabled any-motion (1003).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HerdrMouseAction {
    Down,
    Up,
    Drag,
    Move,
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

/// Single-pass parse of a connector line straight into [`HerdrWireFrame`]:
/// no intermediate `Value`, and unknown fields are skipped under the same
/// depth/array/key ceilings as `validate_json_complexity`. It only ever answers
/// "clean frame"; any failure (syntax, types, a ceiling, duplicate keys) is
/// re-judged by the legacy `Value` route so error kinds stay identical.
struct FastWireFrame(HerdrWireFrame);

struct BoundedSkip(usize);

impl BoundedSkip {
    fn leaf<E: serde::de::Error>(&self) -> Result<(), E> {
        if self.0 > MAX_JSON_DEPTH {
            return Err(E::custom("depth"));
        }
        Ok(())
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for BoundedSkip {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for BoundedSkip {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("any JSON value within the complexity limits")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        self.leaf()
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        self.leaf()
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        self.leaf()
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        self.leaf()
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        self.leaf()
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        self.leaf()
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        self.leaf()?;
        let mut count = 0;
        while seq.next_element_seed(BoundedSkip(self.0 + 1))?.is_some() {
            count += 1;
            if count > MAX_JSON_ARRAY_LEN {
                return Err(serde::de::Error::custom("array"));
            }
        }
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.leaf()?;
        let mut count = 0;
        while map.next_key::<serde::de::IgnoredAny>()?.is_some() {
            count += 1;
            if count > MAX_JSON_OBJECT_KEYS {
                return Err(serde::de::Error::custom("object"));
            }
            map.next_value_seed(BoundedSkip(self.0 + 1))?;
        }
        Ok(())
    }
}

impl<'de> serde::Deserialize<'de> for FastWireFrame {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FrameVisitor;
        impl<'de> serde::de::Visitor<'de> for FrameVisitor {
            type Value = FastWireFrame;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a connector frame object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<FastWireFrame, A::Error> {
                use serde::de::Error;
                fn once<T>(slot: &mut Option<T>, value: T) -> Result<(), &'static str> {
                    match slot.replace(value) {
                        Some(_) => Err("duplicate field"),
                        None => Ok(()),
                    }
                }
                let (mut kind, mut seq, mut full, mut encoding) = (None, None, None, None);
                let (mut width, mut height, mut bytes, mut reason) = (None, None, None, None);
                let mut keys = 0;
                while let Some(key) = map.next_key::<std::borrow::Cow<str>>()? {
                    keys += 1;
                    if keys > MAX_JSON_OBJECT_KEYS {
                        return Err(A::Error::custom("object"));
                    }
                    match &*key {
                        "type" => once(&mut kind, map.next_value::<String>()?),
                        "seq" => once(&mut seq, map.next_value::<Option<u64>>()?),
                        "full" => once(&mut full, map.next_value::<Option<bool>>()?),
                        "encoding" => once(&mut encoding, map.next_value::<Option<String>>()?),
                        "width" => once(&mut width, map.next_value::<Option<u32>>()?),
                        "height" => once(&mut height, map.next_value::<Option<u32>>()?),
                        "bytes" => once(&mut bytes, map.next_value::<Option<String>>()?),
                        "reason" => once(&mut reason, map.next_value::<Option<String>>()?),
                        _ => {
                            map.next_value_seed(BoundedSkip(1))?;
                            Ok(())
                        }
                    }
                    .map_err(A::Error::custom)?;
                }
                Ok(FastWireFrame(HerdrWireFrame {
                    kind: kind.ok_or_else(|| A::Error::missing_field("type"))?,
                    seq: seq.flatten(),
                    full: full.flatten(),
                    encoding: encoding.flatten(),
                    width: width.flatten(),
                    height: height.flatten(),
                    bytes: bytes.flatten(),
                    reason: reason.flatten(),
                }))
            }
        }
        deserializer.deserialize_map(FrameVisitor)
    }
}

enum WireLineError {
    Parse(String),
    TooComplex(HerdrProtocolError),
}

fn parse_wire_line(line: &str) -> Result<HerdrWireFrame, WireLineError> {
    if let Ok(FastWireFrame(wire)) = serde_json::from_str(line) {
        return Ok(wire);
    }
    let parse =
        |error: serde_json::Error| WireLineError::Parse(format!("invalid connector json: {error}"));
    let value: serde_json::Value = serde_json::from_str(line).map_err(parse)?;
    validate_json_complexity(&value).map_err(WireLineError::TooComplex)?;
    serde_json::from_value(value).map_err(parse)
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
    InvalidGeometry,
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
        self.ingest_wire_owned(wire.clone())
    }

    pub fn ingest_frame(&mut self, wire: &HerdrWireFrame) -> FrameDecision {
        self.ingest_frame_owned(wire.clone())
    }

    /// Same rules, moving the payload out of `wire` instead of cloning it.
    pub fn ingest_wire_owned(&mut self, wire: HerdrWireFrame) -> FrameDecision {
        match wire.kind.as_str() {
            "terminal.closed" => FrameDecision::Closed {
                reason: wire.reason,
            },
            "terminal.frame" => self.ingest_frame_owned(wire),
            _ => FrameDecision::Ignore,
        }
    }

    fn ingest_frame_owned(&mut self, wire: HerdrWireFrame) -> FrameDecision {
        let Some(seq) = wire.seq else {
            return FrameDecision::Resync {
                expected_seq: self.next_expected(),
                received_seq: None,
                message: "terminal.frame missing seq".into(),
            };
        };
        let full = wire.full.unwrap_or(false);
        let valid_dimension = |value: u32| (1..=1000).contains(&value);
        let valid_geometry = match (wire.width, wire.height) {
            (Some(width), Some(height)) => valid_dimension(width) && valid_dimension(height),
            (None, None) => !full,
            _ => false,
        };
        if !valid_geometry {
            return FrameDecision::InvalidGeometry;
        }
        let encoding = wire.encoding.unwrap_or_else(|| "ansi".to_string());
        let width = wire.width.unwrap_or(0);
        let height = wire.height.unwrap_or(0);
        let Some(bytes_base64) = wire.bytes else {
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
    /// `column`/`row` are the zero-based pointer cell HERDR uses when it
    /// routes the wheel to a mouse-reporting application.
    #[serde(rename = "terminal.scroll")]
    Scroll {
        direction: HerdrScrollDirection,
        lines: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        column: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        row: Option<u16>,
    },
    /// HERDR encodes the event at this zero-based cell for the child's mouse
    /// mode, and drops it when the child has not enabled mouse reporting.
    #[serde(rename = "terminal.mouse")]
    Mouse {
        action: HerdrMouseAction,
        column: u16,
        row: u16,
        modifiers: u8,
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

    pub fn scroll(
        direction: HerdrScrollDirection,
        lines: u32,
        column: Option<u16>,
        row: Option<u16>,
    ) -> Result<Self, String> {
        if lines == 0 {
            return Err("terminal.scroll lines must be greater than 0".into());
        }
        Ok(Self::Scroll {
            direction,
            lines,
            column,
            row,
        })
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
    remote: Option<Arc<dyn HerdrRemoteBackend>>,
    sessions: Mutex<HashMap<String, Arc<ConnectorSession>>>,
    native_clients: Mutex<HashMap<String, Arc<native_client::NativeHerdrClient>>>,
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
    configured_custom_path: Mutex<Option<PathBuf>>,
    active_custom_path: Mutex<Option<PathBuf>>,
    /// Diagnostic retained when the persisted preference cannot be trusted.
    binary_source_config_error: Mutex<Option<String>>,
    /// Detailed failure from the automatic default-server startup attempt.
    startup_error: Mutex<Option<String>>,
    /// Serialize all local status/check-and-launch paths, including named sessions.
    pub(crate) startup_lock: Mutex<()>,
    /// Serializes atomic preference replacement and matching in-memory updates.
    binary_source_write_lock: Mutex<()>,
    /// Capability documents are expensive to discover because they spawn the
    /// selected CLI for status + schema. Fast paths cache them only while the
    /// named-session socket, server protocol, and selected binary fingerprint
    /// still match. Session list + `ping` validate at their bounded cadences.
    capability_cache: Mutex<HashMap<String, CachedCapabilities>>,
    /// One probe lock per cache key: probes of the same Session stay ordered,
    /// while a slow Session never blocks another Session's discovery.
    capability_probe_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Short-lived validation state for hot paths; see `RUNTIME_VALIDATION_TTL`.
    session_inventory: Mutex<Option<(Instant, Vec<HerdrNamedSession>)>>,
    /// CLI single-flight is separate so warm readers never wait for CLI I/O.
    session_inventory_refresh_lock: Mutex<()>,
    /// Incremented under the inventory lock; fences publication after invalidation.
    session_inventory_epoch: AtomicU64,
    socket_identity: Mutex<HashMap<String, (Instant, ServerIdentity)>>,
    binary_fingerprint_cache: Mutex<Option<(Instant, Option<String>)>>,
    validation_ttl: Mutex<Duration>,
    session_poll_ttl: Mutex<Duration>,
}

/// Live server `(version, protocol)` reported by `ping`.
type ServerIdentity = (String, u32);

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
    // The reader owns the socket. A completed reader must close it even if the
    // frontend has not released its subscription entry yet.
    #[cfg(unix)]
    reader_socket: Weak<LocalStream>,
}

impl Default for HerdrManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HerdrManager {
    pub fn new() -> Self {
        Self {
            remote: None,
            sessions: Mutex::new(HashMap::new()),
            native_clients: Mutex::new(HashMap::new()),
            event_subscriptions: Mutex::new(HashMap::new()),
            binary_override: Mutex::new(None),
            managed_binary_override: Mutex::new(None),
            socket_override: Mutex::new(None),
            config_dir: Mutex::new(None),
            resource_dir: Mutex::new(None),
            configured_source: Mutex::new(HerdrBinarySource::Default),
            active_source: Mutex::new(HerdrBinarySource::Default),
            configured_custom_path: Mutex::new(None),
            active_custom_path: Mutex::new(None),
            binary_source_config_error: Mutex::new(None),
            startup_error: Mutex::new(None),
            startup_lock: Mutex::new(()),
            binary_source_write_lock: Mutex::new(()),
            capability_cache: Mutex::new(HashMap::new()),
            capability_probe_locks: Mutex::new(HashMap::new()),
            session_inventory: Mutex::new(None),
            session_inventory_refresh_lock: Mutex::new(()),
            session_inventory_epoch: AtomicU64::new(0),
            socket_identity: Mutex::new(HashMap::new()),
            binary_fingerprint_cache: Mutex::new(None),
            validation_ttl: Mutex::new(RUNTIME_VALIDATION_TTL),
            session_poll_ttl: Mutex::new(SESSION_POLL_TTL),
        }
    }

    /// Wire app-data / resource directories once during Tauri setup.
    pub fn configure_paths(&self, config_dir: PathBuf, resource_dir: Option<PathBuf>) {
        *self.config_dir.lock().unwrap() = Some(config_dir.clone());
        *self.resource_dir.lock().unwrap() = resource_dir;
        let loaded = load_binary_source_preference(&config_dir);
        *self.configured_source.lock().unwrap() = loaded.source;
        *self.active_source.lock().unwrap() = loaded.source;
        *self.configured_custom_path.lock().unwrap() = loaded.custom_path.clone();
        *self.active_custom_path.lock().unwrap() = loaded.custom_path;
        *self.binary_source_config_error.lock().unwrap() = loaded.error;
        self.capability_cache.lock().unwrap().clear();
        self.invalidate_runtime_caches();
    }

    /// Drop the short-lived Session/ping/fingerprint validation state so the
    /// next request rediscovers it. Called after any request failure and any
    /// Session lifecycle change.
    pub(crate) fn invalidate_runtime_caches(&self) {
        {
            let mut inventory = self.session_inventory.lock().unwrap();
            *inventory = None;
            self.session_inventory_epoch.fetch_add(1, Ordering::SeqCst);
        }
        self.socket_identity.lock().unwrap().clear();
        *self.binary_fingerprint_cache.lock().unwrap() = None;
    }

    // Only the Unix fake-socket validation tests shorten the window.
    #[cfg(all(test, unix))]
    pub(crate) fn set_validation_ttl_for_test(&self, ttl: Duration) {
        *self.validation_ttl.lock().unwrap() = ttl;
    }

    #[cfg(all(test, unix))]
    pub(crate) fn set_session_poll_ttl_for_test(&self, ttl: Duration) {
        *self.session_poll_ttl.lock().unwrap() = ttl;
    }

    fn validation_fresh(&self, at: Instant) -> bool {
        at.elapsed() < *self.validation_ttl.lock().unwrap()
    }

    /// Binary fingerprint reused within the validation window. Remote runtimes
    /// answer this over the host helper, so avoid one round trip per request.
    fn recent_binary_fingerprint(&self) -> Option<String> {
        if let Some((at, fingerprint)) = self.binary_fingerprint_cache.lock().unwrap().clone() {
            if self.validation_fresh(at) {
                return fingerprint;
            }
        }
        let fingerprint = self.active_binary_fingerprint();
        *self.binary_fingerprint_cache.lock().unwrap() =
            Some((Instant::now(), fingerprint.clone()));
        fingerprint
    }

    /// Live server identity for a socket, pinged at most once per validation window.
    fn recent_server_identity(&self, socket_path: &str) -> Option<(String, u32)> {
        if let Some((at, identity)) = self
            .socket_identity
            .lock()
            .unwrap()
            .get(socket_path)
            .cloned()
        {
            if at.elapsed() < SOCKET_IDENTITY_TTL {
                return Some(identity);
            }
        }
        let identity = self.ping_server_identity(socket_path).ok()?;
        self.socket_identity
            .lock()
            .unwrap()
            .insert(socket_path.to_owned(), (Instant::now(), identity.clone()));
        Some(identity)
    }

    /// Start the resolved local Herdr headless server from a background worker.
    /// The server is intentionally detached and remains independent of Yuzora's
    /// connector-child cleanup on app exit.
    pub fn ensure_server_running_on_startup(&self) -> Result<bool, String> {
        if self.remote.is_some() {
            return Err("start-runtime-on-owning-host".into());
        }
        self.ensure_server_running_with_timeouts(
            HERDR_STARTUP_TIMEOUT,
            HERDR_STARTUP_STATUS_TIMEOUT,
            HERDR_STARTUP_POLL_INTERVAL,
        )
    }

    fn ensure_server_running_with_timeouts(
        &self,
        startup_timeout: Duration,
        status_timeout: Duration,
        poll_interval: Duration,
    ) -> Result<bool, String> {
        let _startup = self.startup_lock.lock().unwrap();
        let result = (|| -> Result<bool, String> {
            let binary = self
                .resolve_binary()
                .ok_or_else(|| "herdr-binary-unavailable".to_string())?;
            let existing = query_herdr_server_startup_status(&binary, status_timeout)?;
            if existing.running {
                if existing.compatible == Some(false) {
                    return Err(
                        "running Herdr server is protocol incompatible; update or select a compatible Herdr client while preserving running sessions"
                            .to_string(),
                    );
                }
                return Ok(false);
            }

            let mut command = default_server_command(&binary);
            process_kill::configure_background_process(&mut command);
            let mut child = command.spawn().map_err(|error| {
                format!(
                    "failed to launch Herdr server from {}: {error}",
                    binary.display()
                )
            })?;
            let deadline = Instant::now() + startup_timeout;
            let timeout_error = |child: &mut Child, last_probe: &str| {
                let message = format!(
                    "Herdr server did not become ready within {}s; {last_probe}",
                    startup_timeout.as_secs_f64()
                );
                match process_kill::terminate_direct_child_and_reap(child) {
                    Ok(()) => message,
                    Err(error) => {
                        format!("{message}; failed to terminate startup child: {error}")
                    }
                }
            };

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timeout_error(&mut child, "server has not reported running"));
                }
                let probe_timeout = status_timeout.min(remaining);
                let last_probe = match query_herdr_server_running(&binary, probe_timeout) {
                    Ok(true) => return Ok(true),
                    Ok(false) => "server has not reported running".to_string(),
                    Err(error) => error,
                };

                let child_status = match child.try_wait() {
                    Ok(status) => status,
                    Err(error) => {
                        let message = format!("failed to inspect Herdr server process: {error}");
                        return Err(
                            match process_kill::terminate_direct_child_and_reap(&mut child) {
                                Ok(()) => message,
                                Err(cleanup_error) => {
                                    format!("{message}; failed to terminate startup child: {cleanup_error}")
                                }
                            },
                        );
                    }
                };
                if let Some(status) = child_status {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if !remaining.is_zero()
                        && query_herdr_server_running(&binary, status_timeout.min(remaining))
                            .unwrap_or(false)
                    {
                        return Ok(true);
                    }
                    return Err(format!(
                        "Herdr server exited before becoming ready ({status}); {last_probe}"
                    ));
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timeout_error(&mut child, &last_probe));
                }
                std::thread::sleep(poll_interval.min(remaining));
            }
        })();
        *self.startup_error.lock().unwrap() = result.as_ref().err().cloned();
        if matches!(result, Ok(true)) {
            // Inventories read before the spawn list the Session as stopped.
            self.invalidate_runtime_caches();
        }
        result
    }

    pub fn with_binary(binary: PathBuf) -> Self {
        let mgr = Self::new();
        *mgr.binary_override.lock().unwrap() = Some(binary);
        mgr
    }

    /// Remote facades never inspect or execute the remote binary on this machine.
    /// Connector lifecycle remains on the host and uses its dedicated stream.
    pub fn with_remote(binary: PathBuf, backend: Arc<dyn HerdrRemoteBackend>) -> Self {
        let mut manager = Self::with_binary(binary);
        manager.remote = Some(backend);
        manager
    }

    pub fn metadata(
        &self,
        query: HerdrMetadata,
        session: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        if let Some(remote) = &self.remote {
            return remote.metadata(query, session);
        }
        match query {
            HerdrMetadata::BinarySource => {
                serde_json::to_value(self.binary_source_info()).map_err(|e| e.to_string())
            }
            HerdrMetadata::BinaryFingerprint => {
                Ok(serde_json::json!(self.active_binary_fingerprint()))
            }
            _ => {
                let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
                let args: &[&str] = match query {
                    HerdrMetadata::Sessions => &["session", "list", "--json"],
                    HerdrMetadata::Status => &["status", "--json"],
                    HerdrMetadata::Schema => &["api", "schema", "--json"],
                    _ => unreachable!(),
                };
                run_herdr_json_with_session(&binary, args, session)
            }
        }
    }

    fn request_api(
        &self,
        socket: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if let Some(remote) = &self.remote {
            return validate_api_response(remote.request(socket, method, params)?);
        }
        api_request(socket, method, params)
    }

    fn ping_server_identity(&self, socket: &str) -> Result<(String, u32), String> {
        parse_ping_identity(self.request_api(socket, "ping", serde_json::json!({}))?)
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
        self.configure_paths(dir, None);
    }

    pub fn binary_source_info(&self) -> HerdrBinarySourceInfo {
        if let Some(remote) = &self.remote {
            return remote
                .metadata(HerdrMetadata::BinarySource, None)
                .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()))
                .unwrap_or_else(|error| HerdrBinarySourceInfo {
                    reason: Some(error),
                    ..Default::default()
                });
        }
        let configured = *self.configured_source.lock().unwrap();
        let active = *self.active_source.lock().unwrap();
        let custom_path = self.configured_custom_path.lock().unwrap().clone();
        let active_custom_path = self.active_custom_path.lock().unwrap().clone();
        let restart_required = configured != active || custom_path != active_custom_path;
        let (active_path, resolved, active_reason) = self.resolve_binary_selection(active);
        let (configured_path, _configured_resolved, configured_reason) = if !restart_required {
            (active_path.clone(), resolved, active_reason.clone())
        } else if configured == HerdrBinarySource::Custom {
            match checked_custom_binary(custom_path.as_deref()) {
                Ok(path) => (Some(path), Some(configured), None),
                Err(error) => (None, None, Some(error)),
            }
        } else {
            self.resolve_binary_selection(configured)
        };
        let (version, protocol) = active_path
            .as_deref()
            .map(probe_binary_identity)
            .unwrap_or((None, None));
        let (configured_version, configured_protocol) = if !restart_required {
            (version.clone(), protocol)
        } else {
            configured_path
                .as_deref()
                .map(probe_binary_identity)
                .unwrap_or((None, None))
        };
        HerdrBinarySourceInfo {
            custom_path: custom_path.map(display_path),
            configured,
            active,
            resolved,
            available: active_path.is_some(),
            path: active_path.map(display_path),
            reason: active_reason,
            version,
            protocol,
            configured_available: configured_path.is_some(),
            configured_path: configured_path.map(display_path),
            configured_reason,
            configured_version,
            configured_protocol,
            configuration_error: self.binary_source_config_error.lock().unwrap().clone(),
            restart_required,
        }
    }

    pub fn get_binary_source(&self) -> HerdrBinarySource {
        *self.configured_source.lock().unwrap()
    }

    pub fn check_binary_source(
        &self,
        source: HerdrBinarySource,
        custom_path: Option<String>,
    ) -> Result<crate::herdr_runtime::RuntimeBinaryCheck, String> {
        let binary = if source == HerdrBinarySource::Custom {
            let custom_path = custom_path.as_deref().map(normalize_custom_path);
            checked_custom_binary(custom_path.as_deref().map(Path::new))?
        } else {
            let (path, reason) = self.resolve_binary_for_source(source);
            path.ok_or_else(|| reason.unwrap_or_else(|| "herdr-unavailable".into()))?
        };
        crate::herdr_runtime::inspect_local(binary)
    }

    pub fn set_binary_source(
        &self,
        source: HerdrBinarySource,
    ) -> Result<HerdrBinarySourceSetResult, String> {
        self.set_binary_source_with_path(source, None)
    }

    /// Validate every running Session before persisting. Existing connectors keep
    /// their client until Yuzora restarts; this never stops a HERDR server.
    pub fn set_binary_source_with_path(
        &self,
        source: HerdrBinarySource,
        custom_path: Option<String>,
    ) -> Result<HerdrBinarySourceSetResult, String> {
        let _write_guard = self.binary_source_write_lock.lock().unwrap();
        self.check_binary_source(source, custom_path.clone())?
            .require_compatible()?;
        let custom_path = if source == HerdrBinarySource::Custom {
            custom_path
                .as_deref()
                .map(normalize_custom_path)
                .map(PathBuf::from)
        } else {
            None
        };
        let config_dir = self
            .config_dir
            .lock()
            .unwrap()
            .clone()
            .ok_or("herdr config directory is not configured")?;
        save_binary_source_preference(&config_dir, source, custom_path.as_deref())?;
        *self.configured_source.lock().unwrap() = source;
        *self.configured_custom_path.lock().unwrap() = custom_path.clone();
        *self.binary_source_config_error.lock().unwrap() = None;
        let active = *self.active_source.lock().unwrap();
        let restart_required =
            source != active || custom_path != *self.active_custom_path.lock().unwrap();
        Ok(HerdrBinarySourceSetResult {
            configured: source,
            restart_required,
        })
    }

    pub fn resolve_binary(&self) -> Option<PathBuf> {
        if self.remote.is_some() {
            return self.binary_override.lock().unwrap().clone();
        }
        let active = *self.active_source.lock().unwrap();
        self.resolve_binary_selection(active).0
    }

    /// The Yuzora-managed binary whichever source is active: an app update
    /// replaces it even while another HERDR is selected.
    pub fn managed_binary(&self) -> Option<PathBuf> {
        if self.remote.is_some() {
            return None;
        }
        self.resolve_binary_for_source(HerdrBinarySource::Default).0
    }

    /// Resolve exactly the chosen source. An installed selection never falls
    /// back to a managed binary when PATH changes.
    fn resolve_binary_selection(
        &self,
        source: HerdrBinarySource,
    ) -> (Option<PathBuf>, Option<HerdrBinarySource>, Option<String>) {
        let (path, reason) = self.resolve_binary_for_source(source);
        let resolved = path.as_ref().map(|_| source);
        (path, resolved, reason)
    }

    /// Resolve only the selected source; missing installed tools never fall back.
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
            HerdrBinarySource::Custom => {
                match checked_custom_binary(self.active_custom_path.lock().unwrap().as_deref()) {
                    Ok(path) => (Some(path), None),
                    Err(error) => (None, Some(error)),
                }
            }
            HerdrBinarySource::Global => match which_binary("herdr").map(PathBuf::from) {
                Some(path) => (Some(path), None),
                None => (None, Some("herdr-not-found-on-path".into())),
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

    fn capability_probe_lock(&self, cache_key: &str) -> Arc<Mutex<()>> {
        let mut locks = self.capability_probe_locks.lock().unwrap();
        if locks.len() > CAPABILITY_PROBE_LOCK_LIMIT {
            // Only the map holds an unshared lock: nobody is probing with it.
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        locks.entry(cache_key.to_string()).or_default().clone()
    }

    fn active_binary_fingerprint(&self) -> Option<String> {
        if let Some(remote) = &self.remote {
            return remote
                .metadata(HerdrMetadata::BinaryFingerprint, None)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned));
        }
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
        let cache_key = Self::capability_cache_key(session_name);
        let probe_lock = self.capability_probe_lock(&cache_key);
        let _probe_guard = probe_lock.lock().unwrap();
        let mut caps = self.discover_capabilities_for_session(session_name);
        let should_probe = caps.server.running
            && caps.server.socket_path.is_some()
            && caps.server.compatible != Some(false);
        let cache_entry = if should_probe {
            match self.require_running_session_socket(session_name) {
                Ok((session, socket_path)) => match self.ping_server_identity(&socket_path) {
                    Ok(live_identity)
                        if Some(live_identity.clone())
                            == caps.server.version.clone().zip(caps.server.protocol) =>
                    {
                        self.socket_identity
                            .lock()
                            .unwrap()
                            .insert(socket_path.clone(), (Instant::now(), live_identity));
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

    /// Fast path after bootstrap. Session list and socket `ping` are checked
    /// at their validation cadences. A restart, default-session change, protocol change, binary
    /// replacement, or negative cache condition falls back to fresh discovery.
    fn cached_capabilities_for_session(&self, session_name: Option<&str>) -> HerdrCapabilities {
        let current_session = self.require_running_session_socket(session_name).ok();
        self.cached_capabilities_with_session(session_name, current_session)
    }

    /// Same as `cached_capabilities_for_session`, for callers that already
    /// resolved the running Session and its socket (one resolution per request).
    fn cached_capabilities_with_session(
        &self,
        session_name: Option<&str>,
        current_session: Option<(HerdrNamedSession, String)>,
    ) -> HerdrCapabilities {
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
                && cached.binary_fingerprint == self.recent_binary_fingerprint()
                && self.recent_server_identity(&socket_path)
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
        let binary_path = binary_source.path.as_ref().map(PathBuf::from);
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
                workspace_move: false,
                workspace_move_block: false,
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
        let startup_error = if session_name.is_none() {
            self.startup_error.lock().unwrap().clone()
        } else {
            None
        };
        let apply_startup_error = |caps: &mut HerdrCapabilities| {
            if !caps.server.running {
                if let Some(error) = startup_error.as_deref() {
                    let reason = format!("herdr automatic startup failed: {error}");
                    caps.api.reason = Some(reason.clone());
                    caps.terminal.reason = Some(reason.clone());
                    caps.events.reason = Some(reason);
                }
            }
        };

        let Some(_binary) = binary_path else {
            apply_startup_error(&mut caps);
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
                apply_startup_error(&mut caps);
                return caps;
            }
        };
        let session_env = named.as_ref().map(|s| s.name.as_str());

        match self.metadata(HerdrMetadata::Status, session_env) {
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
        match self.metadata(HerdrMetadata::Schema, session_env) {
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

        apply_startup_error(&mut caps);

        caps
    }

    /// `herdr session list --json` — authoritative named-session inventory.
    pub fn list_sessions(&self) -> Result<Vec<HerdrNamedSession>, String> {
        let _refresh = self.session_inventory_refresh_lock.lock().unwrap();
        self.refresh_session_inventory()
    }

    /// Idle-poll variant of `list_sessions`: serves the inventory while it is
    /// younger than `SESSION_POLL_TTL` instead of forking the CLI every tick.
    pub fn list_sessions_polled(&self) -> Result<Vec<HerdrNamedSession>, String> {
        if let Some(sessions) = self.polled_session_inventory() {
            return Ok(sessions);
        }
        let _refresh = self.session_inventory_refresh_lock.lock().unwrap();
        if let Some(sessions) = self.polled_session_inventory() {
            return Ok(sessions);
        }
        self.refresh_session_inventory()
    }

    fn polled_session_inventory(&self) -> Option<Vec<HerdrNamedSession>> {
        let ttl = *self.session_poll_ttl.lock().unwrap();
        let inventory = self.session_inventory.lock().unwrap();
        let (at, sessions) = inventory.as_ref()?;
        (at.elapsed() < ttl).then(|| sessions.clone())
    }

    /// Caller holds only the refresh lock during CLI I/O, never the cache lock.
    fn refresh_session_inventory(&self) -> Result<Vec<HerdrNamedSession>, String> {
        let epoch = self.session_inventory_epoch.load(Ordering::SeqCst);
        let result = self
            .metadata(HerdrMetadata::Sessions, None)
            .and_then(|value| bounded_ipc(parse_session_list_json(&value)?));
        let mut inventory = self.session_inventory.lock().unwrap();
        match result {
            Ok(sessions) => {
                if self.session_inventory_epoch.load(Ordering::SeqCst) == epoch {
                    *inventory = Some((Instant::now(), sessions.clone()));
                }
                Ok(sessions)
            }
            Err(error) => {
                // Warm readers may use the old list while refreshing, not after failure.
                *inventory = None;
                self.session_inventory_epoch.fetch_add(1, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    fn cached_session_inventory(
        &self,
        session_name: Option<&str>,
    ) -> Option<Vec<HerdrNamedSession>> {
        let inventory = self.session_inventory.lock().unwrap();
        let (at, sessions) = inventory.as_ref()?;
        (self.validation_fresh(*at)
            && Self::session_from_inventory(sessions, session_name)
                .is_ok_and(|session| session.running))
        .then(|| sessions.clone())
    }

    /// Serialize refreshes so simultaneous hot requests share one CLI probe.
    /// A cached missing/stopped target gets exactly one fresh list before refusal.
    fn session_inventory(
        &self,
        session_name: Option<&str>,
    ) -> Result<Vec<HerdrNamedSession>, String> {
        if let Some(sessions) = self.cached_session_inventory(session_name) {
            return Ok(sessions);
        }
        let _refresh = self.session_inventory_refresh_lock.lock().unwrap();
        // Another reader or an explicit list may have refreshed while we waited.
        if let Some(sessions) = self.cached_session_inventory(session_name) {
            return Ok(sessions);
        }
        self.refresh_session_inventory()
    }

    /// Resolve a named session from `session list --json` only.
    /// `None` / empty / `"live"` maps to the default session entry.
    pub fn resolve_named_session(
        &self,
        session_name: Option<&str>,
    ) -> Result<HerdrNamedSession, String> {
        let sessions = self.session_inventory(session_name)?;
        Self::session_from_inventory(&sessions, session_name)
    }

    fn session_from_inventory(
        sessions: &[HerdrNamedSession],
        session_name: Option<&str>,
    ) -> Result<HerdrNamedSession, String> {
        if sessions.is_empty() {
            return Err("no herdr named sessions found".into());
        }
        let requested = session_name.map(str::trim).filter(|s| !s.is_empty());
        match requested {
            Some(name) if name != "live" => sessions
                .iter()
                .find(|s| s.name == name)
                .cloned()
                .ok_or_else(|| format!("herdr session not found: {name}")),
            _ => {
                if let Some(default_session) = sessions.iter().find(|s| s.default).cloned() {
                    Ok(default_session)
                } else {
                    sessions
                        .first()
                        .cloned()
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
        let response = self.call_checked_api(
            session_name,
            |api| api.snapshot,
            "session.snapshot",
            serde_json::json!({}),
            "herdr snapshot unavailable",
        )?;
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
        let cwd = cwd.map(crate::shell::working_directory);
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

    /// Public `workspace.move { workspace_id, insert_index }`.
    pub fn workspace_move(
        &self,
        session_name: Option<&str>,
        workspace_id: String,
        insert_index: u32,
    ) -> Result<HerdrWorkspaceOrderResult, String> {
        if workspace_id.trim().is_empty() {
            return Err("workspace_id is required".into());
        }
        let response = self.call_checked_api(
            session_name,
            |api| api.workspace_move,
            "workspace.move",
            build_workspace_move_params(workspace_id, insert_index),
            "herdr workspace.move unavailable",
        )?;
        parse_workspace_order(response)
    }

    /// Newer HERDR runtimes expose the atomic block reorder API. A one-item
    /// block is equivalent to workspace.move and also works when that legacy
    /// method is omitted from a WSL schema.
    pub fn workspace_move_block(
        &self,
        session_name: Option<&str>,
        workspace_ids: Vec<String>,
        before_workspace_id: Option<String>,
    ) -> Result<HerdrWorkspaceOrderResult, String> {
        if workspace_ids.is_empty() || workspace_ids.iter().any(|id| id.trim().is_empty()) {
            return Err("workspace_ids must not be empty".into());
        }
        let mut params = serde_json::json!({ "workspace_ids": workspace_ids });
        if let Some(anchor) = before_workspace_id {
            params["before_workspace_id"] = anchor.into();
        }
        let response = self.call_checked_api(
            session_name,
            |api| api.workspace_move_block,
            "workspace.move_block",
            params,
            "herdr workspace.move_block unavailable",
        )?;
        parse_workspace_order(response)
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

    /// Long-lived `events.subscribe` for Agent status and pane lifecycle.
    pub fn events_subscribe(
        self: &Arc<Self>,
        session_name: Option<String>,
        pane_ids: Vec<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String> {
        if self.remote.is_some() {
            return Err("use-host-events-stream".into());
        }
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
        let req = subscription_request(&request_id, &pane_ids)?;
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
        #[cfg(unix)]
        let stream = Arc::new(stream);
        #[cfg(unix)]
        let reader_socket = Arc::downgrade(&stream);

        // Register ownership before the reader can emit events, so a fast
        // terminal error/release can always find and close this stream.
        let manager_for_thread = Arc::downgrade(self);
        let handle = std::thread::spawn(move || {
            if start_rx.recv().is_err() {
                closed_for_thread.store(true, Ordering::SeqCst);
                return;
            }
            let stream = stream;
            let mut pending = pending;
            loop {
                if closed_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                #[cfg(unix)]
                let deadline = None;
                #[cfg(windows)]
                let deadline = Some(Instant::now() + EVENT_POLL_INTERVAL);
                let result = read_local_ndjson_line_with(
                    &stream,
                    &mut pending,
                    deadline,
                    MAX_NDJSON_LINE_BYTES,
                    LocalWaitProfile::Idle,
                );
                if closed_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                match result {
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
            // Terminal subscriptions no longer own a live reader. Release its
            // socket and callback before retiring the manager's bookkeeping.
            drop(stream);
            drop(on_event_for_thread);
            if let Some(manager) = manager_for_thread.upgrade() {
                manager
                    .event_subscriptions
                    .lock()
                    .unwrap()
                    .remove(&subscription_id_for_thread);
            }
        });

        let subscription = Arc::new(EventSubscription {
            closed,
            reader: Mutex::new(Some(handle)),
            #[cfg(unix)]
            reader_socket,
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
    pub(crate) fn call_checked_api(
        &self,
        session_name: Option<&str>,
        is_available: impl Fn(&HerdrApiCapability) -> bool,
        method: &str,
        params: serde_json::Value,
        unavailable: &str,
    ) -> Result<serde_json::Value, String> {
        let response = (|| {
            // Refuse missing/stopped sessions before capability discovery could
            // redundantly refresh their inventory again.
            let resolved = self.require_running_session_socket(session_name)?;
            let caps = self.cached_capabilities_with_session(session_name, Some(resolved.clone()));
            if !is_available(&caps.api) {
                return Err(caps.api.reason.unwrap_or_else(|| unavailable.into()));
            }
            let (_session, socket) = resolved;
            self.request_api(&socket, method, params)
        })();
        if response.is_err() {
            // A failed request may mean a restarted, replaced or stopped
            // server: the next request rediscovers instead of trusting caches.
            self.invalidate_runtime_caches();
        }
        response
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
        if self.remote.is_some() {
            return Err("use-host-terminal-stream".into());
        }
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
            .ok_or_else(|| HERDR_PATH_BINARY_NOT_FOUND.to_string())?;

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

        let mut cmd = terminal_connector_command(&binary, &args, &named.name);
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
        if session_id.starts_with("herdr-client-") {
            return self.native_client_input(session_id, text, bytes_base64);
        }
        let cmd = TerminalControlCommand::input(text, bytes_base64)?;
        self.send_control(session_id, &cmd)
    }

    pub fn terminal_resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        if session_id.starts_with("herdr-client-") {
            return self.native_client_resize(session_id, cols, rows);
        }
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
        column: Option<u16>,
        row: Option<u16>,
    ) -> Result<(), String> {
        let cmd = TerminalControlCommand::scroll(direction, lines, column, row)?;
        self.send_control(session_id, &cmd)
    }

    pub fn terminal_mouse(
        &self,
        session_id: &str,
        action: HerdrMouseAction,
        column: u16,
        row: u16,
        modifiers: u8,
    ) -> Result<(), String> {
        let cmd = TerminalControlCommand::Mouse {
            action,
            column,
            row,
            modifiers,
        };
        self.send_control(session_id, &cmd)
    }

    /// Release only the Yuzora-owned connector child for this session.
    pub fn terminal_release(&self, session_id: &str) -> Result<(), String> {
        if session_id.starts_with("herdr-client-") {
            return self.release_native_client(session_id);
        }
        let session = {
            let mut map = self.sessions.lock().unwrap();
            map.remove(session_id)
                .ok_or_else(|| format!("no herdr terminal session {session_id}"))?
        };
        release_connector(&session)
    }

    /// Shutdown path: drop every Yuzora connector child. Never stops Herdr server/panes.
    pub fn release_all_connectors(&self) {
        let clients: Vec<_> = self
            .native_clients
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for id in clients {
            let _ = self.release_native_client(&id);
        }
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

/// Serialized-size gate for a terminal event. A frame's base64 payload is
/// accounted from its length (it dominates the size and serializes verbatim
/// when no byte needs JSON escaping); anything else, or a payload that does
/// need escaping, is measured by serializing as before.
fn ensure_terminal_event_bound(event: &mut HerdrTerminalEvent) -> Result<(), HerdrProtocolError> {
    if let HerdrTerminalEvent::Frame { bytes_base64, .. } = event {
        let clean = bytes_base64.as_bytes().chunks(64).all(|chunk| {
            !chunk.iter().fold(false, |bad, &b| {
                bad | (b < 0x20) | (b == b'"') | (b == b'\\')
            })
        });
        if clean {
            let payload = std::mem::take(bytes_base64);
            let shell = serde_json::to_vec(&*event).map_err(|_| HerdrProtocolError::InvalidJson);
            if let HerdrTerminalEvent::Frame { bytes_base64, .. } = event {
                *bytes_base64 = payload;
            }
            let shell = shell?;
            let payload_len = match event {
                HerdrTerminalEvent::Frame { bytes_base64, .. } => bytes_base64.len(),
                _ => 0,
            };
            if shell.len().saturating_add(payload_len) > MAX_IPC_BYTES {
                return Err(HerdrProtocolError::ResponseTooLarge);
            }
            return Ok(());
        }
    }
    ensure_ipc_bound(event)
}

fn emit_terminal_event(
    on_event: &OnTerminalEvent,
    mut event: HerdrTerminalEvent,
) -> Result<(), String> {
    if let Err(error) = ensure_terminal_event_bound(&mut event) {
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
        let wire = match parse_wire_line(trimmed) {
            Ok(wire) => wire,
            Err(WireLineError::Parse(message)) => {
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Error {
                        session_id: session.id.clone(),
                        code: "frame_parse".into(),
                        message,
                    },
                );
                continue;
            }
            Err(WireLineError::TooComplex(error)) => {
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
        };

        match tracker.ingest_wire_owned(wire) {
            FrameDecision::InvalidGeometry => {
                let _ = emit_terminal_event(
                    &on_event,
                    HerdrTerminalEvent::Error {
                        session_id: session.id.clone(),
                        code: "invalid-terminal-geometry".into(),
                        message: "terminal frame dimensions exceed the supported limits".into(),
                    },
                );
                break;
            }
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

/// Strip one pair of matching surrounding quotes left by "Copy as path" or a
/// hand-typed value. Unpaired quotes and nested pairs are kept.
pub fn normalize_custom_path(raw: &str) -> String {
    // The same quote pairs as the frontend's sanitizeCustomPath, including smart quotes.
    const PAIRS: [(char, char); 4] = [
        ('"', '"'),
        ('\'', '\''),
        ('\u{201c}', '\u{201d}'),
        ('\u{2018}', '\u{2019}'),
    ];
    let trimmed = raw.trim();
    let mut chars = trimmed.chars();
    if let (Some(first), Some(last)) = (chars.next(), chars.next_back()) {
        if PAIRS.contains(&(first, last)) {
            return trimmed[first.len_utf8()..trimmed.len() - last.len_utf8()]
                .trim()
                .to_string();
        }
    }
    trimmed.to_string()
}

/// Windows can only run `.exe` here; `.cmd`/`.bat` shims are not spawnable.
pub fn require_exe_on_windows(path: &Path, windows: bool) -> Result<(), String> {
    if windows
        && !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err(format!("herdr-custom-path-not-exe: {}", path.display()));
    }
    Ok(())
}

fn checked_custom_binary(path: Option<&Path>) -> Result<PathBuf, String> {
    let path = path.ok_or("herdr-custom-path-required")?;
    let path = PathBuf::from(normalize_custom_path(&path.to_string_lossy()));
    if !path.is_absolute() {
        return Err(format!(
            "herdr-custom-path-not-executable: {}",
            path.display()
        ));
    }
    require_exe_on_windows(&path, cfg!(windows))?;
    if !is_executable(&path) {
        return Err(format!(
            "herdr-custom-path-not-executable: {}",
            path.display()
        ));
    }
    Ok(path)
}

/// Drop the Windows verbatim prefix (`\\?\C:\` and `\\?\UNC\`) so paths shown to
/// the user match what Explorer prints. Operates on text, on every platform.
pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    match strip_verbatim_text(&path.to_string_lossy()) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

/// Text form of [`strip_verbatim_prefix`]; `None` when nothing needs stripping.
pub fn strip_verbatim_text(text: &str) -> Option<String> {
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{share}"));
    }
    let rest = text.strip_prefix(r"\\?\")?;
    let bytes = rest.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| rest.to_string())
}

fn display_path(path: PathBuf) -> String {
    strip_verbatim_prefix(path).to_string_lossy().into_owned()
}

fn managed_binary_path(resource_dir: &Path) -> PathBuf {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let relative = PathBuf::from(format!("{os}-{arch}")).join(if cfg!(windows) {
        "herdr.exe"
    } else {
        "herdr"
    });
    let primary = resource_dir.join("herdr").join(&relative);
    if primary.is_file() {
        primary
    } else {
        resource_dir.join("host").join(relative)
    }
}

fn binary_source_config_path(config_dir: &Path) -> PathBuf {
    config_dir.join(BINARY_SOURCE_CONFIG_FILE)
}

struct BinarySourcePreferenceLoad {
    source: HerdrBinarySource,
    custom_path: Option<PathBuf>,
    error: Option<String>,
}

fn load_binary_source_preference(config_dir: &Path) -> BinarySourcePreferenceLoad {
    let path = binary_source_config_path(config_dir);
    let fallback = |error| BinarySourcePreferenceLoad {
        source: HerdrBinarySource::Default,
        custom_path: None,
        error,
    };
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return fallback(None),
        Err(error) => {
            return fallback(Some(format!(
                "failed to read Herdr binary-source preference at {}: {error}",
                path.display()
            )))
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            return fallback(Some(format!(
                "invalid Herdr binary-source preference at {}: {error}",
                path.display()
            )))
        }
    };
    let source = match value.get("binarySource").and_then(|v| v.as_str()) {
        Some("global") => HerdrBinarySource::Global,
        Some("default") => HerdrBinarySource::Default,
        Some("custom") => HerdrBinarySource::Custom,
        other => {
            return fallback(Some(format!(
                "unknown Herdr binarySource {other:?} in {}",
                path.display()
            )))
        }
    };
    let raw_custom_path = value.get("customPath").and_then(|v| v.as_str());
    let custom_path = raw_custom_path.map(|raw| PathBuf::from(normalize_custom_path(raw)));
    if source == HerdrBinarySource::Custom {
        // Preferences saved before normalization may carry quotes: rewrite them.
        if let (Some(raw), Some(normalized)) = (raw_custom_path, custom_path.as_deref()) {
            if normalized.is_absolute() && normalized.to_string_lossy() != raw {
                let _ = save_binary_source_preference(config_dir, source, Some(normalized));
            }
        }
    }
    if source == HerdrBinarySource::Custom && !custom_path.as_deref().is_some_and(Path::is_absolute)
    {
        return fallback(Some("invalid Herdr custom binary path".into()));
    }
    BinarySourcePreferenceLoad {
        source,
        custom_path,
        error: None,
    }
}

fn save_binary_source_preference(
    config_dir: &Path,
    source: HerdrBinarySource,
    custom_path: Option<&Path>,
) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|e| format!("failed to create herdr config dir: {e}"))?;
    let path = binary_source_config_path(config_dir);
    let value = serde_json::json!({
        "binarySource": match source {
            HerdrBinarySource::Global => "global",
            HerdrBinarySource::Default => "default",
            HerdrBinarySource::Custom => "custom",
        },
        "customPath": custom_path.map(|path| path.to_string_lossy().into_owned())
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

/// Version reported by `herdr status --json`, for display only.
pub fn probe_binary_version(binary: &Path) -> Option<String> {
    probe_binary_identity(binary).0
}

/// Like [`probe_binary_version`] but bounded by `timeout`; a timeout or any
/// failure yields `None`.
pub fn probe_binary_version_with_timeout(binary: &Path, timeout: Duration) -> Option<String> {
    let binary = binary.to_path_buf();
    // Killing and reaping a hung probe can outlast `timeout`; bound the whole probe.
    within(timeout, move || {
        let status =
            run_herdr_json_with_session_timeout(&binary, &["status", "--json"], None, timeout)
                .ok()?;
        let client = status.get("client").unwrap_or(&status);
        client
            .get("version")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    })
}

/// Run `probe` on its own thread and give up after `timeout`; a slow tail finishes detached.
fn within<T: Send + 'static>(
    timeout: Duration,
    probe: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(probe());
    });
    receiver.recv_timeout(timeout).ok().flatten()
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
    #[cfg(unix)]
    if let Some(stream) = subscription.reader_socket.upgrade() {
        let LocalStream::UdSocket(socket) = &*stream;
        let _ = socket.inner().shutdown(std::net::Shutdown::Both);
    }
    if let Some(handle) = subscription.reader.lock().unwrap().take() {
        let _ = handle.join();
    }
}

pub fn subscription_request(id: &str, pane_ids: &[String]) -> Result<serde_json::Value, String> {
    if pane_ids.len() > MAX_PANE_COUNT || pane_ids.iter().any(|id| id.is_empty() || id.len() > 256)
    {
        return Err("invalid-subscription-panes".into());
    }
    let mut request = serde_json::json!({
        "id": id,
        "method": "events.subscribe",
        "params": {
            "subscriptions": [
                { "type": "pane.created" },
                { "type": "pane.closed" },
                { "type": "pane.moved" },
                { "type": "pane.updated" },
                { "type": "pane.agent_detected" },
                { "type": "pane.exited" },
                { "type": "worktree.created" },
                { "type": "worktree.opened" },
                { "type": "worktree.removed" },
                { "type": "tab.created" },
                { "type": "tab.closed" },
                { "type": "tab.moved" },
                { "type": "tab.renamed" },
                { "type": "workspace.created" },
                { "type": "workspace.renamed" },
                { "type": "workspace.closed" },
                { "type": "workspace.moved" },
                { "type": "workspace.reordered" }
            ]
        }
    });
    // Official protocol 22 has no wildcard agent-status selector. Snapshot pane
    // IDs scope these subscriptions; the client replaces them when panes change.
    let subscriptions = request["params"]["subscriptions"].as_array_mut().unwrap();
    for pane_id in pane_ids.iter().collect::<std::collections::BTreeSet<_>>() {
        subscriptions
            .push(serde_json::json!({ "type": "pane.agent_status_changed", "pane_id": pane_id }));
    }
    Ok(request)
}

pub fn parse_subscription_event_line(
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
            "pane.created" | "pane_created" => Some("pane.created"),
            "pane.closed" | "pane_closed" => Some("pane.closed"),
            "pane.moved" | "pane_moved" => Some("pane.moved"),
            "pane.updated" | "pane_updated" => Some("pane.updated"),
            "pane.agent_detected" | "pane_agent_detected" => Some("pane.agent_detected"),
            "tab.created" | "tab_created" => Some("tab.created"),
            "tab.closed" | "tab_closed" => Some("tab.closed"),
            "tab.moved" | "tab_moved" => Some("tab.moved"),
            "tab.renamed" | "tab_renamed" => Some("tab.renamed"),
            "workspace.created" | "workspace_created" => Some("workspace.created"),
            "workspace.renamed" | "workspace_renamed" => Some("workspace.renamed"),
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
        state_labels,
    }))
}

#[cfg(test)]
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

/// Windows only launches `.exe` here: `herdr.cmd`/`.bat` shims and extensionless
/// files on PATH are not spawnable, so they are never candidates.
pub fn resolve_in_dirs(
    dirs: &[PathBuf],
    command: &str,
    windows: bool,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let file_name = if !windows || command.to_ascii_lowercase().ends_with(".exe") {
        command.to_string()
    } else {
        format!("{command}.exe")
    };
    dirs.iter()
        .map(|dir| dir.join(&file_name))
        .find(|candidate| is_file(candidate))
}

fn which_binary(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    resolve_in_dirs(&dirs, command, cfg!(windows), is_executable)
        .map(|found| found.to_string_lossy().into_owned())
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

/// Variables a HERDR pane exports to its children. HERDR prefers an inherited
/// `HERDR_SOCKET_PATH` over `HERDR_SESSION`, so Yuzora launched from a pane
/// (`tauri dev`, `open`) would otherwise send its HERDR children to that
/// pane's server instead of the Session it selected (#132).
pub(crate) const PARENT_PANE_HERDR_ENV: [&str; 6] = [
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_ENV",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
];

/// Routes a HERDR child to `session`, or to the default Session for `None`,
/// whatever environment Yuzora itself inherited.
pub(crate) fn pin_herdr_session(command: &mut Command, session: Option<&str>) {
    for key in PARENT_PANE_HERDR_ENV {
        command.env_remove(key);
    }
    match session {
        Some(name) => command.env("HERDR_SESSION", name),
        None => command.env_remove("HERDR_SESSION"),
    };
}

fn default_server_command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("server")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    pin_herdr_session(&mut command, None);
    command
}

/// Official connector child: bind to the named session via HERDR_SESSION.
/// Default session remains valid. Never guess socket paths here.
fn terminal_connector_command(binary: &Path, args: &[String], session: &str) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pin_herdr_session(&mut cmd, Some(session));
    cmd
}

fn herdr_cli_command(binary: &Path, args: &[&str], session_name: Option<&str>) -> Command {
    let mut cmd = Command::new(binary);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    pin_herdr_session(&mut cmd, session_name.filter(|s| !s.trim().is_empty()));
    cmd
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
    let mut cmd = herdr_cli_command(binary, args, session_name);
    process_kill::configure_background_process(&mut cmd);
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

pub(crate) fn wait_bounded_child(
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

pub fn parse_session_list_json(
    value: &serde_json::Value,
) -> Result<Vec<HerdrNamedSession>, String> {
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
    "workspace.move",
    "workspace.move_block",
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
    "pane.scroll",
    "pane.process_info",
    "layout.export",
    "layout.set_split_ratio",
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
    api.workspace_move = false;
    api.workspace_move_block = false;
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
    api.workspace_move = has("workspace.move");
    api.workspace_move_block = has("workspace.move_block");
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
    api.events_subscribe = has("events.subscribe");
    api.worktree_list = has("worktree.list");

    let mut methods = Vec::new();
    for name in IMPLEMENTED_API_METHODS.iter().chain(FEATURE_API_METHODS) {
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
pub(crate) fn collect_schema_methods(schema: &serde_json::Value) -> HashSet<String> {
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
    if name == "ping" {
        return true;
    }
    // Nested public methods include plugin.pane.open and plugin.action.invoke.
    name.contains('.')
        && name.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HerdrServerStartupStatus {
    running: bool,
    compatible: Option<bool>,
}

fn query_herdr_server_startup_status(
    binary: &Path,
    timeout: Duration,
) -> Result<HerdrServerStartupStatus, String> {
    let status = run_herdr_json_with_session_timeout(binary, &["status", "--json"], None, timeout)?;
    let server = status.get("server").unwrap_or(&status);
    Ok(HerdrServerStartupStatus {
        running: status_reports_server_running(&status),
        compatible: server.get("compatible").and_then(|value| value.as_bool()),
    })
}

fn query_herdr_server_running(binary: &Path, timeout: Duration) -> Result<bool, String> {
    Ok(query_herdr_server_startup_status(binary, timeout)?.running)
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

fn parse_ping_identity(response: serde_json::Value) -> Result<(String, u32), String> {
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
    api_request_with_timeout(socket_path, method, params, LOCAL_IO_TIMEOUT)
}

fn api_request_with_timeout(
    socket_path: &str,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + timeout;
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
    validate_api_response_sized(value, response.len())
}

pub fn validate_api_response(value: serde_json::Value) -> Result<serde_json::Value, String> {
    ensure_ipc_bound(&value).map_err(String::from)?;
    check_api_response(value)
}

/// `validate_api_response` for a value parsed from a line of `raw_len` bytes:
/// the size gate uses the line already read instead of re-serializing.
pub fn validate_api_response_sized(
    value: serde_json::Value,
    raw_len: usize,
) -> Result<serde_json::Value, String> {
    ensure_raw_ipc_bound(raw_len).map_err(String::from)?;
    check_api_response(value)
}

fn check_api_response(value: serde_json::Value) -> Result<serde_json::Value, String> {
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

fn parse_snapshot_response(mut response: serde_json::Value) -> Result<HerdrSnapshotResult, String> {
    let result = response
        .get_mut("result")
        .ok_or_else(|| "snapshot response missing result".to_string())?;
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if result_type != "session_snapshot" && result.get("snapshot").is_none() {
        return Err(format!("unexpected snapshot result type: {result_type}"));
    }
    // Move the (large) snapshot out of the response instead of deep-cloning it.
    let snapshot = result
        .get_mut("snapshot")
        .map(serde_json::Value::take)
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
    let cwd = cwd.map(crate::shell::working_directory);
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

fn build_workspace_move_params(workspace_id: String, insert_index: u32) -> serde_json::Value {
    serde_json::json!({ "workspace_id": workspace_id, "insert_index": insert_index })
}

fn build_pane_split_params(
    direction: HerdrSplitDirection,
    target_pane_id: Option<String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
    ratio: Option<f64>,
    focus: bool,
) -> serde_json::Value {
    let cwd = cwd.map(crate::shell::working_directory);
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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod within_tests {
    use super::*;

    #[test]
    fn herdr_bounded_probe_returns_within_its_timeout() {
        let started = std::time::Instant::now();
        let slow = within(Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_secs(2));
            Some(1)
        });
        assert_eq!(slow, None);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(within(Duration::from_secs(1), || Some(2)), Some(2));
    }
}

#[cfg(test)]
mod custom_path_quote_tests {
    use super::*;

    #[test]
    fn herdr_normalize_custom_path_strips_smart_quote_pairs() {
        for (raw, expected) in [
            ("\u{201c}C:\\a b\\herdr.exe\u{201d}", "C:\\a b\\herdr.exe"),
            ("\u{2018}/opt/herdr\u{2019}", "/opt/herdr"),
            ("\u{201c}/opt/herdr", "\u{201c}/opt/herdr"),
            ("\u{201c}", "\u{201c}"),
        ] {
            assert_eq!(normalize_custom_path(raw), expected, "{raw:?}");
        }
    }
}

#[cfg(test)]
mod tests;

#[path = "herdr_features.rs"]
mod features;
pub use features::*;

#[path = "herdr_native_client.rs"]
mod native_client;
pub use native_client::HerdrClientSize;
