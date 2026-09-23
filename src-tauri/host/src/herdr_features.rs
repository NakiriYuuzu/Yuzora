//! Typed public HERDR operations used by the workbench management surfaces.
//! This module is a child of herdr_service so every operation shares its binary,
//! named-session discovery, compatibility gate and bounded socket transport.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const FEATURE_API_METHODS: &[&str] = &[
    "worktree.create",
    "worktree.open",
    "worktree.remove",
    "pane.move",
    "agent.start",
    "agent.prompt",
    "agent.wait",
    "agent.rename",
    "agent.send_keys",
    "agent.explain",
    "integration.list",
    "integration.install",
    "integration.uninstall",
    "plugin.list",
    "plugin.enable",
    "plugin.disable",
    "plugin.action.invoke",
    "plugin.log.list",
    "plugin.pane.open",
];

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum HerdrFeatureRequest {
    #[serde(rename = "worktree.create")]
    WorktreeCreate(WorktreeCreate),
    #[serde(rename = "worktree.open")]
    WorktreeOpen(WorktreeOpen),
    #[serde(rename = "worktree.remove")]
    WorktreeRemove(WorktreeRemove),
    #[serde(rename = "pane.move")]
    PaneMove(PaneMove),
    #[serde(rename = "agent.start")]
    AgentStart(AgentStart),
    #[serde(rename = "agent.prompt")]
    AgentPrompt(AgentPrompt),
    #[serde(rename = "agent.wait")]
    AgentWait(AgentWait),
    #[serde(rename = "agent.rename")]
    AgentRename(AgentRename),
    #[serde(rename = "agent.send_keys")]
    AgentSendKeys(AgentSendKeys),
    #[serde(rename = "agent.explain")]
    AgentExplain(AgentTarget),
    #[serde(rename = "integration.list")]
    IntegrationList {},
    #[serde(rename = "integration.install")]
    IntegrationInstall(IntegrationTarget),
    #[serde(rename = "integration.uninstall")]
    IntegrationUninstall(IntegrationTarget),
    #[serde(rename = "plugin.list")]
    PluginList {},
    #[serde(rename = "plugin.enable")]
    PluginEnable(PluginTarget),
    #[serde(rename = "plugin.disable")]
    PluginDisable(PluginTarget),
    #[serde(rename = "plugin.action.invoke")]
    PluginActionInvoke(PluginAction),
    #[serde(rename = "plugin.log.list")]
    PluginLogList(PluginTarget),
    #[serde(rename = "plugin.pane.open")]
    PluginPaneOpen(PluginPaneOpen),
    #[serde(rename = "session.start")]
    SessionStart {},
    #[serde(rename = "session.stop")]
    SessionStop {},
    #[serde(rename = "session.delete")]
    SessionDelete {},
    #[serde(rename = "plugin.install")]
    PluginInstall {
        source: String,
        revision: Option<String>,
    },
    #[serde(rename = "plugin.uninstall")]
    PluginUninstall(PluginTarget),
}

macro_rules! params {
    ($name:ident { $($field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Debug, Deserialize, Serialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $ty),* }
    };
}
params!(WorktreeCreate { workspace_id: String, branch: Option<String>, base: Option<String>, path: Option<String>, label: Option<String>, focus: bool });
params!(WorktreeOpen { workspace_id: String, path: String, label: Option<String>, focus: bool });
params!(WorktreeRemove {
    workspace_id: String,
    force: bool
});
params!(PaneMove {
    pane_id: String,
    destination: PaneMoveDestination,
    focus: bool
});
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaneMoveDestination {
    Tab {
        tab_id: String,
        target_pane_id: Option<String>,
        split: HerdrSplitDirection,
    },
    NewTab {
        workspace_id: String,
        label: Option<String>,
    },
    NewWorkspace {
        label: Option<String>,
        tab_label: Option<String>,
    },
}
params!(AgentStart { pane_id: String, name: String, kind: String, args: Vec<String>, timeout_ms: u64 });
params!(AgentTarget { target: String });
params!(AgentWaitOptions { until: Vec<String>, timeout_ms: u64 });
params!(AgentPrompt { target: String, text: String, wait: Option<AgentWaitOptions> });
params!(AgentWait { target: String, until: Vec<String>, timeout_ms: u64 });
params!(AgentRename { target: String, name: Option<String> });
params!(AgentSendKeys { target: String, keys: Vec<String> });
params!(IntegrationTarget { target: String });
params!(PluginTarget { plugin_id: String });
params!(PluginAction { plugin_id: String, action_id: String, context: Option<PluginContext> });
params!(PluginContext { workspace_id: String, tab_id: Option<String>, focused_pane_id: Option<String> });
params!(PluginPaneOpen { plugin_id: String, entrypoint: String, placement: String, workspace_id: Option<String>, target_pane_id: Option<String>, direction: Option<HerdrSplitDirection>, focus: bool });

