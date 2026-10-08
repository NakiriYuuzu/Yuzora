//! HERDR machines (saved SSH machines) management through the official CLI.
//!
//! Everything here runs on the local manager only and never touches the
//! official `endpoints.json`/`endpoint-selection.json` catalog directly: all
//! mutations go through `herdr machine …` so the catalog and SSH ControlPath
//! stay shared with the user's own terminal.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::herdr_limits::{
    validate_json_complexity, validate_snapshot_counts, MAX_NDJSON_LINE_BYTES,
};
use crate::herdr_service::{wait_bounded_child, HerdrClientSize, HerdrManager, OnTerminalEvent};
use crate::process_kill;

#[cfg(test)]
mod tests;

pub const MIN_MACHINES_VERSION: (u32, u32, u32) = (0, 9, 2);
const MAX_CONCURRENT_MACHINE_PROCESSES: usize = 3;
const MAX_TARGET_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 128;
const MAX_SESSION_BYTES: usize = 64;
const MAX_DETAIL_BYTES: usize = 4096;

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachinesCapabilities {
    pub binary_path: String,
    pub version: Option<String>,
    pub supported: bool,
    pub has_status: bool,
    pub has_reconnect: bool,
    /// False when a subcommand probe could not finish (timeout, signal, spawn
    /// failure, oversized output): the frontend asks again later.
    pub probes_complete: bool,
    pub source: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachine {
    pub id: String,
    pub label: String,
    pub target: String,
    pub session: String,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineStatus {
    pub id: String,
    pub label: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineWorkspace {
    pub workspace_id: String,
    pub label: Option<String>,
    pub repo_name: Option<String>,
    pub checkout_path: Option<String>,
    pub agent_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineAgent {
    pub terminal_id: String,
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub workspace_label: Option<String>,
    pub agent: Option<String>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub folder: Option<String>,
    pub status: String,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineSnapshot {
    pub machine_id: String,
    pub fetched_at: u64,
    pub server_version: Option<String>,
    pub workspaces: Vec<HerdrMachineWorkspace>,
    pub agents: Vec<HerdrMachineAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HerdrMachineInteractiveSpec {
    Add {
        target: String,
        remote_session: Option<String>,
        label: Option<String>,
    },
    Reconnect {
        machine_id: String,
    },
    Client,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrMachineInteractiveOpened {
    pub session_id: String,
}

// ── Validation ──────────────────────────────────────────────────────────────

fn error(code: &str, detail: &str) -> String {
    if detail.is_empty() {
        code.to_string()
    } else {
        format!("{code}: {detail}")
    }
}

pub fn validate_machine_id(id: &str) -> Result<(), String> {
    if id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err("machines-invalid-id".into())
    }
}

pub fn validate_machine_target(target: &str) -> Result<(), String> {
    let invalid = |detail: &str| Err(error("machines-invalid-target", detail));
    if target.trim().is_empty() {
        return invalid("empty");
    }
    if target.len() > MAX_TARGET_BYTES {
        return invalid("too-long");
    }
    if target.trim_start().starts_with('-') {
        return invalid("leading-dash");
    }
    if target.chars().any(char::is_control) {
        return invalid("control-character");
    }
    let rest = target.split_once("://").map_or(target, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    if let Some((userinfo, _)) = authority.rsplit_once('@') {
        if userinfo.contains(':') {
            return invalid("embedded-password");
        }
    }
    Ok(())
}

pub fn validate_machine_label(label: &str) -> Result<(), String> {
    let label = label.trim();
    if label.is_empty() {
        return Err(error("machines-invalid-label", "empty"));
    }
    if label.len() > MAX_LABEL_BYTES {
        return Err(error("machines-invalid-label", "too-long"));
    }
    if label.chars().any(char::is_control) {
        return Err(error("machines-invalid-label", "control-character"));
    }
    Ok(())
}

pub fn validate_machine_session(session: &str) -> Result<(), String> {
    let valid = !session.is_empty()
        && session.len() <= MAX_SESSION_BYTES
        && session != "."
        && session != ".."
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err("machines-invalid-session".into())
    }
}

// ── argv ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineOp<'a> {
    List,
    Status(&'a str),
    Agents(&'a str),
    Rename(&'a str, &'a str),
    Enable(&'a str),
    Disable(&'a str),
    Remove(&'a str),
    Capability,
    ReconnectCapability,
}

/// Argv for the non-interactive operations. Inputs must already be validated.
pub fn build_machine_argv(op: &MachineOp<'_>) -> Vec<String> {
    let v = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match op {
        MachineOp::List => v(&["machine", "list", "--json"]),
        MachineOp::Status(id) => v(&["machine", "status", id, "--json"]),
        MachineOp::Agents(id) => v(&["--machine", id, "api", "snapshot"]),
        MachineOp::Rename(id, label) => v(&["machine", "rename", id, "--label", label.trim()]),
        MachineOp::Enable(id) => v(&["machine", "enable", id]),
        MachineOp::Disable(id) => v(&["machine", "disable", id]),
        MachineOp::Remove(id) => v(&["machine", "remove", id]),
        MachineOp::Capability => v(&["machine", "status", "--help"]),
        MachineOp::ReconnectCapability => v(&["machine", "reconnect", "--help"]),
    }
}

/// Validate a PTY spec and build its argv. The target is always the first
/// positional argument, ahead of every fixed flag.
pub fn build_interactive_args(
    spec: &HerdrMachineInteractiveSpec,
    windows: bool,
) -> Result<Vec<String>, String> {
    match spec {
        HerdrMachineInteractiveSpec::Add {
            target,
            remote_session,
            label,
        } => {
            validate_machine_target(target)?;
            let mut args = vec!["machine".to_string(), "add".to_string(), target.clone()];
            if let Some(session) = remote_session {
                validate_machine_session(session)?;
                args.push("--remote-session".into());
                args.push(session.clone());
            }
            if let Some(label) = label {
                validate_machine_label(label)?;
                args.push("--label".into());
                args.push(label.trim().to_string());
            }
            Ok(args)
        }
        HerdrMachineInteractiveSpec::Reconnect { machine_id } => {
            if windows {
                return Err("machines-reconnect-unsupported-windows".into());
            }
            validate_machine_id(machine_id)?;
            Ok(vec![
                "machine".into(),
                "reconnect".into(),
                machine_id.clone(),
            ])
        }
        HerdrMachineInteractiveSpec::Client => Ok(vec!["client".into()]),
    }
}

// ── Version ─────────────────────────────────────────────────────────────────

/// Parse `herdr 0.9.3` (or a bare/suffixed version) into `(major, minor, patch)`.
pub fn parse_version(output: &str) -> Option<(u32, u32, u32)> {
    let token = output.split_whitespace().find(|t| {
        t.trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
    })?;
    let token = token.trim_start_matches('v');
    let mut parts = token.split('.');
    let number = |part: Option<&str>| -> Option<u32> {
        let digits: String = part?.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let major = number(parts.next())?;
    let minor = number(parts.next())?;
    let patch = number(parts.next()).unwrap_or(0);
    Some((major, minor, patch))
}

pub fn version_at_least(version: (u32, u32, u32), min: (u32, u32, u32)) -> bool {
    version >= min
}

// ── Output parsing ──────────────────────────────────────────────────────────

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&next) {
                    break;
                }
            }
        } else {
            chars.next();
        }
    }
    out
}

fn parse_json_value(stdout: &[u8]) -> Result<serde_json::Value, String> {
    if stdout.len() > MAX_NDJSON_LINE_BYTES {
        return Err("machines-output-too-large".into());
    }
    let text =
        std::str::from_utf8(stdout).map_err(|_| error("machines-parse-failed", "invalid-utf8"))?;
    let cleaned = strip_ansi(text);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err(error("machines-parse-failed", "empty-output"));
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| error("machines-parse-failed", &format!("invalid-json: {e}")))?;
    validate_json_complexity(&value).map_err(|e| error("machines-parse-failed", &e.to_string()))?;
    Ok(value)
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn non_empty_field(value: &serde_json::Value, key: &str) -> Option<String> {
    str_field(value, key).filter(|s| !s.is_empty())
}

/// Result of parsing `machine list --json`: rows missing required fields are
/// dropped and counted instead of failing the whole list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMachineList {
    pub machines: Vec<HerdrMachine>,
    pub dropped: usize,
}

fn machine_from_value(value: &serde_json::Value) -> Option<HerdrMachine> {
    let id = str_field(value, "id")?;
    validate_machine_id(&id).ok()?;
    let label = str_field(value, "label")?;
    let target = str_field(value, "target")?;
    let session = str_field(value, "session")
        .or_else(|| str_field(value, "remote_session"))
        .unwrap_or_default();
    Some(HerdrMachine {
        id,
        label,
        target,
        session,
        enabled: value
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        selected: value
            .get("selected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

pub fn parse_machine_list(stdout: &[u8]) -> Result<ParsedMachineList, String> {
    let value = parse_json_value(stdout)?;
    let rows = match &value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(map) => match map.get("machines") {
            Some(serde_json::Value::Array(rows)) => rows,
            _ => return Err(error("machines-parse-failed", "list-is-not-an-array")),
        },
        _ => return Err(error("machines-parse-failed", "list-is-not-an-array")),
    };
    let mut machines = Vec::with_capacity(rows.len());
    let mut dropped = 0;
    for row in rows {
        match machine_from_value(row) {
            Some(machine) => machines.push(machine),
            None => dropped += 1,
        }
    }
    Ok(ParsedMachineList { machines, dropped })
}

fn normalize_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "reachable" => "reachable",
        "auth required" | "auth-required" | "auth_required" => "auth-required",
        "disabled" => "disabled",
        _ => "error",
    }
}

/// Parse `machine status <id> --json`. The command may report one object, an
/// array, or `{machines:[…]}`; only the entry for `machine_id` counts.
pub fn parse_machine_status(
    stdout: &[u8],
    machine_id: &str,
    fallback_label: &str,
) -> Result<HerdrMachineStatus, String> {
    let value = parse_json_value(stdout)?;
    let entry = match &value {
        serde_json::Value::Array(rows) => pick_status_row(rows, machine_id),
        serde_json::Value::Object(map) => match map.get("machines") {
            Some(serde_json::Value::Array(rows)) => pick_status_row(rows, machine_id),
            _ => Some(&value),
        },
        _ => None,
    }
    .ok_or_else(|| error("machines-parse-failed", "status-entry-missing"))?;
    let raw_status = str_field(entry, "status")
        .ok_or_else(|| error("machines-parse-failed", "status-field-missing"))?;
    let status = normalize_status(&raw_status);
    let mut error_text = str_field(entry, "error").filter(|s| !s.is_empty());
    if status == "error" && error_text.is_none() {
        error_text = Some(raw_status.clone());
    }
    Ok(HerdrMachineStatus {
        id: machine_id.to_string(),
        label: str_field(entry, "label").unwrap_or_else(|| fallback_label.to_string()),
        status: status.to_string(),
        error: error_text,
    })
}

fn pick_status_row<'a>(
    rows: &'a [serde_json::Value],
    machine_id: &str,
) -> Option<&'a serde_json::Value> {
    rows.iter()
        .find(|row| row.get("id").and_then(|v| v.as_str()) == Some(machine_id))
        .or(if rows.len() == 1 { rows.first() } else { None })
}

/// Last path segment, tolerant of `\`, `/` and trailing separators.
pub fn folder_name(path: &str) -> Option<String> {
    path.split(['/', '\\'])
        .rfind(|segment| !segment.is_empty())
        .map(str::to_string)
}

fn agent_status(value: Option<&serde_json::Value>) -> &'static str {
    match value.and_then(|v| v.as_str()) {
        Some("idle") => "idle",
        Some("working") => "working",
        Some("blocked") => "blocked",
        Some("done") => "done",
        _ => "unknown",
    }
}

pub fn parse_snapshot(
    stdout: &[u8],
    machine_id: &str,
    fetched_at: u64,
) -> Result<HerdrMachineSnapshot, String> {
    let value = parse_json_value(stdout)?;
    let snapshot = value
        .get("result")
        .and_then(|r| r.get("snapshot"))
        .or_else(|| value.get("snapshot"))
        .ok_or_else(|| error("machines-parse-failed", "snapshot-missing"))?;
    validate_snapshot_counts(snapshot)
        .map_err(|e| error("machines-parse-failed", &e.to_string()))?;
    let empty = Vec::new();
    let array = |key: &str| {
        snapshot
            .get(key)
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
    };
    let mut workspaces = Vec::new();
    for ws in array("workspaces") {
        let Some(workspace_id) = non_empty_field(ws, "workspace_id") else {
            continue;
        };
        let worktree = ws.get("worktree");
        workspaces.push(HerdrMachineWorkspace {
            workspace_id,
            label: non_empty_field(ws, "label"),
            repo_name: worktree.and_then(|w| non_empty_field(w, "repo_name")),
            checkout_path: worktree.and_then(|w| non_empty_field(w, "checkout_path")),
            agent_status: agent_status(ws.get("agent_status")).to_string(),
        });
    }
    let panes_by_id: HashMap<&str, &serde_json::Value> = array("panes")
        .iter()
        .filter_map(|pane| Some((pane.get("pane_id")?.as_str()?, pane)))
        .collect();
    let mut agents = Vec::new();
    for agent in array("agents") {
        let (Some(pane_id), Some(workspace_id)) = (
            non_empty_field(agent, "pane_id"),
            non_empty_field(agent, "workspace_id"),
        ) else {
            continue;
        };
        let pane = panes_by_id.get(pane_id.as_str()).copied();
        let from_agent_or_pane = |key: &str| {
            non_empty_field(agent, key).or_else(|| pane.and_then(|p| non_empty_field(p, key)))
        };
        let workspace = workspaces.iter().find(|w| w.workspace_id == workspace_id);
        let cwd = from_agent_or_pane("cwd");
        let folder_source = cwd
            .clone()
            .or_else(|| from_agent_or_pane("foreground_cwd"))
            .or_else(|| workspace.and_then(|w| w.checkout_path.clone()));
        agents.push(HerdrMachineAgent {
            terminal_id: non_empty_field(agent, "terminal_id").unwrap_or_else(|| pane_id.clone()),
            tab_id: non_empty_field(agent, "tab_id").unwrap_or_default(),
            workspace_label: workspace.and_then(|w| w.label.clone()),
            agent: non_empty_field(agent, "display_agent")
                .or_else(|| non_empty_field(agent, "agent")),
            name: non_empty_field(agent, "name"),
            title: non_empty_field(agent, "title")
                .or_else(|| non_empty_field(agent, "terminal_title_stripped"))
                .or_else(|| non_empty_field(agent, "terminal_title")),
            folder: folder_source.as_deref().and_then(folder_name),
            cwd,
            status: agent_status(agent.get("agent_status")).to_string(),
            focused: agent
                .get("focused")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            pane_id,
            workspace_id,
        });
    }
    Ok(HerdrMachineSnapshot {
        machine_id: machine_id.to_string(),
        fetched_at,
        server_version: non_empty_field(snapshot, "version"),
        workspaces,
        agents,
    })
}

// ── Error classification ────────────────────────────────────────────────────

fn sanitize_detail(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        let c = if c == '\n' || c == '\r' || c == '\t' {
            ' '
        } else if c.is_control() {
            continue;
        } else {
            c
        };
        if out.len() + c.len_utf8() > MAX_DETAIL_BYTES {
            break;
        }
        out.push(c);
    }
    out
}

/// Pull the message out of Rust's `Debug` rendering of `io::Error::Custom`:
/// `Error: Custom { kind: Other, error: "…" }`, undoing `\"`/`\\` escapes.
fn extract_custom_error(stderr: &str) -> Option<String> {
    let start = stderr.find("error: \"")? + "error: \"".len();
    let mut out = String::new();
    let mut chars = stderr[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    // Unterminated quote: keep what we have rather than losing the cause.
    Some(out)
}

fn classify_message(message: &str, raw_detail: &str) -> (String, String) {
    let lower = message.to_lowercase();
    let host_key = lower.contains("host key verification failed")
        || lower.contains("identification has changed");
    let code = if !host_key
        && ((lower.contains("permission denied")
            && (lower.contains("(publickey")
                || lower.contains("(keyboard-interactive")
                || lower.contains("(password")))
            || lower.contains("signing failed"))
    {
        "machines-auth-required"
    } else if host_key {
        "machines-host-key"
    } else if lower.contains("does not support machine api forwarding") {
        "machines-remote-incompatible"
    } else if lower.contains("failed to connect to remote herdr api socket") {
        "machines-remote-server-stopped"
    } else {
        // Connection refused / timed out / DNS / no route, and any other
        // transport failure surfaced through `Custom`.
        "machines-unreachable"
    };
    (code.to_string(), raw_detail.to_string())
}

/// Map a failed official CLI invocation to `(code, detail)`.
pub fn classify_machine_error(
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> (String, String) {
    let source = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let detail = sanitize_detail(source);
    let lower = source.to_lowercase();
    // Official 0.9.3 reports a missing profile as exit 1 with this message.
    if lower.contains("machine profile") && lower.contains("was not found") {
        return ("machines-unknown-machine".into(), detail);
    }
    if exit_code == Some(2) {
        let code = if lower.contains("unknown machine") {
            "machines-unknown-machine"
        } else if lower.contains("is disabled") {
            "machines-disabled"
        } else if lower.contains("not an api-backed machine command") {
            "machines-unsupported-subcommand"
        } else if [
            "duplicate",
            "already exists",
            "already in use",
            "already used",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ("machines-invalid-label".into(), "duplicate".into());
        } else {
            "machines-invalid-target"
        };
        return (code.to_string(), detail);
    }
    let trimmed = source.trim_start();
    if trimmed.starts_with('{') {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed.trim()) {
            if let Some(err) = json.get("error") {
                if err.get("code").and_then(|c| c.as_str()) == Some("protocol_mismatch") {
                    return ("machines-remote-incompatible".into(), detail);
                }
                let message = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(sanitize_detail)
                    .unwrap_or(detail.clone());
                return ("herdr-operation-error".into(), message);
            }
        }
    }
    if trimmed.starts_with("Error: Custom") {
        let message = extract_custom_error(trimmed).unwrap_or_else(|| trimmed.to_string());
        return classify_message(&message, &detail);
    }
    // Plain text failures (for example ssh output relayed verbatim): reuse the
    // substring rules, but do not claim "unreachable" for unrelated errors.
    let (code, _) = classify_message(source, &detail);
    if code != "machines-unreachable"
        || [
            "connection refused",
            "timed out",
            "could not resolve",
            "no route",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return (code, detail);
    }
    ("herdr-operation-error".into(), detail)
}

fn format_classified(exit_code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> String {
    let (code, detail) = classify_machine_error(
        exit_code,
        &String::from_utf8_lossy(stdout),
        &String::from_utf8_lossy(stderr),
    );
    error(&code, &detail)
}

// ── Concurrency gate ────────────────────────────────────────────────────────

pub(crate) struct Gate {
    limit: usize,
    used: Mutex<usize>,
    freed: Condvar,
}

pub(crate) struct GatePermit<'a>(&'a Gate);

impl Gate {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    pub(crate) fn acquire(&self) -> GatePermit<'_> {
        let mut used = self.used.lock().unwrap();
        while *used >= self.limit {
            used = self.freed.wait(used).unwrap();
        }
        *used += 1;
        GatePermit(self)
    }

    #[cfg(test)]
    pub(crate) fn try_acquire(&self) -> Option<GatePermit<'_>> {
        let mut used = self.used.lock().unwrap();
        if *used >= self.limit {
            return None;
        }
        *used += 1;
        Some(GatePermit(self))
    }
}

impl Drop for GatePermit<'_> {
    fn drop(&mut self) {
        *self.0.used.lock().unwrap() -= 1;
        self.0.freed.notify_one();
    }
}

