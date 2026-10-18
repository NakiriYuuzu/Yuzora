//! Closed local Herdr boundary. No raw API or server/session lifecycle methods.
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf, sync::Arc};
use yuzora_host::herdr_service::{
    AgentPrompt, AgentSendKeys, HerdrClientSize, HerdrFeatureRequest, HerdrManager,
    HerdrNamedSession, HerdrScrollDirection, HerdrSnapshotResult, HerdrTerminalMode,
    OnSubscriptionEvent, OnTerminalEvent,
};

pub(crate) type Result<T> = std::result::Result<T, &'static str>;
pub(crate) const SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;

/// Direct NDJSON reachability, including Host's compatibility ping. Connector
/// and official client operations are separate transports, not raw API forwarding.
pub(crate) const METHOD_MAPPING: &[(&str, &[&str], &str)] = &[
    ("hello", &[], "gateway"),
    ("session.ping", &[], "gateway"),
    ("sessions.list", &[], "session list --json"),
    ("snapshot.get", &["ping", "session.snapshot"], "API"),
    (
        "events.subscribe",
        &["ping", "events.subscribe"],
        "API subscription",
    ),
    (
        "terminal.observe",
        &["ping", "session.snapshot"],
        "terminal session observe",
    ),
    ("stream.close", &[], "release connector/subscription"),
    (
        "terminal.control",
        &["ping", "session.snapshot"],
        "terminal session control",
    ),
    ("terminal.input", &[], "connector terminal.input"),
    ("terminal.scroll", &[], "connector terminal.scroll"),
    ("terminal.release", &[], "release connector"),
    (
        "agent.prompt",
        &["ping", "session.snapshot", "agent.prompt"],
        "API",
    ),
    (
        "pane.sendKeys",
        &["ping", "session.snapshot", "agent.send_keys"],
        "API; agent panes only",
    ),
    ("full.open", &["ping"], "official client PTY"),
    ("full.input", &[], "official client PTY input"),
    ("full.resize", &[], "official client PTY resize"),
    ("full.close", &[], "release official client"),
];

/// Call only from the gateway's bounded blocking executor. Callback consumers
/// must be non-blocking; release joins Host-owned readers and reaps connectors.
pub(crate) trait HerdrPort: Send + Sync {
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>>;
    fn snapshot(&self, session: &str) -> Result<HerdrSnapshotResult>;
    fn observe(
        &self,
        session: &str,
        target: &str,
        cols: u16,
        rows: u16,
        sink: OnTerminalEvent,
    ) -> Result<String>;
    /// Protocol 22 has no wildcard agent-status selector: `panes` scopes them.
    fn subscribe(
        &self,
        session: &str,
        panes: Vec<String>,
        sink: OnSubscriptionEvent,
    ) -> Result<String>;
    fn release(&self, resource: Resource);
    fn control(
        &self,
        session: &str,
        target: &str,
        size: (u16, u16),
        takeover: bool,
        sink: OnTerminalEvent,
    ) -> Result<String>;
    fn full(&self, session: &str, size: (u16, u16), sink: OnTerminalEvent) -> Result<String>;
    fn input(&self, resource: &str, bytes: Vec<u8>) -> Result<()>;
    fn scroll(&self, resource: &str, direction: HerdrScrollDirection, lines: u32) -> Result<()>;
    fn resize(&self, resource: &str, size: (u16, u16)) -> Result<()>;
    fn prompt(&self, session: &str, target: &str, text: String) -> Result<()>;
    fn keys(&self, session: &str, target: &str, keys: Vec<String>) -> Result<()>;
}

pub(crate) enum Resource {
    Observe(String),
    Events(String),
}