impl HerdrFeatureRequest {
    fn envelope(&self) -> Result<(String, Value), String> {
        let value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        ensure_ipc_bound(&value).map_err(String::from)?;
        validate_json_complexity(&value).map_err(String::from)?;
        let method = value["method"]
            .as_str()
            .ok_or("invalid-herdr-operation")?
            .to_owned();
        let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
        // Values are argv/JSON data, never shell fragments. NUL cannot be a path,
        // identity or argv value, and is not accepted as terminal input here.
        fn valid_strings(v: &Value) -> bool {
            match v {
                Value::String(s) => !s.contains('\0'),
                Value::Array(a) => a.iter().all(valid_strings),
                Value::Object(o) => o.values().all(valid_strings),
                _ => true,
            }
        }
        if !valid_strings(&params) {
            return Err("herdr-input-contains-nul".into());
        }
        match self {
            Self::AgentStart(p) if !(3001..=120_000).contains(&p.timeout_ms) => {
                return Err("agent-start-timeout-out-of-range".into())
            }
            Self::AgentWait(p) if !(1..=120_000).contains(&p.timeout_ms) => {
                return Err("agent-wait-timeout-out-of-range".into())
            }
            Self::AgentPrompt(p)
                if p.wait
                    .as_ref()
                    .is_some_and(|w| !(1..=120_000).contains(&w.timeout_ms)) =>
            {
                return Err("agent-wait-timeout-out-of-range".into())
            }
            Self::AgentSendKeys(p) if p.keys.is_empty() || p.keys.len() > 32 => {
                return Err("invalid-agent-keys".into())
            }
            Self::PluginPaneOpen(p) => {
                let valid_target = match p.placement.as_str() {
                    "overlay" | "popup" => {
                        p.workspace_id.is_none()
                            && p.target_pane_id.is_none()
                            && p.direction.is_none()
                    }
                    "split" | "zoomed" => p.workspace_id.is_none(),
                    "tab" => p.target_pane_id.is_none() && p.direction.is_none(),
                    _ => false,
                };
                if !valid_target {
                    return Err("invalid-plugin-pane-placement-target".into());
                }
            }
            _ => {}
        }
        for key in [
            "pane_id",
            "target",
            "workspace_id",
            "plugin_id",
            "entrypoint",
        ] {
            if params
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|s| s.trim().is_empty())
            {
                return Err(format!("{key} is required"));
            }
        }
        Ok((method, params))
    }
}

impl HerdrManager {
    pub fn feature(
        &self,
        session_name: &str,
        request: HerdrFeatureRequest,
    ) -> Result<Value, String> {
        validate_session_name(session_name)?;
        let (method, params) = request.envelope()?;
        if self.remote.is_some() {
            return Err("herdr-features-must-run-on-owning-host".into());
        }
        match request {
            HerdrFeatureRequest::SessionStart {} => return self.start_named_session(session_name),
            HerdrFeatureRequest::SessionStop {} | HerdrFeatureRequest::SessionDelete {} => {
                // Require the exact advertised name; do not let an inherited
                // caller or a case-insensitive filesystem select another Session.
                if !self.list_sessions()?.iter().any(|s| s.name == session_name) {
                    return Err("unknown-herdr-session".into());
                }
                let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
                let action = if method == "session.stop" {
                    "stop"
                } else {
                    "delete"
                };
                let result = run_herdr_json_with_session_timeout(
                    &binary,
                    &["session", action, "--json", "--", session_name],
                    Some(session_name),
                    Duration::from_secs(20),
                );
                self.capability_cache.lock().unwrap().clear();
                return result;
            }
            HerdrFeatureRequest::PluginInstall { source, revision } => {
                // Plugin CLI commands are only offered where the Session
                // advertises the official plugin API.
                self.require_api_method(session_name, "plugin.list")?;
                if source.starts_with('-')
                    || source.split('/').count() < 2
                    || source.chars().any(char::is_whitespace)
                {
                    return Err("plugin-source-must-be-github-owner-repository".into());
                }
                let mut args = vec![
                    "plugin".to_owned(),
                    "install".into(),
                    source,
                    "--yes".into(),
                ];
                if let Some(revision) = revision.filter(|v| !v.trim().is_empty()) {
                    args.extend(["--ref".into(), revision]);
                }
                return self.feature_cli(session_name, &args);
            }
            HerdrFeatureRequest::PluginUninstall(p) => {
                self.require_api_method(session_name, "plugin.list")?;
                if p.plugin_id.starts_with('-') {
                    return Err("invalid-plugin-id".into());
                }
                return self.feature_cli(
                    session_name,
                    &["plugin".into(), "uninstall".into(), p.plugin_id],
                );
            }
            _ => {}
        }
        self.require_api_method(session_name, &method)?;
        let (_, socket) = self.require_running_session_socket(Some(session_name))?;
        let response =
            api_request_with_timeout(&socket, &method, params, Duration::from_secs(135))?;
        bounded_ipc(
            response
                .get("result")
                .cloned()
                .ok_or("herdr-result-missing")?,
        )
    }