static PROCESS_GATE: Gate = Gate::new(MAX_CONCURRENT_MACHINE_PROCESSES);
static IN_FLIGHT_SNAPSHOTS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

struct InFlight(String);

impl InFlight {
    fn claim(id: &str) -> Result<Self, String> {
        let mut guard = IN_FLIGHT_SNAPSHOTS.lock().unwrap();
        if !guard
            .get_or_insert_with(HashSet::new)
            .insert(id.to_string())
        {
            return Err("machines-busy".into());
        }
        Ok(Self(id.to_string()))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(set) = IN_FLIGHT_SNAPSHOTS.lock().unwrap().as_mut() {
            set.remove(&self.0);
        }
    }
}

// ── CLI execution ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) struct CliOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CliOutput {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

fn scaled(duration: Duration) -> Duration {
    if cfg!(windows) {
        duration.mul_f32(1.5)
    } else {
        duration
    }
}

const SHORT_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// Run the official CLI without a shell. The caller env is inherited except the
/// variables that would redirect a command to a pane-local server or Session;
/// `SSH_AUTH_SOCK` stays so ssh-agent keeps working.
pub(crate) fn run_machine_cli(
    binary: &Path,
    argv: &[String],
    timeout: Duration,
) -> Result<CliOutput, String> {
    let _permit = PROCESS_GATE.acquire();
    run_machine_cli_ungated(binary, argv, timeout)
}