pub(crate) struct LocalHerdr(Arc<HerdrManager>);
impl LocalHerdr {
    pub(crate) fn new(binary: PathBuf) -> Self {
        // Explicit admin-selected binary; no PATH fallback, persisted preference,
        // configure_paths, remote backend, or startup probe. Main sanitizes env
        // before creating the runtime. Merely constructing this performs no IO.
        Self(Arc::new(HerdrManager::with_binary(binary)))
    }
}
fn clean_environment() -> Result<()> {
    // Library embedding must also fail closed rather than follow pane-local
    // routing. Only the single-threaded executable entry point mutates env.
    if std::env::vars_os().any(|(name, _)| {
        name.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("HERDR_")
    }) {
        return Err("herdr-environment");
    }
    Ok(())
}
fn host_error(error: String) -> &'static str {
    // Host exposes protocol errors as strings, not a typed public error API.
    // Match its stable size code only; never forward backend messages/paths.
    if error.starts_with("tooLarge:") {
        "payload-too-large"
    } else if error.starts_with("agent_blocked:") {
        // A blocked agent takes keys or terminal input, not a new prompt.
        "agent-blocked"
    } else {
        "herdr-unavailable"
    }
}
impl HerdrPort for LocalHerdr {
    fn sessions(&self) -> Result<Vec<HerdrNamedSession>> {
        clean_environment()?;
        self.0.list_sessions().map_err(host_error)
    }
    fn snapshot(&self, session: &str) -> Result<HerdrSnapshotResult> {
        clean_environment()?;
        self.0.snapshot(Some(session)).map_err(host_error)
    }
    fn observe(
        &self,
        session: &str,
        target: &str,
        cols: u16,
        rows: u16,
        sink: OnTerminalEvent,
    ) -> Result<String> {
        clean_environment()?;
        self.0
            .open_terminal(
                target.into(),
                HerdrTerminalMode::Observe,
                false,
                cols,
                rows,
                Some(session.into()),
                sink,
            )
            .map(|opened| opened.session_id)
            .map_err(host_error)
    }
    fn subscribe(
        &self,
        session: &str,
        panes: Vec<String>,
        sink: OnSubscriptionEvent,
    ) -> Result<String> {
        clean_environment()?;
        self.0
            .events_subscribe(Some(session.into()), panes, sink)
            .map_err(host_error)
    }
    fn control(
        &self,
        session: &str,
        target: &str,
        size: (u16, u16),
        takeover: bool,
        sink: OnTerminalEvent,
    ) -> Result<String> {
        clean_environment()?;
        self.0
            .open_terminal(
                target.into(),
                HerdrTerminalMode::Control,
                takeover,
                size.0,
                size.1,
                Some(session.into()),
                sink,
            )
            .map(|v| v.session_id)
            .map_err(host_error)
    }
    fn full(&self, session: &str, size: (u16, u16), sink: OnTerminalEvent) -> Result<String> {
        clean_environment()?;
        self.0
            .open_native_client(
                session,
                HerdrClientSize {
                    cols: size.0,
                    rows: size.1,
                    cell_width: 0,
                    cell_height: 0,
                },
                sink,
            )
            .map(|v| v.session_id)
            .map_err(host_error)
    }
    fn input(&self, resource: &str, bytes: Vec<u8>) -> Result<()> {
        use base64::Engine;
        clean_environment()?;
        self.0
            .terminal_input(
                resource,
                None,
                Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            )
            .map_err(host_error)
    }
    fn scroll(&self, resource: &str, direction: HerdrScrollDirection, lines: u32) -> Result<()> {
        clean_environment()?;
        self.0
            .terminal_scroll(resource, direction, lines, None, None)
            .map_err(host_error)
    }
    fn resize(&self, resource: &str, size: (u16, u16)) -> Result<()> {
        clean_environment()?;
        self.0
            .terminal_resize(resource, size.0, size.1)
            .map_err(host_error)
    }
    fn prompt(&self, session: &str, target: &str, text: String) -> Result<()> {
        clean_environment()?;
        self.0
            .feature(
                session,
                HerdrFeatureRequest::AgentPrompt(AgentPrompt {
                    target: target.into(),
                    text,
                    wait: None,
                }),
            )
            .map(|_| ())
            .map_err(host_error)
    }
    fn keys(&self, session: &str, target: &str, keys: Vec<String>) -> Result<()> {
        clean_environment()?;
        self.0
            .feature(
                session,
                HerdrFeatureRequest::AgentSendKeys(AgentSendKeys {
                    target: target.into(),
                    keys,
                }),
            )
            .map(|_| ())
            .map_err(host_error)
    }
    fn release(&self, resource: Resource) {
        match resource {
            Resource::Observe(id) => {
                let _ = self.0.terminal_release(&id);
            }
            Resource::Events(id) => {
                let _ = self.0.events_release(&id);
            }
        }
    }
}