    fn require_api_method(&self, session_name: &str, method: &str) -> Result<(), String> {
        let caps = self.cached_capabilities_for_session(Some(session_name));
        if caps.api.methods.iter().any(|m| m == method) {
            return Ok(());
        }
        Err(caps
            .api
            .reason
            .unwrap_or_else(|| format!("HERDR does not support {method}")))
    }

    /// Shared gate for operations that drive a Session outside its public
    /// socket API (CLI commands and the official client).
    pub(super) fn session_is_compatible(&self, session_name: &str) -> bool {
        let caps = self.cached_capabilities_for_session(Some(session_name));
        caps.api.snapshot && caps.server.compatible == Some(true)
    }

    fn feature_cli(&self, session_name: &str, args: &[String]) -> Result<Value, String> {
        if !self.session_is_compatible(session_name) {
            return Err("herdr-session-incompatible".into());
        }
        self.require_running_session_socket(Some(session_name))?;
        let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
        let mut command = Command::new(binary);
        command
            .args(["--session", session_name])
            .args(args)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_ENV")
            .env("HERDR_SESSION", session_name)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_background_process(&mut command);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let mut tree = process_kill::attach_process_tree(&mut child).map_err(|e| e.to_string())?;
        let (stdout, stderr, status) = wait_bounded_child(
            &mut child,
            &mut tree,
            Duration::from_secs(120),
            MAX_NDJSON_LINE_BYTES,
        )?;
        let output = String::from_utf8(stdout).map_err(|_| "invalid-herdr-output")?;
        let errors = String::from_utf8(stderr).map_err(|_| "invalid-herdr-output")?;
        if !status.success() {
            return Err(format!("HERDR operation failed: {errors}\n{output}"));
        }
        bounded_ipc(json!({"messages": [output, errors]}))
    }