fn run_machine_cli_ungated(
    binary: &Path,
    argv: &[String],
    timeout: Duration,
) -> Result<CliOutput, String> {
    let mut cmd = Command::new(binary);
    cmd.args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("HERDR_ENV")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .env_remove("HERDR_CLIENT_SOCKET_PATH");
    process_kill::configure_background_process(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| error("machines-binary-unavailable", &e.to_string()))?;
    let mut tree = process_kill::attach_process_tree(&mut child).map_err(|e| {
        error(
            "herdr-operation-error",
            &format!("process containment: {e}"),
        )
    })?;
    match wait_bounded_child(
        &mut child,
        &mut tree,
        scaled(timeout),
        MAX_NDJSON_LINE_BYTES,
    ) {
        Ok((stdout, stderr, status)) => Ok(CliOutput {
            code: status.code(),
            stdout,
            stderr,
        }),
        Err(message) if message.starts_with("timeout") => Err("machines-timeout".into()),
        Err(message) if message.starts_with("tooLarge") => Err("machines-output-too-large".into()),
        Err(message) => Err(error("herdr-operation-error", &message)),
    }
}

// ── Capability detection ────────────────────────────────────────────────────

/// The parsed version, which every machine operation checks.
#[derive(Clone)]
struct Detection {
    version: Option<String>,
    parsed: Option<(u32, u32, u32)>,
}