pub(crate) struct Reads {
    pub(crate) port: Arc<dyn HerdrPort>,
    allowlist: HashSet<String>,
}
impl Reads {
    pub(crate) fn new(port: Arc<dyn HerdrPort>, allowlist: &[String]) -> Self {
        Self {
            port,
            allowlist: allowlist.iter().cloned().collect(),
        }
    }
    pub(crate) fn sessions(&self) -> Result<Value> {
        if self.allowlist.is_empty() {
            return Ok(json!({"sessions":[]}));
        }
        let sessions = self
            .port
            .sessions()?
            .into_iter()
            .filter(|s| s.running && valid_id(&s.name) && self.allowlist.contains(&s.name))
            .map(|s| json!({"name":s.name,"default":s.default}))
            .collect::<Vec<_>>();
        Ok(json!({"sessions":sessions}))
    }
    pub(crate) fn running(&self, session: &str) -> Result<()> {
        if !valid_id(session) || !self.allowlist.contains(session) {
            return Err("session-not-allowed");
        }
        let sessions = self.port.sessions()?;
        let named = sessions
            .iter()
            .find(|s| s.name == session)
            .ok_or("session-stopped")?;
        if !named.running {
            return Err("session-stopped");
        }
        // Host treats the literal `live` as a default-session alias. Never let
        // an allowlisted literal accidentally select a different named session.
        if session == "live"
            && sessions
                .iter()
                .find(|s| s.default)
                .or(sessions.first())
                .is_none_or(|s| s.name != session)
        {
            return Err("session-not-allowed");
        }
        Ok(())
    }
    pub(crate) fn snapshot(&self, session: &str) -> Result<Value> {
        self.running(session)?;
        let raw = self
            .port
            .snapshot(session)
            .map_err(|e| self.after_error(session, e))?;
        normalize_snapshot(session, &raw.snapshot)
    }
    pub(crate) fn after_error(&self, session: &str, error: &'static str) -> &'static str {
        match self.running(session) {
            Err("session-stopped") => "session-stopped",
            _ => error,
        }
    }
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}
fn text(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}
fn status(value: &Value) -> &str {
    match value.as_str() {
        Some("idle") => "idle",
        Some("working") => "working",
        Some("blocked") => "blocked",
        Some("done") => "done",
        _ => "unknown",
    }
}
fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value[key].as_array().into_iter().flatten()
}
fn rect_for(snapshot: &Value, pane: &str) -> Option<(u16, u16)> {
    array(snapshot, "layouts")
        .flat_map(|layout| array(layout, "panes"))
        .filter(|p| p["pane_id"] == pane)
        .filter_map(|p| {
            Some((
                u16::try_from(p["rect"]["width"].as_u64()?).ok()?,
                u16::try_from(p["rect"]["height"].as_u64()?).ok()?,
            ))
        })
        .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)))
}

/// Layout rect is NOT the PTY size. Herdr 0.9.1's schema has viewport_rows,
/// graphics grid placement and layout rectangles, but no actual PTY cols/rows.
/// Observe frames are fixed at these requested dimensions (small ones crop).
/// Pane IDs of an official snapshot, for scoping agent-status subscriptions.
pub(crate) fn pane_ids(snapshot: &Value) -> Vec<String> {
    array(snapshot, "panes")
        .filter_map(|p| p["pane_id"].as_str())
        .filter(|id| valid_id(id))
        .map(str::to_owned)
        .collect()
}
pub(crate) fn observe_geometry(snapshot: &Value, target: &str) -> Result<(String, u16, u16)> {
    let pane = array(snapshot, "panes")
        .find(|p| p["pane_id"] == target || p["terminal_id"] == target)
        .ok_or("target-not-found")?;
    let id = pane["pane_id"]
        .as_str()
        .filter(|s| valid_id(s))
        .ok_or("target-not-found")?;
    let (cols, rows) = rect_for(snapshot, id)
        .filter(|(c, r)| *c > 0 && *r > 0)
        .ok_or("geometry-unavailable")?;
    Ok((id.into(), cols, rows))
}

/// Explicit minimal projection, never pass through paths, commands, state_labels,
/// tokens, raw titles, schema/capability documents or arbitrary backend fields.
pub(crate) fn normalize_snapshot(session: &str, raw: &Value) -> Result<Value> {
    let workspaces: Vec<_> = array(raw,"workspaces").map(|w| json!({
        "id":text(w,"workspace_id"),"label":text(w,"label"),"status":status(&w["agent_status"])
    })).collect();
    let tabs: Vec<_> = array(raw, "tabs")
        .map(|t| {
            json!({
                "id":text(t,"tab_id"),"workspaceId":text(t,"workspace_id"),"label":text(t,"label")
            })
        })
        .collect();
    let panes: Vec<_> = array(raw,"panes").map(|p| {
        let rect = p["pane_id"].as_str().and_then(|id| rect_for(raw,id));
        json!({"id":text(p,"pane_id"),"tabId":text(p,"tab_id"),"workspaceId":text(p,"workspace_id"),
            "status":status(&p["agent_status"]),"label":text(p,"label"),
            "layout":rect.map(|(cols,rows)|json!({"cols":cols,"rows":rows}))})
    }).collect();
    let agents: Vec<_> = array(raw,"agents").map(|a| json!({
        "paneId":text(a,"pane_id"),"name":text(a,"name"),"agent":text(a,"display_agent").or_else(||text(a,"agent")),"status":status(&a["agent_status"])
    })).collect();
    let value = json!({"session":session,"workspaces":workspaces,"tabs":tabs,"panes":panes,"agents":agents});
    if serde_json::to_vec(&value)
        .map_err(|_| "herdr-unavailable")?
        .len()
        > SNAPSHOT_BYTES
    {
        return Err("payload-too-large");
    }
    Ok(value)
}

pub(crate) fn closed_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("terminal attach taken over") => "taken-over",
        Some("server is shutting down") => "session-stopped",
        Some(s)
            if s.starts_with("terminal attach failed: ")
                && s.ends_with(" already has an attached client; retry with --takeover") =>
        {
            "owned-elsewhere"
        }
        _ => "stream-closed",
    }
}

#[cfg(test)]
#[path = "herdr_tests.rs"]
mod tests;