    fn start_named_session(&self, name: &str) -> Result<Value, String> {
        let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
        let status = run_herdr_json_with_session(&binary, &["status", "--json"], Some(name))?;
        if status.pointer("/server/running").and_then(Value::as_bool) == Some(true) {
            if status
                .pointer("/server/compatible")
                .and_then(Value::as_bool)
                != Some(true)
            {
                return Err("herdr-session-incompatible".into());
            }
            return Ok(json!({"started": false, "name": name}));
        }
        let mut command = Command::new(&binary);
        command.args(["--session", name]);
        // On Windows the official launcher detects SSH kill-on-close jobs
        // and starts its daemon through WMI when needed. A direct `server`
        // child could otherwise disappear when this helper channel closes.
        #[cfg(not(windows))]
        command.arg("server");
        command
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_ENV")
            .env("HERDR_SESSION", name)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        process_kill::configure_background_process(&mut command);
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(status) = run_herdr_json_with_session_timeout(
                &binary,
                &["status", "--json"],
                Some(name),
                Duration::from_secs(2),
            ) {
                if status.pointer("/server/running").and_then(Value::as_bool) == Some(true) {
                    if status
                        .pointer("/server/compatible")
                        .and_then(Value::as_bool)
                        != Some(true)
                    {
                        return Err("herdr-session-incompatible".into());
                    }
                    self.capability_cache.lock().unwrap().clear();
                    #[cfg(windows)]
                    process_kill::terminate_direct_child_and_reap(&mut child)
                        .map_err(|error| error.to_string())?;
                    // Reap our server child when the user eventually stops it;
                    // closing Yuzora must not terminate the persistent Session.
                    #[cfg(not(windows))]
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return Ok(json!({"started": true, "name": name}));
                }
            }
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("herdr-session-start-failed".into());
            }
            if Instant::now() >= deadline {
                process_kill::terminate_direct_child_and_reap(&mut child)
                    .map_err(|e| e.to_string())?;
                return Err("herdr-session-start-timeout".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || name == "."
        || name == ".."
    {
        return Err("invalid-herdr-session-name".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_are_closed_and_cannot_smuggle_unsupported_parameters() {
        for value in [
            json!({"method":"server.stop","params":{}}),
            json!({"method":"worktree.remove","params":{"workspace_id":"w1","force":false,"trust_repository":true}}),
            json!({"method":"plugin.enable","params":{"plugin_id":"a"},"socket":"elsewhere"}),
        ] {
            assert!(serde_json::from_value::<HerdrFeatureRequest>(value).is_err());
        }
    }

    #[test]
    fn native_move_payload_preserves_destination_and_opaque_ids() {
        let value = json!({"method":"pane.move","params":{"pane_id":"w2:p9","destination":{"type":"new_tab","workspace_id":"w8","label":"Review"},"focus":false}});
        let request: HerdrFeatureRequest = serde_json::from_value(value.clone()).unwrap();
        let (method, params) = request.envelope().unwrap();
        assert_eq!(method, "pane.move");
        assert_eq!(params, value["params"]);
    }

    #[test]
    fn plugin_pane_targets_follow_official_placement_contract() {
        for (placement, workspace_id, target_pane_id) in [
            ("popup", None, None),
            ("overlay", None, None),
            ("split", None, Some("w1:p1")),
            ("zoomed", None, Some("w1:p1")),
            ("tab", Some("w1"), None),
        ] {
            let request: HerdrFeatureRequest = serde_json::from_value(json!({
                "method": "plugin.pane.open",
                "params": {"plugin_id":"example.test", "entrypoint":"board", "placement":placement,
                    "workspace_id":workspace_id, "target_pane_id":target_pane_id, "focus":true}
            }))
            .unwrap();
            let (_, params) = request.envelope().unwrap();
            assert_eq!(params["workspace_id"], json!(workspace_id));
            assert_eq!(params["target_pane_id"], json!(target_pane_id));
        }
        // Popup/overlay omit target fields entirely in the workbench request.
        let omitted: HerdrFeatureRequest = serde_json::from_value(json!({
            "method":"plugin.pane.open", "params":{"plugin_id":"example.test",
                "entrypoint":"board", "placement":"popup", "focus":true}
        }))
        .unwrap();
        assert!(omitted.envelope().is_ok());
        for placement in ["popup", "overlay", "split", "zoomed", "tab", "unknown"] {
            let incompatible: HerdrFeatureRequest = serde_json::from_value(json!({
                "method":"plugin.pane.open", "params":{"plugin_id":"example.test",
                    "entrypoint":"board", "placement":placement, "workspace_id":"w1",
                    "target_pane_id":"w1:p1", "focus":true}
            }))
            .unwrap();
            assert!(incompatible.envelope().is_err());
        }
    }

    #[test]
    fn rejects_unbounded_waits_empty_targets_and_nul_values_before_io() {
        for value in [
            json!({"method":"agent.wait","params":{"target":"w1:p1","until":[],"timeout_ms":0}}),
            json!({"method":"agent.wait","params":{"target":"w1:p1","until":[],"timeout_ms":120001}}),
            json!({"method":"agent.rename","params":{"target":"","name":"reviewer"}}),
            json!({"method":"plugin.install","params":{"source":"owner/repo\u{0000}","revision":null}}),
        ] {
            assert!(serde_json::from_value::<HerdrFeatureRequest>(value)
                .unwrap()
                .envelope()
                .is_err());
        }
        for name in ["", "../work", "a\nb", "a b", ".", ".."] {
            assert!(validate_session_name(name).is_err());
        }
        assert!(validate_session_name("review-1").is_ok());
    }
}