type DetectionKey = (PathBuf, Option<SystemTime>);
static DETECTIONS: Mutex<Option<HashMap<DetectionKey, Detection>>> = Mutex::new(None);
/// Subcommands found present; an absent or failed probe is never stored.
static SUBCOMMANDS: Mutex<Option<HashSet<(DetectionKey, &'static str)>>> = Mutex::new(None);

fn detection_key(binary: &Path) -> DetectionKey {
    let mtime = std::fs::metadata(binary).and_then(|m| m.modified()).ok();
    (binary.to_path_buf(), mtime)
}

fn detect(binary: &Path) -> Detection {
    let key = detection_key(binary);
    if let Some(found) = DETECTIONS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|map| map.get(&key))
    {
        return found.clone();
    }
    let version_text = run_machine_cli(binary, &["--version".to_string()], VERSION_TIMEOUT)
        .ok()
        .filter(CliOutput::success)
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let parsed = version_text.as_deref().and_then(parse_version);
    let version = parsed.map(|(a, b, c)| format!("{a}.{b}.{c}"));
    let detection = Detection { version, parsed };
    // A failed or unparsable probe is not cached, so the next call re-detects
    // instead of sticking until the binary changes.
    if detection.parsed.is_some() {
        let mut guard = DETECTIONS.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= 8 {
            map.clear();
        }
        map.insert(key, detection.clone());
    }
    detection
}

/// Whether `op`'s subcommand exists, and whether the probe finished. Only a
/// present one is cached: a nonzero exit, signal, timeout or oversized output
/// may be transient, so an absent answer is asked again next time.
/// Capabilities are read rarely (startup, the Machines panel, retries), never
/// per poll, so that stays cheap. A probe that never reached an exit code
/// (timeout, signal, spawn failure, oversized output) is incomplete.
fn has_subcommand(binary: &Path, op: MachineOp<'_>, name: &'static str) -> (bool, bool) {
    let key = (detection_key(binary), name);
    if SUBCOMMANDS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|found| found.contains(&key))
    {
        return (true, true);
    }
    let probe = run_machine_cli(binary, &build_machine_argv(&op), SHORT_TIMEOUT);
    let complete = probe.as_ref().is_ok_and(|out| out.code.is_some());
    let present = probe.is_ok_and(|out| out.success());
    if present {
        let mut guard = SUBCOMMANDS.lock().unwrap();
        let found = guard.get_or_insert_with(HashSet::new);
        if found.len() >= 16 {
            found.clear();
        }
        found.insert(key);
    }
    (present, complete)
}

pub fn machines_capabilities(manager: &HerdrManager) -> HerdrMachinesCapabilities {
    let source = manager.machines_active_source().to_string();
    if manager.machines_is_remote() {
        return HerdrMachinesCapabilities {
            binary_path: String::new(),
            version: None,
            supported: false,
            has_status: false,
            has_reconnect: false,
            probes_complete: true,
            source,
            reason: Some("machines-local-only".into()),
        };
    }
    let Some(binary) = manager.resolve_binary() else {
        return HerdrMachinesCapabilities {
            binary_path: String::new(),
            version: None,
            supported: false,
            has_status: false,
            has_reconnect: false,
            probes_complete: true,
            source,
            reason: Some("machines-binary-unavailable".into()),
        };
    };
    let detection = detect(&binary);
    let supported = detection
        .parsed
        .is_some_and(|v| version_at_least(v, MIN_MACHINES_VERSION));
    // A custom build may ship `machine status` without `machine reconnect`
    // (Windows HERDR has none), so each subcommand is probed on its own.
    let (has_status, status_complete) = if supported {
        has_subcommand(&binary, MachineOp::Capability, "status")
    } else {
        (false, true)
    };
    let (has_reconnect, reconnect_complete) = if supported {
        has_subcommand(&binary, MachineOp::ReconnectCapability, "reconnect")
    } else {
        (false, true)
    };
    let reason = (!supported).then(|| {
        if source == "global" {
            error("machines-runtime-too-old", "global-use-bundled")
        } else {
            "machines-runtime-too-old".to_string()
        }
    });
    HerdrMachinesCapabilities {
        binary_path: binary.display().to_string(),
        version: detection.version,
        supported,
        has_status,
        has_reconnect,
        probes_complete: status_complete && reconnect_complete,
        source,
        reason,
    }
}

fn ready_binary(manager: &HerdrManager) -> Result<PathBuf, String> {
    if manager.machines_is_remote() {
        return Err("machines-local-only".into());
    }
    let binary = manager
        .resolve_binary()
        .ok_or("machines-binary-unavailable")?;
    let detection = detect(&binary);
    if !detection
        .parsed
        .is_some_and(|v| version_at_least(v, MIN_MACHINES_VERSION))
    {
        return Err("machines-runtime-too-old".into());
    }
    Ok(binary)
}

// ── Operations ──────────────────────────────────────────────────────────────

fn expect_success(out: &CliOutput) -> Result<(), String> {
    if out.success() {
        Ok(())
    } else {
        Err(format_classified(out.code, &out.stdout, &out.stderr))
    }
}

fn list_with(binary: &Path) -> Result<Vec<HerdrMachine>, String> {
    let out = run_machine_cli(binary, &build_machine_argv(&MachineOp::List), SHORT_TIMEOUT)?;
    expect_success(&out)?;
    Ok(parse_machine_list(&out.stdout)?.machines)
}

pub fn machines_list(manager: &HerdrManager) -> Result<Vec<HerdrMachine>, String> {
    list_with(&ready_binary(manager)?)
}

pub fn machines_status(
    manager: &HerdrManager,
    machine_id: &str,
) -> Result<HerdrMachineStatus, String> {
    validate_machine_id(machine_id)?;
    let binary = ready_binary(manager)?;
    status_with(&binary, machine_id, STATUS_TIMEOUT)
}

fn status_with(
    binary: &Path,
    machine_id: &str,
    timeout: Duration,
) -> Result<HerdrMachineStatus, String> {
    let machine = list_with(binary)?
        .into_iter()
        .find(|m| m.id == machine_id)
        .ok_or("machines-unknown-machine")?;
    if !machine.enabled {
        return Ok(HerdrMachineStatus {
            id: machine.id,
            label: machine.label,
            status: "disabled".into(),
            error: None,
        });
    }
    let out = run_machine_cli(
        binary,
        &build_machine_argv(&MachineOp::Status(machine_id)),
        timeout,
    )?;
    // Exit 1 means "some machine is not reachable" and still carries a valid
    // JSON verdict; trust the status field. Anything else needs a parseable body.
    if matches!(out.code, Some(0) | Some(1)) {
        if let Ok(status) = parse_machine_status(&out.stdout, machine_id, &machine.label) {
            return Ok(status);
        }
    }
    if out.success() {
        return parse_machine_status(&out.stdout, machine_id, &machine.label);
    }
    Err(format_classified(out.code, &out.stdout, &out.stderr))
}

pub fn machines_agents(
    manager: &HerdrManager,
    machine_id: &str,
) -> Result<HerdrMachineSnapshot, String> {
    validate_machine_id(machine_id)?;
    let binary = ready_binary(manager)?;
    let _claim = InFlight::claim(machine_id)?;
    let out = run_machine_cli(
        &binary,
        &build_machine_argv(&MachineOp::Agents(machine_id)),
        SNAPSHOT_TIMEOUT,
    )?;
    expect_success(&out)?;
    let fetched_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    parse_snapshot(&out.stdout, machine_id, fetched_at)
}

fn mutate(
    manager: &HerdrManager,
    machine_id: &str,
    op: MachineOp<'_>,
) -> Result<Vec<HerdrMachine>, String> {
    validate_machine_id(machine_id)?;
    let binary = ready_binary(manager)?;
    let out = run_machine_cli(&binary, &build_machine_argv(&op), SHORT_TIMEOUT)?;
    expect_success(&out)?;
    // The catalog already changed: a failed relist must not read as a failed mutation.
    list_with(&binary).map_err(|detail| relist_failed(&detail))
}

fn relist_failed(detail: &str) -> String {
    error("machines-relist-failed", detail)
}

pub fn machines_rename(
    manager: &HerdrManager,
    machine_id: &str,
    label: &str,
) -> Result<Vec<HerdrMachine>, String> {
    validate_machine_label(label)?;
    mutate(manager, machine_id, MachineOp::Rename(machine_id, label))
}

pub fn machines_set_enabled(
    manager: &HerdrManager,
    machine_id: &str,
    enabled: bool,
) -> Result<Vec<HerdrMachine>, String> {
    let op = if enabled {
        MachineOp::Enable(machine_id)
    } else {
        MachineOp::Disable(machine_id)
    };
    mutate(manager, machine_id, op)
}

pub fn machines_remove(
    manager: &HerdrManager,
    machine_id: &str,
) -> Result<Vec<HerdrMachine>, String> {
    mutate(manager, machine_id, MachineOp::Remove(machine_id))
}

/// Open `machine add|reconnect` or the official `client` in a PTY.
pub fn machines_interactive_open(
    manager: &std::sync::Arc<HerdrManager>,
    spec: &HerdrMachineInteractiveSpec,
    size: HerdrClientSize,
    on_event: OnTerminalEvent,
) -> Result<HerdrMachineInteractiveOpened, String> {
    if manager.machines_is_remote() {
        return Err("machines-local-only".into());
    }
    let args = build_interactive_args(spec, cfg!(windows))?;
    let binary = ready_binary(manager)?;
    let session_id = manager.open_native_command(&binary, &args, size, on_event)?;
    Ok(HerdrMachineInteractiveOpened { session_id })
}
