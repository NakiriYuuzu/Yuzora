//! Windows WSL runtime helpers.
//!
//! WSL is an execution boundary, not a path spelling. This module never opens a
//! WSL Unix socket from Windows. It only launches `wsl.exe` with an argv array,
//! and every selected-distro request runs `herdr` inside that distro.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crate::herdr_limits::{
    parse_herdr_cli_stdout, validate_json_complexity, MAX_LAYOUT_DEPTH, MAX_NDJSON_LINE_BYTES,
    MAX_PANE_COUNT,
};
use crate::herdr_service::{
    collect_schema_methods, parse_session_list_json, parse_snapshot_response, wait_bounded_child,
    HerdrApiCapability, HerdrBinarySource, HerdrBinarySourceInfo, HerdrCapabilities,
    HerdrEventsCapability, HerdrEventsStatus, HerdrManager, HerdrNamedSession,
    HerdrScrollDirection, HerdrServerCapability, HerdrSnapshotResult, HerdrTerminalCapability,
    HerdrTerminalMode, HerdrTerminalOpenResult, HerdrTransportDiagnostics, OnSubscriptionEvent,
    OnTerminalEvent,
};
use crate::process_kill;

use super::{HerdrRuntimeProvider, HerdrRuntimeTarget, WslControlPlan};

const WSL_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const WSL_DISTRO_MAX_LEN: usize = 256;
const WSL_HERDR_LAUNCHER_ARG0: &str = "yuzora-wsl-herdr";
const WSL_HERDR_NOT_FOUND_DIAGNOSTIC: &str =
    "Yuzora: Herdr may be installed but unavailable to the non-interactive WSL runtime.";
/// Kept constant so no distro, session, target, or user argument is ever
/// interpolated into shell source. `"$@"` forwards Herdr arguments unchanged.
///
/// Yuzora verifies the directly selected Herdr target is a canonical Linux ELF
/// before `exec`. WSLInterop remains enabled for that Herdr process and its
/// descendants; user-controlled ELF launchers that intentionally hand off to
/// Windows are outside this launcher boundary.
const WSL_HERDR_LAUNCHER: &str = r#"
locations="$HOME/.local/bin/herdr, $HOME/.cargo/bin/herdr, $HOME/.nix-profile/bin/herdr, /nix/var/nix/profiles/default/bin/herdr, $HOME/.linuxbrew/bin/herdr, /home/linuxbrew/.linuxbrew/bin/herdr, /usr/local/bin/herdr, /usr/bin/herdr, /bin/herdr, Linux-native mise which herdr, inherited non-interactive PATH"
resolve_linux_elf() {
  candidate="$1"
  [ -x "$candidate" ] || return 1
  resolved="$(readlink -f -- "$candidate" 2>/dev/null)" || return 1
  case "$resolved" in
    /mnt/*|/run/desktop/mnt/*|*.[Ee][Xx][Ee])
      printf '%s %s\n' 'Yuzora: refusing Windows-interoperability Herdr executable:' "$resolved" >&2
      return 1
      ;;
    /*) ;;
    *) return 1 ;;
  esac
  [ -x "$resolved" ] || return 1
  magic="$(od -An -N4 -tx1 "$resolved" 2>/dev/null | tr -d '[:space:]')" || return 1
  if [ "$magic" != '7f454c46' ]; then
    printf '%s %s (magic %s)\n' 'Yuzora: refusing non-Linux-ELF Herdr executable:' "$resolved" "${magic:-unreadable}" >&2
    return 1
  fi
  printf '%s\n' "$resolved"
}
launch_mise_herdr() {
  mise_candidate="$1"
  shift
  mise="$(resolve_linux_elf "$mise_candidate")" || return 1
  mise_target="$("$mise" which herdr 2>/dev/null)" || return 1
  case "$mise_target" in
    *'
'*)
      printf '%s\n' 'Yuzora: refusing ambiguous mise which herdr output.' >&2
      return 1
      ;;
    /*)
      resolved="$(resolve_linux_elf "$mise_target")" || return 1
      exec "$resolved" "$@"
      ;;
    *) return 1 ;;
  esac
}
for candidate in \
  "$HOME/.local/bin/herdr" \
  "$HOME/.cargo/bin/herdr" \
  "$HOME/.nix-profile/bin/herdr" \
  "/nix/var/nix/profiles/default/bin/herdr" \
  "$HOME/.linuxbrew/bin/herdr" \
  "/home/linuxbrew/.linuxbrew/bin/herdr" \
  "/usr/local/bin/herdr" \
  "/usr/bin/herdr" \
  "/bin/herdr"
do
  if resolved="$(resolve_linux_elf "$candidate")"; then
    exec "$resolved" "$@"
  fi
done
for candidate in \
  "$HOME/.local/bin/mise" \
  "$HOME/.cargo/bin/mise" \
  "/usr/local/bin/mise" \
  "/usr/bin/mise" \
  "/bin/mise"
do
  launch_mise_herdr "$candidate" "$@"
done
candidate="$(command -v mise 2>/dev/null || true)"
case "$candidate" in /*) launch_mise_herdr "$candidate" "$@" ;; esac
candidate="$(command -v herdr 2>/dev/null || true)"
if resolved="$(resolve_linux_elf "$candidate")"; then
  exec "$resolved" "$@"
fi
printf '%s Searched: %s\n' 'Yuzora: Herdr may be installed but unavailable to the non-interactive WSL runtime.' "$locations" >&2
exit 127
"#;
/// Snapshot enrichment is best-effort and must never become one wsl.exe spawn
/// per pane in a large workspace.
const MAX_SNAPSHOT_PATH_CONVERSIONS: usize = 64;

/// A discovered distro. Listing only executes `wsl.exe --list --quiet` and
/// never starts an installed distro.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWslDistribution {
    pub distro: String,
}

/// Runtime/host path pair. Both are operational values; `display_path` is
/// strictly presentation data.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrWslWorkspaceLocation {
    pub distro: String,
    pub runtime_path: String,
    pub host_path: String,
    pub display_path: String,
}

/// One argv-only Windows process launch. Tests inject an executor for this
/// plan, so WSL behavior is exercised on any host without an actual distro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WslCommandPlan {
    pub args: Vec<String>,
    pub timeout: Duration,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
    /// `wsl.exe --list` is host-only; every `--distribution --exec` plan can
    /// start precisely one explicitly selected distro.
    pub starts_selected_distro: bool,
}

/// Distinguishes failures before a WSL child exists from failures after it
/// could have delivered a command to Herdr. Mutation callers must never treat
/// the latter as safely retryable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WslCommandFailure {
    NotStarted(String),
    Started(String),
}

impl WslCommandFailure {
    fn message(self) -> String {
        match self {
            Self::NotStarted(message) | Self::Started(message) => message,
        }
    }

    fn may_have_started(&self) -> bool {
        matches!(self, Self::Started(_))
    }
}

pub(crate) trait WslCommandExecutor {
    fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, WslCommandFailure>;
}

struct SystemWslCommandExecutor;

impl WslCommandExecutor for SystemWslCommandExecutor {
    fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, WslCommandFailure> {
        require_windows().map_err(WslCommandFailure::NotStarted)?;
        let mut command = Command::new("wsl.exe");
        command
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_hidden_process(&mut command);
        wait_for_wsl_command(command, plan.timeout).map_err(|error| {
            if error.starts_with("spawn wsl.exe failed") {
                WslCommandFailure::NotStarted(format!("WSL executable unavailable: {error}"))
            } else if error.contains("timed out") {
                WslCommandFailure::Started(format!(
                    "WSL command timed out; Yuzora-owned child tree was terminated: {error}"
                ))
            } else {
                WslCommandFailure::Started(error)
            }
        })
    }
}

fn list_distributions_plan(timeout: Duration) -> WslCommandPlan {
    WslCommandPlan {
        args: vec!["--list".into(), "--quiet".into()],
        timeout,
        stdout_limit: MAX_NDJSON_LINE_BYTES,
        stderr_limit: MAX_NDJSON_LINE_BYTES,
        starts_selected_distro: false,
    }
}

fn selected_distro_command_plan(
    distro: &str,
    command: &[&str],
    timeout: Duration,
) -> Result<WslCommandPlan, String> {
    Ok(WslCommandPlan {
        args: wsl_selected_distro_exec_args(
            distro,
            command.iter().map(|value| (*value).to_string()).collect(),
        )?,
        timeout,
        stdout_limit: MAX_NDJSON_LINE_BYTES,
        stderr_limit: MAX_NDJSON_LINE_BYTES,
        starts_selected_distro: true,
    })
}

fn herdr_command_plan(
    distro: &str,
    session_name: Option<&str>,
    command: &[&str],
    timeout: Duration,
) -> Result<WslCommandPlan, String> {
    Ok(WslCommandPlan {
        args: wsl_exec_args(distro, session_name, command)?,
        timeout,
        stdout_limit: MAX_NDJSON_LINE_BYTES,
        stderr_limit: MAX_NDJSON_LINE_BYTES,
        starts_selected_distro: true,
    })
}

/// Adapter for a Herdr process that runs inside one selected WSL distro.
pub struct WslHerdrRuntimeProvider;

impl HerdrRuntimeProvider for WslHerdrRuntimeProvider {
    fn target(&self) -> HerdrRuntimeTarget {
        HerdrRuntimeTarget::Wsl {
            distro: "".to_string(),
        }
    }

    fn validate(&self, target: &HerdrRuntimeTarget) -> Result<(), String> {
        let HerdrRuntimeTarget::Wsl { distro } = target else {
            return Err(format!(
                "WSL Herdr provider cannot serve runtime target {target}"
            ));
        };
        validate_distro(distro)?;
        if cfg!(windows) {
            Ok(())
        } else {
            Err("WSL Herdr runtime is available only on Windows hosts".into())
        }
    }

    fn list_sessions(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        _manager: &HerdrManager,
    ) -> Result<Vec<HerdrNamedSession>, String> {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return Err(format!(
                "WSL Herdr provider cannot serve runtime target {runtime_target}"
            ));
        };
        list_sessions(distro)
    }

    fn capabilities(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> HerdrCapabilities {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return unavailable_capabilities("WSL distribution is not selected");
        };
        capabilities(distro, session_name, manager)
    }

    fn snapshot(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        _manager: &HerdrManager,
        session_name: Option<&str>,
    ) -> Result<HerdrSnapshotResult, String> {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return Err("WSL snapshot requires a selected distribution".into());
        };
        snapshot(distro, session_name)
    }

    fn request(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_name: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return Err("WSL API request requires a selected distribution".into());
        };
        let session = resolve_named_session(distro, session_name)?;
        if !session.running {
            return Err(stopped_session_message(&session));
        }
        let plan = match manager.wsl_control_plan(distro, &session.name) {
            Some(plan) => plan,
            None => {
                let (plan, _) = probe_control_plan(distro, &session, manager);
                manager.set_wsl_control_plan(distro, &session.name, plan.clone());
                plan
            }
        };
        match plan {
            WslControlPlan::Proxy => {
                // A proxy write may have reached Herdr before this transport call
                // fails. Never replay it through the CLI map here.
                manager.wsl_proxy_request(distro, &session.name, method, params)
            }
            WslControlPlan::OfficialCliV080 { .. } => {
                dispatch_official_cli_v080(distro, &session.name, method, params, manager)
            }
            WslControlPlan::ReadOnly { reason } => Err(reason),
        }
    }

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
    ) -> Result<HerdrTerminalOpenResult, String> {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return Err("WSL terminal connector requires a selected distribution".into());
        };
        let session = resolve_named_session(distro, session_name.as_deref())?;
        if !session.running {
            return Err(stopped_session_message(&session));
        }
        let plan = match manager.wsl_control_plan(distro, &session.name) {
            Some(plan) => plan,
            None => {
                let (plan, _) = probe_control_plan(distro, &session, manager);
                manager.set_wsl_control_plan(distro, &session.name, plan.clone());
                plan
            }
        };
        ensure_terminal_connector_plan(&plan)?;
        manager.open_wsl_terminal(
            distro,
            target,
            mode,
            takeover,
            cols,
            rows,
            Some(session.name),
            on_event,
        )
    }

    fn terminal_input(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        manager.require_wsl_terminal_control_plan(session_id)?;
        manager.terminal_input_for_runtime(runtime_target, session_id, text, bytes_base64)
    }

    fn terminal_resize(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        manager.require_wsl_terminal_control_plan(session_id)?;
        manager.terminal_resize_for_runtime(runtime_target, session_id, cols, rows)
    }

    fn terminal_scroll(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
        direction: HerdrScrollDirection,
        lines: u32,
    ) -> Result<(), String> {
        manager.require_wsl_terminal_control_plan(session_id)?;
        manager.terminal_scroll_for_runtime(runtime_target, session_id, direction, lines)
    }

    fn terminal_release(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        session_id: &str,
    ) -> Result<(), String> {
        manager.terminal_release_for_runtime(runtime_target, session_id)
    }

    fn subscribe(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &Arc<HerdrManager>,
        session_name: Option<String>,
        on_event: OnSubscriptionEvent,
    ) -> Result<String, String> {
        let HerdrRuntimeTarget::Wsl { distro } = runtime_target else {
            return Err("WSL events require a selected distribution".into());
        };
        let session = resolve_named_session(distro, session_name.as_deref())?;
        if !matches!(
            manager.wsl_control_plan(distro, &session.name),
            Some(WslControlPlan::Proxy)
        ) {
            return Err(
                "WSL public CLI compatibility mode does not support events.subscribe".into(),
            );
        }
        manager.wsl_events_subscribe(distro, Some(session.name), on_event)
    }

    fn release_subscription(
        &self,
        runtime_target: &HerdrRuntimeTarget,
        manager: &HerdrManager,
        subscription_id: &str,
    ) -> Result<(), String> {
        manager.wsl_events_release_for_runtime(runtime_target, subscription_id)
    }
}

/// Windows-only safe distro enumeration. It intentionally does not select or
/// execute inside any distro.
pub fn list_distributions() -> Result<Vec<HerdrWslDistribution>, String> {
    list_distributions_with(&SystemWslCommandExecutor)
}

fn list_distributions_with(
    executor: &impl WslCommandExecutor,
) -> Result<Vec<HerdrWslDistribution>, String> {
    let output = executor
        .execute(&list_distributions_plan(WSL_COMMAND_TIMEOUT))
        .map_err(WslCommandFailure::message)?;
    let names = decode_wsl_list_output(&output)?;
    Ok(names
        .into_iter()
        .map(|distro| HerdrWslDistribution { distro })
        .collect())
}

/// Run a JSON-emitting Herdr CLI command inside the explicitly selected distro.
pub fn run_herdr_json(
    distro: &str,
    session_name: Option<&str>,
    args: &[&str],
) -> Result<serde_json::Value, String> {
    run_herdr_json_with(&SystemWslCommandExecutor, distro, session_name, args)
}

fn run_herdr_json_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: Option<&str>,
    args: &[&str],
) -> Result<serde_json::Value, String> {
    let output = run_herdr_capture_with(executor, distro, session_name, args, WSL_COMMAND_TIMEOUT)?;
    parse_herdr_cli_stdout(&output).map_err(String::from)
}

pub fn list_sessions(distro: &str) -> Result<Vec<HerdrNamedSession>, String> {
    let value = run_herdr_json(distro, None, &["session", "list", "--json"])?;
    parse_session_list_json(&value)
}

pub fn resolve_named_session(
    distro: &str,
    session_name: Option<&str>,
) -> Result<HerdrNamedSession, String> {
    let sessions = list_sessions(distro)?;
    let requested = session_name.map(str::trim).filter(|name| !name.is_empty());
    match requested {
        Some(name) if name != "live" => sessions
            .into_iter()
            .find(|session| session.name == name)
            .ok_or_else(|| format!("herdr session not found in WSL {distro}: {name}")),
        _ => sessions
            .iter()
            .find(|session| session.default)
            .cloned()
            .or_else(|| sessions.into_iter().next())
            .ok_or_else(|| format!("no herdr named sessions found in WSL {distro}")),
    }
}

/// CLI fallback snapshot. This remains useful when a selected distro has not
/// yet installed a Herdr version with `api proxy --stdio`. Host-path enrichment
/// is intentionally outside the authoritative snapshot read so mutation
/// serialization never fans out into one `wslpath` child per path.
pub fn snapshot(distro: &str, session_name: Option<&str>) -> Result<HerdrSnapshotResult, String> {
    let session = resolve_named_session(distro, session_name)?;
    if !session.running {
        return Err(stopped_session_message(&session));
    }
    let mut parsed =
        raw_snapshot_for_session_with(&SystemWslCommandExecutor, distro, &session.name)?;
    enrich_snapshot_paths(distro, &mut parsed.snapshot);
    Ok(parsed)
}

/// Read and validate a single authoritative snapshot without optional path
/// conversion. Callers that serialize topology mutations use this exact helper
/// while holding their RuntimeKey lock.
fn raw_snapshot_for_session_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: &str,
) -> Result<HerdrSnapshotResult, String> {
    let response = run_herdr_json_with(executor, distro, Some(session_name), &["api", "snapshot"])?;
    parse_snapshot_response(response)
}

/// Probe the selected WSL runtime/session. A healthy proxy always wins. When
/// it is absent, only the verified public 0.8.0 CLI dialect is writable; every
/// other version remains fail-closed read-only.
pub fn capabilities(
    distro: &str,
    session_name: Option<&str>,
    manager: &HerdrManager,
) -> HerdrCapabilities {
    let session = match resolve_named_session(distro, session_name) {
        Ok(session) => session,
        Err(error) => return unavailable_capabilities(&error),
    };
    let (plan, mut caps) = probe_control_plan(distro, &session, manager);
    manager.set_wsl_control_plan(distro, &session.name, plan.clone());
    match plan {
        WslControlPlan::Proxy => {
            caps.transport = Some(manager.wsl_proxy_diagnostics(distro, &session.name));
        }
        WslControlPlan::OfficialCliV080 { .. } => {
            apply_cli_v080_method_flags(&mut caps.api);
            caps.api.reason = Some(
                "WSL public CLI compatibility control mode (verified Herdr 0.8.0); polling replaces events."
                    .into(),
            );
            caps.terminal = HerdrTerminalCapability {
                create: true,
                reason: Some("WSL public CLI compatibility control mode".into()),
                ..terminal_capability(true, "")
            };
            caps.events = HerdrEventsCapability {
                status: HerdrEventsStatus::Unavailable,
                reason: Some(
                    "WSL public CLI compatibility mode polls; events.subscribe is unavailable"
                        .into(),
                ),
            };
            caps.transport = Some(cli_control_diagnostics());
        }
        WslControlPlan::ReadOnly { reason } => {
            caps.api.reason = Some(reason.clone());
            caps.terminal = terminal_capability(
                false,
                "WSL terminal connector requires a healthy stdio proxy or the verified official Herdr 0.8.0 protocol 20 dialect",
            );
            caps.events = HerdrEventsCapability {
                status: HerdrEventsStatus::Unavailable,
                reason: Some("WSL read-only mode does not support events.subscribe".into()),
            };
            caps.transport = Some(cli_read_only_diagnostics(&reason));
        }
    }
    caps
}

/// Produce capabilities plus a selected plan. It intentionally does not reuse
/// a previous result: calling `herdr_capabilities` is the explicit re-probe
/// that may upgrade CLI control back to a recovered proxy.
fn probe_control_plan(
    distro: &str,
    session: &HerdrNamedSession,
    manager: &HerdrManager,
) -> (WslControlPlan, HerdrCapabilities) {
    let proxy_healthy = session.running
        && manager
            .wsl_proxy_request(distro, &session.name, "ping", serde_json::json!({}))
            .is_ok();
    probe_control_plan_with(&SystemWslCommandExecutor, distro, session, proxy_healthy)
}

fn probe_control_plan_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session: &HerdrNamedSession,
    proxy_healthy: bool,
) -> (WslControlPlan, HerdrCapabilities) {
    let mut caps = unavailable_capabilities("Herdr in selected WSL distro is unavailable");
    caps.binary_path = Some(format!("wsl:{distro}:herdr"));
    caps.binary_source.available = true;
    caps.binary_source.path = caps.binary_path.clone();
    caps.binary_source.reason = None;
    caps.server.running = session.running;
    caps.server.socket_path = session.running.then_some(session.socket_path.clone());
    if !session.running {
        let reason = stopped_session_message(session);
        return (WslControlPlan::ReadOnly { reason }, caps);
    }

    let status = run_herdr_json_with(executor, distro, Some(&session.name), &["status", "--json"]);
    if let Ok(status) = status {
        apply_wsl_status(&mut caps, &status);
    } else {
        let reason = "herdr status unavailable in selected WSL distro".to_string();
        return (WslControlPlan::ReadOnly { reason }, caps);
    }

    let schema = match run_herdr_json_with(
        executor,
        distro,
        Some(&session.name),
        &["api", "schema", "--json"],
    ) {
        Ok(schema) => schema,
        Err(_) => {
            return (
                WslControlPlan::ReadOnly {
                    reason: "herdr api schema unavailable in selected WSL distro".into(),
                },
                caps,
            );
        }
    };
    let methods = collect_schema_methods(&schema);
    caps.api.schema_protocol = schema
        .get("protocol")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    caps.api.schema_version = schema
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    caps.api.snapshot = methods.contains("session.snapshot");

    if proxy_healthy {
        apply_proxy_method_flags(&mut caps.api, &methods);
        caps.api.reason = None;
        caps.terminal = HerdrTerminalCapability {
            create: caps.api.tab_create,
            reason: None,
            ..terminal_capability(true, "")
        };
        caps.events = HerdrEventsCapability {
            status: if caps.api.events_subscribe {
                HerdrEventsStatus::Available
            } else {
                HerdrEventsStatus::Unavailable
            },
            reason: (!caps.api.events_subscribe)
                .then_some("selected Herdr schema lacks events.subscribe".into()),
        };
        return (WslControlPlan::Proxy, caps);
    }

    if verified_official_cli_v080(&caps, &methods) {
        return (
            WslControlPlan::OfficialCliV080 {
                version: "0.8.0".into(),
                protocol: 20,
            },
            caps,
        );
    }
    (
        WslControlPlan::ReadOnly {
            reason: "WSL proxy unavailable and installed Herdr is not the verified official 0.8.0 protocol 20 CLI dialect"
                .into(),
        },
        caps,
    )
}

fn apply_wsl_status(caps: &mut HerdrCapabilities, status: &serde_json::Value) {
    let client = status.get("client").unwrap_or(status);
    caps.binary_version = client
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    caps.binary_protocol = client
        .get("protocol")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    let server = status.get("server").unwrap_or(status);
    caps.server.version = server
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    caps.server.protocol = server
        .get("protocol")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    caps.server.compatible = server
        .get("compatible")
        .and_then(serde_json::Value::as_bool);
}

const OFFICIAL_CLI_V080_METHODS: &[&str] = &[
    "session.snapshot",
    "workspace.create",
    "workspace.focus",
    "workspace.rename",
    "workspace.close",
    "tab.create",
    "tab.focus",
    "tab.rename",
    "tab.close",
    "pane.rename",
    "pane.split",
    "pane.zoom",
    "pane.swap",
    "pane.close",
    "pane.layout",
    "worktree.list",
];

fn is_official_cli_v080(version: &str) -> bool {
    version == "0.8.0"
}

fn cli_v080_schema_is_usable(methods: &std::collections::HashSet<String>) -> bool {
    OFFICIAL_CLI_V080_METHODS
        .iter()
        .all(|method| methods.contains(*method))
}

fn verified_official_cli_v080(
    caps: &HerdrCapabilities,
    methods: &std::collections::HashSet<String>,
) -> bool {
    is_official_cli_v080(caps.binary_version.as_deref().unwrap_or_default())
        && is_official_cli_v080(caps.server.version.as_deref().unwrap_or_default())
        && caps.binary_protocol == Some(20)
        && caps.server.protocol == Some(20)
        && caps.server.compatible == Some(true)
        && caps.api.schema_protocol == Some(20)
        && caps.api.schema_version == Some(1)
        && cli_v080_schema_is_usable(methods)
}

fn ensure_terminal_connector_plan(plan: &WslControlPlan) -> Result<(), String> {
    match plan {
        WslControlPlan::Proxy | WslControlPlan::OfficialCliV080 { .. } => Ok(()),
        WslControlPlan::ReadOnly { reason } => {
            Err(format!("WSL terminal connector unavailable: {reason}"))
        }
    }
}

fn cli_control_diagnostics() -> HerdrTransportDiagnostics {
    HerdrTransportDiagnostics {
        mode: "wsl-public-cli-control".into(),
        state: "available".into(),
        generation: None,
        pending_requests: 0,
        event_listeners: 0,
        active_children: 0,
        requests: 0,
        responses: 0,
        events_delivered: 0,
        stale_events_dropped: 0,
        cold_start_ms: None,
        last_request_ms: None,
        max_request_ms: 0,
        last_event_dispatch_ms: None,
        max_event_dispatch_ms: 0,
        failure: None,
    }
}

fn cli_read_only_diagnostics(reason: &str) -> HerdrTransportDiagnostics {
    HerdrTransportDiagnostics {
        mode: "wsl-cli-read-only".into(),
        state: "degraded".into(),
        failure: Some(reason.into()),
        ..cli_control_diagnostics()
    }
}

/// Closed public-CLI command map for the documented Herdr 0.8 surface. This
/// is deliberately not a generic `herdr` command runner: every accepted method
/// and every argument is validated and emitted as its own argv entry.
fn dispatch_official_cli_v080(
    distro: &str,
    session_name: &str,
    method: &str,
    params: serde_json::Value,
    manager: &HerdrManager,
) -> Result<serde_json::Value, String> {
    dispatch_official_cli_v080_with(
        &SystemWslCommandExecutor,
        distro,
        session_name,
        method,
        params,
        manager,
    )
}

fn dispatch_official_cli_v080_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: &str,
    method: &str,
    params: serde_json::Value,
    manager: &HerdrManager,
) -> Result<serde_json::Value, String> {
    if method == "layout.export" {
        return cli_layout_export(distro, session_name, params);
    }
    let (args, mutation) = cli_v080_command_args(method, &params)?;
    if !mutation {
        let response = run_herdr_json_owned_with(executor, distro, Some(session_name), args)?;
        ensure_cli_success(&response)?;
        return Ok(response);
    }

    let lock = manager
        .wsl_cli_topology_lock(distro, session_name)
        .ok_or_else(|| "WSL distribution name is invalid".to_string())?;
    let _guard = lock.lock().unwrap();
    let response =
        match run_herdr_json_owned_for_mutation_with(executor, distro, Some(session_name), args) {
            Ok(response) => response,
            Err(failure) if failure.may_have_started() => {
                return Err(reconcile_unknown_mutation_outcome_with(
                    executor,
                    distro,
                    session_name,
                    failure,
                ));
            }
            Err(failure) => return Err(failure.message()),
        };
    if let Err(error) = ensure_cli_success(&response) {
        // An explicit Herdr error is authoritative. A syntactically valid but
        // incomplete response is not: the child may have applied the command
        // before its output was truncated or corrupted.
        if response.get("error").is_some() {
            return Err(error);
        }
        return Err(reconcile_unknown_mutation_outcome_with(
            executor,
            distro,
            session_name,
            WslCommandFailure::Started(error),
        ));
    }
    // This is exactly one raw authoritative read while the RuntimeKey lock is
    // still held. Do not enrich paths here: `wslpath` fanout belongs to the
    // normal UI snapshot path after this mutation is released.
    raw_snapshot_for_session_with(executor, distro, session_name).map_err(|error| {
        format!(
            "WSL CLI mutation applied but immediate raw snapshot reconciliation failed: {error}"
        )
    })?;
    Ok(response)
}

fn run_herdr_json_owned_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: Option<&str>,
    args: Vec<String>,
) -> Result<serde_json::Value, String> {
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_herdr_json_with(executor, distro, session_name, &refs)
}

fn run_herdr_json_owned_for_mutation_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: Option<&str>,
    args: Vec<String>,
) -> Result<serde_json::Value, WslCommandFailure> {
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let plan = herdr_command_plan(distro, session_name, &refs, WSL_COMMAND_TIMEOUT)
        .map_err(WslCommandFailure::NotStarted)?;
    let output = executor.execute(&plan)?;
    parse_herdr_cli_stdout(&output).map_err(|error| WslCommandFailure::Started(error.into()))
}

fn reconcile_unknown_mutation_outcome_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: &str,
    failure: WslCommandFailure,
) -> String {
    let failure = failure.message();
    match raw_snapshot_for_session_with(executor, distro, session_name) {
        Ok(_) => format!(
            "WSL CLI mutation outcome unknown: {failure}; may have applied; do not retry blindly; raw snapshot reconciliation succeeded"
        ),
        Err(snapshot_error) => format!(
            "WSL CLI mutation outcome unknown: {failure}; may have applied; do not retry blindly; raw snapshot reconciliation failed: {snapshot_error}"
        ),
    }
}

fn ensure_cli_success(response: &serde_json::Value) -> Result<(), String> {
    if let Some(error) = response.get("error") {
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("error");
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!("{code}: {message}"));
    }
    if response.get("result").is_none() {
        return Err("Herdr public CLI response missing result".into());
    }
    Ok(())
}

fn cli_v080_command_args(
    method: &str,
    params: &serde_json::Value,
) -> Result<(Vec<String>, bool), String> {
    let object = params
        .as_object()
        .ok_or_else(|| format!("{method} parameters must be an object"))?;
    match method {
        "session.snapshot" => {
            require_only_keys(method, object, &[])?;
            Ok((vec!["api".into(), "snapshot".into()], false))
        }
        "workspace.create" => {
            require_only_keys(method, object, &["cwd", "label", "focus"])?;
            let mut args = vec!["workspace".into(), "create".into()];
            push_optional_flag(&mut args, "--cwd", optional_string(object, "cwd")?);
            push_optional_flag(&mut args, "--label", optional_string(object, "label")?);
            push_focus(&mut args, optional_bool(object, "focus")?.unwrap_or(false));
            Ok((args, true))
        }
        "workspace.focus" => {
            require_only_keys(method, object, &["workspace_id"])?;
            Ok((
                vec![
                    "workspace".into(),
                    "focus".into(),
                    required_string(object, "workspace_id")?,
                ],
                true,
            ))
        }
        "workspace.rename" => {
            require_only_keys(method, object, &["workspace_id", "label"])?;
            Ok((
                vec![
                    "workspace".into(),
                    "rename".into(),
                    required_string(object, "workspace_id")?,
                    required_string(object, "label")?,
                ],
                true,
            ))
        }
        "workspace.close" => {
            require_only_keys(method, object, &["workspace_id"])?;
            Ok((
                vec![
                    "workspace".into(),
                    "close".into(),
                    required_string(object, "workspace_id")?,
                ],
                true,
            ))
        }
        "tab.create" => {
            require_only_keys(method, object, &["workspace_id", "cwd", "label", "focus"])?;
            let mut args = vec!["tab".into(), "create".into()];
            push_optional_flag(
                &mut args,
                "--workspace",
                optional_string(object, "workspace_id")?,
            );
            push_optional_flag(&mut args, "--cwd", optional_string(object, "cwd")?);
            push_optional_flag(&mut args, "--label", optional_string(object, "label")?);
            push_focus(&mut args, optional_bool(object, "focus")?.unwrap_or(false));
            Ok((args, true))
        }
        "tab.focus" => {
            require_only_keys(method, object, &["tab_id"])?;
            Ok((
                vec![
                    "tab".into(),
                    "focus".into(),
                    required_string(object, "tab_id")?,
                ],
                true,
            ))
        }
        "tab.rename" => {
            require_only_keys(method, object, &["tab_id", "label"])?;
            Ok((
                vec![
                    "tab".into(),
                    "rename".into(),
                    required_string(object, "tab_id")?,
                    required_string(object, "label")?,
                ],
                true,
            ))
        }
        "tab.close" => {
            require_only_keys(method, object, &["tab_id"])?;
            Ok((
                vec![
                    "tab".into(),
                    "close".into(),
                    required_string(object, "tab_id")?,
                ],
                true,
            ))
        }
        "pane.rename" => {
            require_only_keys(method, object, &["pane_id", "label"])?;
            let pane_id = required_string(object, "pane_id")?;
            let label = match object.get("label") {
                None | Some(serde_json::Value::Null) => "--clear".to_string(),
                Some(serde_json::Value::String(label))
                    if !label.trim().is_empty() && label != "--clear" =>
                {
                    label.clone()
                }
                Some(serde_json::Value::String(label)) if label == "--clear" => {
                    return Err(
                        "pane.rename cannot use literal --clear in WSL public CLI mode".into(),
                    )
                }
                Some(_) => {
                    return Err("pane.rename label must be a non-empty string or null".into())
                }
            };
            Ok((vec!["pane".into(), "rename".into(), pane_id, label], true))
        }
        "pane.split" => {
            require_only_keys(
                method,
                object,
                &[
                    "direction",
                    "target_pane_id",
                    "workspace_id",
                    "cwd",
                    "ratio",
                    "focus",
                ],
            )?;
            let direction = required_enum(object, "direction", &["right", "down"])?;
            let target_pane = optional_string(object, "target_pane_id")?;
            // The public CLI derives the workspace from its explicit target
            // pane. The IPC field is redundant metadata from the generic raw
            // API and is accepted only with that target; it is never emitted
            // as a synthetic or unscoped CLI operation.
            if optional_string(object, "workspace_id")?.is_some() && target_pane.is_none() {
                return Err(
                    "pane.split workspace_id requires target_pane_id in WSL public CLI mode".into(),
                );
            }
            let mut args = vec!["pane".into(), "split".into()];
            if let Some(pane) = target_pane {
                args.push("--pane".into());
                args.push(pane);
            }
            args.push("--direction".into());
            args.push(direction);
            if let Some(ratio) = optional_ratio(object, "ratio")? {
                args.push("--ratio".into());
                args.push(ratio.to_string());
            }
            push_optional_flag(&mut args, "--cwd", optional_string(object, "cwd")?);
            push_focus(&mut args, optional_bool(object, "focus")?.unwrap_or(false));
            Ok((args, true))
        }
        "pane.zoom" => {
            require_only_keys(method, object, &["pane_id", "mode"])?;
            let mut args = vec!["pane".into(), "zoom".into()];
            if let Some(pane) = optional_string(object, "pane_id")? {
                args.push("--pane".into());
                args.push(pane);
            }
            let mode = optional_string(object, "mode")?.unwrap_or_else(|| "toggle".into());
            let flag = match mode.as_str() {
                "toggle" => "--toggle",
                "on" => "--on",
                "off" => "--off",
                _ => return Err("pane.zoom mode must be toggle, on, or off".into()),
            };
            args.push(flag.into());
            Ok((args, true))
        }
        "pane.swap" => {
            require_only_keys(
                method,
                object,
                &["source_pane_id", "target_pane_id", "pane_id", "direction"],
            )?;
            let source = optional_string(object, "source_pane_id")?;
            let target = optional_string(object, "target_pane_id")?;
            let pane = optional_string(object, "pane_id")?;
            let direction = optional_string(object, "direction")?;
            let mut args = vec!["pane".into(), "swap".into()];
            match (source, target, pane, direction) {
                (Some(source), Some(target), None, None) => {
                    args.extend([
                        "--source-pane".into(),
                        source,
                        "--target-pane".into(),
                        target,
                    ]);
                }
                (None, None, pane, Some(direction))
                    if matches!(direction.as_str(), "left" | "right" | "up" | "down") =>
                {
                    if let Some(pane) = pane {
                        args.extend(["--pane".into(), pane]);
                    } else {
                        args.push("--current".into());
                    }
                    args.extend(["--direction".into(), direction]);
                }
                _ => {
                    return Err(
                        "pane.swap requires explicit source/target or pane/current plus direction"
                            .into(),
                    )
                }
            }
            Ok((args, true))
        }
        "pane.close" => {
            require_only_keys(method, object, &["pane_id"])?;
            Ok((
                vec![
                    "pane".into(),
                    "close".into(),
                    required_string(object, "pane_id")?,
                ],
                true,
            ))
        }
        "worktree.list" => {
            require_only_keys(method, object, &["cwd", "workspace_id"])?;
            let cwd = optional_string(object, "cwd")?;
            let workspace = optional_string(object, "workspace_id")?;
            if cwd.is_some() && workspace.is_some() {
                return Err("worktree.list accepts cwd or workspace_id, not both".into());
            }
            let mut args = vec!["worktree".into(), "list".into()];
            push_optional_flag(&mut args, "--cwd", cwd);
            push_optional_flag(&mut args, "--workspace", workspace);
            Ok((args, false))
        }
        // Explicitly refuse methods that only have a raw schema equivalent or
        // a non-equivalent directional CLI command.
        "events.subscribe" | "tab.move" | "pane.focus" | "layout.set_split_ratio" => Err(format!(
            "{method} is unavailable in WSL public CLI compatibility mode"
        )),
        _ => Err(format!(
            "{method} is unavailable in WSL public CLI compatibility mode"
        )),
    }
}

fn require_only_keys(
    method: &str,
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{method} does not support parameter {key} in WSL public CLI mode"
            ));
        }
    }
    Ok(())
}

fn required_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty() && !value.contains('\0'))
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

fn optional_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value))
            if !value.trim().is_empty() && !value.contains('\0') =>
        {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(format!("{key} must be a non-empty string")),
    }
}

fn optional_bool(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<bool>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

fn required_enum(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    allowed: &[&str],
) -> Result<String, String> {
    let value = required_string(object, key)?;
    if allowed.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(format!("{key} has an unsupported value"))
    }
}

fn optional_ratio(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<f64>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => {
            let value = value
                .as_f64()
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                .ok_or_else(|| format!("{key} must be a finite number between 0 and 1"))?;
            Ok(Some(value))
        }
    }
}

fn push_optional_flag(args: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(value) = value {
        args.push(flag.into());
        args.push(value);
    }
}

fn push_focus(args: &mut Vec<String>, focus: bool) {
    args.push(if focus { "--focus" } else { "--no-focus" }.into());
}

fn apply_cli_v080_method_flags(api: &mut HerdrApiCapability) {
    api.snapshot = true;
    api.ping = false;
    api.workspace_focus = true;
    api.workspace_create = true;
    api.workspace_rename = true;
    api.workspace_close = true;
    api.tab_create = true;
    api.tab_rename = true;
    api.tab_close = true;
    api.tab_focus = true;
    api.tab_move = false;
    api.pane_focus = false;
    api.pane_rename = true;
    api.pane_split = true;
    api.pane_zoom = true;
    api.pane_swap = true;
    api.pane_close = true;
    api.layout_export = true;
    api.layout_set_split_ratio = false;
    api.agent_get = false;
    api.agent_read = false;
    api.events_subscribe = false;
    api.worktree_list = true;
    api.methods = vec![
        "session.snapshot",
        "workspace.create",
        "workspace.focus",
        "workspace.rename",
        "workspace.close",
        "worktree.list",
        "tab.create",
        "tab.focus",
        "tab.rename",
        "tab.close",
        "pane.rename",
        "pane.split",
        "pane.zoom",
        "pane.swap",
        "pane.close",
        "layout.export",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
}

fn cli_layout_export(
    distro: &str,
    session_name: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let object = params
        .as_object()
        .ok_or_else(|| "layout.export parameters must be an object".to_string())?;
    require_only_keys("layout.export", object, &["tab_id", "pane_id"])?;
    let tab_id = optional_string(object, "tab_id")?;
    let pane_id = optional_string(object, "pane_id")?;
    let snapshot_response = run_herdr_json(distro, Some(session_name), &["api", "snapshot"])?;
    ensure_cli_success(&snapshot_response)?;
    if let Some(layout) =
        layout_from_snapshot_response(&snapshot_response, tab_id.as_deref(), pane_id.as_deref())?
    {
        return Ok(synthetic_layout_export_response(layout));
    }
    let pane_id = pane_id.ok_or_else(|| {
        "WSL public CLI layout export needs snapshot.layouts or an explicit pane_id for pane layout"
            .to_string()
    })?;
    let response = run_herdr_json_owned_with(
        &SystemWslCommandExecutor,
        distro,
        Some(session_name),
        vec!["pane".into(), "layout".into(), "--pane".into(), pane_id],
    )?;
    ensure_cli_success(&response)?;
    let layout = response
        .get("result")
        .and_then(|result| result.get("layout"))
        .ok_or_else(|| "pane layout response missing layout".to_string())?;
    Ok(synthetic_layout_export_response(layout_from_geometry(
        layout,
    )?))
}

fn layout_from_snapshot_response(
    response: &serde_json::Value,
    tab_id: Option<&str>,
    pane_id: Option<&str>,
) -> Result<Option<serde_json::Value>, String> {
    let snapshot = response
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .ok_or_else(|| "snapshot response missing snapshot".to_string())?;
    let Some(layouts) = snapshot
        .get("layouts")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(None);
    };
    // `layout.export {}` is defined for the active tab. Snapshot geometry
    // carries that public selection, so avoid treating a multi-tab snapshot as
    // ambiguous when callers omit both targets.
    let selected_tab = tab_id.or_else(|| {
        snapshot
            .get("focused_tab_id")
            .and_then(serde_json::Value::as_str)
    });
    let mut matches = layouts.iter().filter(|layout| {
        let tab_matches = selected_tab.is_none_or(|tab| {
            layout.get("tab_id").and_then(serde_json::Value::as_str) == Some(tab)
        });
        let pane_matches = pane_id.is_none_or(|pane| {
            layout
                .get("panes")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|panes| {
                    panes.iter().any(|item| {
                        item.get("pane_id").and_then(serde_json::Value::as_str) == Some(pane)
                    })
                })
        });
        tab_matches && pane_matches
    });
    let Some(layout) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err("snapshot layouts are ambiguous for requested tab/pane".into());
    }
    Ok(Some(layout_from_geometry(layout)?))
}

fn synthetic_layout_export_response(layout: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "yuzora:wsl-cli:layout",
        "result": { "type": "layout_export", "layout": layout }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GeometryRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl GeometryRect {
    fn right(self) -> u32 {
        self.x + self.width
    }

    fn bottom(self) -> u32 {
        self.y + self.height
    }

    fn contains(self, other: Self) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

#[derive(Clone)]
struct GeometryPane {
    id: String,
    rect: GeometryRect,
}

#[derive(Clone)]
struct GeometrySplit {
    direction: String,
    ratio: f64,
    rect: GeometryRect,
}

/// Convert documented `PaneLayoutSnapshot` geometry into the portable BSP tree
/// expected by the existing UI. It does not interpret split ids: exact public
/// rect containment, direction, and ratio are the only reconstruction inputs.
fn layout_from_geometry(layout: &serde_json::Value) -> Result<serde_json::Value, String> {
    validate_json_complexity(layout).map_err(String::from)?;
    let workspace_id = geometry_required_string(layout, "workspace_id")?;
    let tab_id = geometry_required_string(layout, "tab_id")?;
    let focused_pane_id = geometry_required_string(layout, "focused_pane_id")?;
    let area = geometry_rect(
        layout
            .get("area")
            .ok_or_else(|| "layout missing area".to_string())?,
    )?;
    let pane_values = layout
        .get("panes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "layout missing panes".to_string())?;
    if pane_values.is_empty() || pane_values.len() > MAX_PANE_COUNT {
        return Err("layout pane count is invalid".into());
    }
    let mut pane_ids = std::collections::HashSet::new();
    let panes = pane_values
        .iter()
        .map(|pane| {
            let id = geometry_required_string(pane, "pane_id")?;
            if !pane_ids.insert(id.clone()) {
                return Err("layout contains duplicate pane ids".to_string());
            }
            Ok(GeometryPane {
                id,
                rect: geometry_rect(
                    pane.get("rect")
                        .ok_or_else(|| "pane missing rect".to_string())?,
                )?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !pane_ids.contains(&focused_pane_id) {
        return Err("layout focused pane is not present".into());
    }
    let split_values = layout
        .get("splits")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "layout missing splits".to_string())?;
    if split_values.len() != panes.len().saturating_sub(1) {
        return Err("layout split count must equal pane count minus one".into());
    }
    let splits = split_values
        .iter()
        .map(|split| {
            let direction = geometry_required_string(split, "direction")?;
            if direction != "right" && direction != "down" {
                return Err("layout split direction is invalid".to_string());
            }
            let ratio = split
                .get("ratio")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite() && *value > 0.0 && *value < 1.0)
                .ok_or_else(|| {
                    "layout split ratio must be finite and strictly between 0 and 1".to_string()
                })?;
            Ok(GeometrySplit {
                direction,
                ratio,
                rect: geometry_rect(
                    split
                        .get("rect")
                        .ok_or_else(|| "split missing rect".to_string())?,
                )?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for pane in &panes {
        if !area.contains(pane.rect) {
            return Err("layout pane rect lies outside layout area".into());
        }
    }
    for split in &splits {
        if !area.contains(split.rect) {
            return Err("layout split rect lies outside layout area".into());
        }
    }

    let root = if panes.len() == 1 {
        serde_json::json!({ "type": "pane", "pane_id": panes[0].id })
    } else {
        let roots = splits
            .iter()
            .enumerate()
            .filter_map(|(index, split)| (split.rect == area).then_some(index))
            .collect::<Vec<_>>();
        if roots.len() != 1 {
            return Err("layout geometry must have exactly one root split".into());
        }
        let mut used_panes = std::collections::HashSet::new();
        let mut used_splits = std::collections::HashSet::new();
        let all_panes = (0..panes.len()).collect::<Vec<_>>();
        let root = build_geometry_tree(
            roots[0],
            &all_panes,
            &panes,
            &splits,
            &mut used_panes,
            &mut used_splits,
            0,
        )?;
        if used_panes.len() != panes.len() || used_splits.len() != splits.len() {
            return Err("layout geometry did not consume every pane and split exactly once".into());
        }
        root
    };
    Ok(serde_json::json!({
        "workspace_id": workspace_id,
        "tab_id": tab_id,
        "zoomed": layout.get("zoomed").and_then(serde_json::Value::as_bool).unwrap_or(false),
        "focused_pane_id": focused_pane_id,
        "root": root,
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_geometry_tree(
    split_index: usize,
    pane_indexes: &[usize],
    panes: &[GeometryPane],
    splits: &[GeometrySplit],
    used_panes: &mut std::collections::HashSet<usize>,
    used_splits: &mut std::collections::HashSet<usize>,
    depth: usize,
) -> Result<serde_json::Value, String> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err("layout geometry exceeds the maximum depth".into());
    }
    if !used_splits.insert(split_index) {
        return Err("layout geometry reuses a split".into());
    }
    let split = &splits[split_index];
    let (first_panes, second_panes) = partition_geometry_panes(split, pane_indexes, panes)?;
    let first = build_geometry_child(
        split,
        &first_panes,
        panes,
        splits,
        used_panes,
        used_splits,
        depth + 1,
    )?;
    let second = build_geometry_child(
        split,
        &second_panes,
        panes,
        splits,
        used_panes,
        used_splits,
        depth + 1,
    )?;
    Ok(serde_json::json!({
        "type": "split",
        "direction": split.direction,
        "ratio": split.ratio,
        "first": first,
        "second": second,
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_geometry_child(
    parent: &GeometrySplit,
    pane_indexes: &[usize],
    panes: &[GeometryPane],
    splits: &[GeometrySplit],
    used_panes: &mut std::collections::HashSet<usize>,
    used_splits: &mut std::collections::HashSet<usize>,
    depth: usize,
) -> Result<serde_json::Value, String> {
    if pane_indexes.len() == 1 {
        let index = pane_indexes[0];
        if !used_panes.insert(index) {
            return Err("layout geometry reuses a pane".into());
        }
        return Ok(serde_json::json!({ "type": "pane", "pane_id": panes[index].id }));
    }
    let candidates = splits
        .iter()
        .enumerate()
        .filter(|(index, split)| {
            !used_splits.contains(index)
                && parent.rect.contains(split.rect)
                && pane_indexes
                    .iter()
                    .all(|pane| split.rect.contains(panes[*pane].rect))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if candidates.len() != 1 {
        return Err("layout geometry is ambiguous or has no child split".into());
    }
    build_geometry_tree(
        candidates[0],
        pane_indexes,
        panes,
        splits,
        used_panes,
        used_splits,
        depth,
    )
}

fn partition_geometry_panes(
    split: &GeometrySplit,
    pane_indexes: &[usize],
    panes: &[GeometryPane],
) -> Result<(Vec<usize>, Vec<usize>), String> {
    let boundary = match split.direction.as_str() {
        "right" => split.rect.x as f64 + split.rect.width as f64 * split.ratio,
        "down" => split.rect.y as f64 + split.rect.height as f64 * split.ratio,
        _ => return Err("layout split direction is invalid".into()),
    };
    let mut first = Vec::new();
    let mut second = Vec::new();
    for index in pane_indexes {
        let rect = panes[*index].rect;
        // Pane chrome/gaps can make integer rect edges land on either side of
        // a fractional public ratio. The centre remains unambiguous; reject
        // only a centre exactly on the separator rather than guessing a side.
        let centre = match split.direction.as_str() {
            "right" => rect.x as f64 + rect.width as f64 / 2.0,
            "down" => rect.y as f64 + rect.height as f64 / 2.0,
            _ => unreachable!(),
        };
        if (centre - boundary).abs() < f64::EPSILON {
            return Err("layout pane lies on an ambiguous split boundary".into());
        }
        if centre < boundary {
            first.push(*index);
        } else {
            second.push(*index);
        }
    }
    if first.is_empty() || second.is_empty() {
        return Err("layout split does not partition panes".into());
    }
    Ok((first, second))
}

fn geometry_required_string(value: &serde_json::Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("layout missing {key}"))
}

fn geometry_rect(value: &serde_json::Value) -> Result<GeometryRect, String> {
    let number = |key: &str| -> Result<u32, String> {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .filter(|value| *value <= u32::MAX as u64)
            .map(|value| value as u32)
            .ok_or_else(|| format!("layout rect missing or invalid {key}"))
    };
    let rect = GeometryRect {
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    };
    if rect.width == 0 || rect.height == 0 || rect.right() < rect.x || rect.bottom() < rect.y {
        return Err("layout rect has invalid dimensions".into());
    }
    Ok(rect)
}

/// Convert WSL runtime path to a host path using the selected distro's
/// `wslpath`, preserving custom mounts instead of assuming `/mnt/c`.
pub fn runtime_to_host_path(
    distro: &str,
    runtime_path: &str,
) -> Result<HerdrWslWorkspaceLocation, String> {
    runtime_to_host_path_with(&SystemWslCommandExecutor, distro, runtime_path)
}

fn runtime_to_host_path_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    runtime_path: &str,
) -> Result<HerdrWslWorkspaceLocation, String> {
    validate_distro(distro)?;
    validate_runtime_path(runtime_path)?;
    let host_path = run_wsl_text_with(executor, distro, &["wslpath", "-w", "--", runtime_path])?;
    let host_path = normalize_wsl_host_path(distro, &host_path)?;
    Ok(HerdrWslWorkspaceLocation {
        distro: distro.to_string(),
        runtime_path: runtime_path.to_string(),
        display_path: host_path.clone(),
        host_path,
    })
}

/// Convert only a host path that demonstrably belongs to the selected WSL
/// distro. This fails closed for cross-distro UNC paths.
pub fn host_to_runtime_path(
    distro: &str,
    host_path: &str,
) -> Result<HerdrWslWorkspaceLocation, String> {
    host_to_runtime_path_with(&SystemWslCommandExecutor, distro, host_path)
}

fn host_to_runtime_path_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    host_path: &str,
) -> Result<HerdrWslWorkspaceLocation, String> {
    validate_distro(distro)?;
    validate_host_path_distro(distro, host_path)?;
    let runtime_path = run_wsl_text_with(executor, distro, &["wslpath", "-u", "--", host_path])?;
    validate_runtime_path(&runtime_path)?;
    Ok(HerdrWslWorkspaceLocation {
        distro: distro.to_string(),
        runtime_path,
        host_path: host_path.to_string(),
        display_path: host_path.to_string(),
    })
}

pub fn validate_distro(distro: &str) -> Result<(), String> {
    if distro != distro.trim() {
        return Err("WSL distribution name must not contain leading or trailing whitespace".into());
    }
    if distro.is_empty() || distro.len() > WSL_DISTRO_MAX_LEN || distro.contains('\0') {
        return Err("WSL distribution name is invalid".into());
    }
    Ok(())
}

fn require_windows() -> Result<(), String> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err("WSL Herdr runtime is available only on Windows hosts".into())
    }
}

fn stopped_session_message(session: &HerdrNamedSession) -> String {
    format!(
        "herdr session '{}' is not running in the selected WSL distribution",
        session.name
    )
}

/// Exact argv for the official Herdr terminal connector inside one selected
/// distro. The operation/target/size remain individual arguments; no caller
/// ever builds a shell command string.
pub(crate) fn wsl_terminal_connector_args(
    distro: &str,
    session_name: &str,
    mode: HerdrTerminalMode,
    target: &str,
    cols: u16,
    rows: u16,
    takeover: bool,
) -> Result<Vec<String>, String> {
    let operation = match mode {
        HerdrTerminalMode::Observe => "observe",
        HerdrTerminalMode::Control => "control",
    };
    let cols = cols.to_string();
    let rows = rows.to_string();
    let mut command = vec![
        "terminal", "session", operation, target, "--cols", &cols, "--rows", &rows,
    ];
    if takeover {
        command.push("--takeover");
    }
    wsl_exec_args(distro, Some(session_name), &command)
}

fn wsl_selected_distro_exec_args(
    distro: &str,
    command: Vec<String>,
) -> Result<Vec<String>, String> {
    validate_distro(distro)?;
    let mut args = vec![
        "--distribution".to_string(),
        distro.to_string(),
        "--exec".to_string(),
    ];
    args.extend(command);
    Ok(args)
}

/// Exact argv for a Herdr command inside a selected WSL distro. The launcher
/// is constant shell source; all dynamic values remain distinct argv or env
/// entries and are forwarded to the resolved Linux-native executable as `"$@"`.
pub(crate) fn wsl_exec_args(
    distro: &str,
    session_name: Option<&str>,
    command: &[&str],
) -> Result<Vec<String>, String> {
    let mut launcher = vec![
        "env".to_string(),
        "/bin/sh".to_string(),
        "-c".to_string(),
        WSL_HERDR_LAUNCHER.to_string(),
        WSL_HERDR_LAUNCHER_ARG0.to_string(),
    ];
    if let Some(session_name) = session_name.map(str::trim).filter(|name| !name.is_empty()) {
        if session_name.contains('\0') {
            return Err("Herdr session name is invalid".into());
        }
        launcher.insert(1, format!("HERDR_SESSION={session_name}"));
    }
    launcher.extend(command.iter().map(|value| (*value).to_string()));
    wsl_selected_distro_exec_args(distro, launcher)
}

fn run_herdr_capture_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    session_name: Option<&str>,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    executor
        .execute(&herdr_command_plan(distro, session_name, args, timeout)?)
        .map_err(WslCommandFailure::message)
}

fn run_wsl_text_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    args: &[&str],
) -> Result<String, String> {
    let output = executor
        .execute(&selected_distro_command_plan(
            distro,
            args,
            WSL_COMMAND_TIMEOUT,
        )?)
        .map_err(WslCommandFailure::message)?;
    let output =
        String::from_utf8(output).map_err(|_| "WSL command output is not UTF-8".to_string())?;
    let output = output.trim();
    if output.is_empty() {
        return Err("WSL path conversion returned an empty path".into());
    }
    Ok(output.to_string())
}

fn wait_for_wsl_command(mut command: Command, timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn wsl.exe failed: {error}"))?;
    let mut process_tree = process_kill::attach_process_tree(&mut child)
        .map_err(|error| format!("contain wsl.exe failed: {error}"))?;
    let (stdout, stderr, status) = wait_bounded_child(
        &mut child,
        &mut process_tree,
        timeout,
        MAX_NDJSON_LINE_BYTES,
    )?;
    let stderr = String::from_utf8(stderr).map_err(|_| "WSL stderr is not UTF-8".to_string())?;
    if !status.success() {
        return Err(classify_wsl_process_failure(
            status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }
    Ok(stdout)
}

/// Preserve a user-actionable failure class without parsing paths or trying a
/// different runtime. In particular, no WSL failure may fall back to Native.
fn classify_wsl_process_failure(exit_code: i32, stderr: &str) -> String {
    let lower = stderr.to_ascii_lowercase();
    let kind = if stderr.contains(WSL_HERDR_NOT_FOUND_DIAGNOSTIC) {
        "Herdr unavailable to non-interactive selected WSL runtime"
    } else if lower.contains("no installed distributions")
        || lower.contains("there is no distribution")
        || lower.contains("distribution with the supplied name")
    {
        "WSL distribution unavailable"
    } else if lower.contains("wsl")
        && (lower.contains("not found") || lower.contains("not recognized"))
    {
        "WSL executable unavailable"
    } else if lower.contains("herdr")
        && (lower.contains("not found") || lower.contains("command not found"))
    {
        "Herdr unavailable in selected WSL distribution"
    } else if lower.contains("session")
        && (lower.contains("not found") || lower.contains("not running"))
    {
        "Herdr session unavailable in selected WSL distribution"
    } else if lower.contains("protocol")
        || lower.contains("schema")
        || lower.contains("incompatible")
    {
        "Herdr protocol incompatible in selected WSL distribution"
    } else {
        "WSL command failed"
    };
    if stderr.is_empty() {
        format!("{kind} (wsl.exe exit {exit_code})")
    } else {
        format!("{kind} (wsl.exe exit {exit_code}): {stderr}")
    }
}

fn decode_wsl_list_output(bytes: &[u8]) -> Result<Vec<String>, String> {
    let looks_utf16le =
        bytes.starts_with(&[0xff, 0xfe]) || bytes.chunks_exact(2).take(32).any(|pair| pair[1] == 0);
    let text = if looks_utf16le {
        let start = usize::from(bytes.starts_with(&[0xff, 0xfe])) * 2;
        let units = bytes[start..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units).map_err(|_| "WSL distro list is invalid UTF-16".to_string())?
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| "WSL distro list is not UTF-8".to_string())?
    };
    let mut seen = std::collections::HashSet::new();
    let mut distros = Vec::new();
    for line in text.lines() {
        let distro = line.trim_matches(['\u{feff}', '\0', ' ', '\t', '\r']);
        if distro.is_empty() {
            continue;
        }
        validate_distro(distro)?;
        if !seen.insert(distro.to_string()) {
            return Err(format!(
                "WSL distro list contains duplicate entry: {distro}"
            ));
        }
        distros.push(distro.to_string());
    }
    Ok(distros)
}

/// Enrich only path-bearing records. Failed conversions are retained as runtime
/// paths rather than guessed host paths, so callers fail visibly instead of
/// sending a Linux path to Windows filesystem APIs.
fn enrich_snapshot_paths(distro: &str, snapshot: &mut serde_json::Value) {
    enrich_snapshot_paths_with(&SystemWslCommandExecutor, distro, snapshot)
}

fn enrich_snapshot_paths_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    snapshot: &mut serde_json::Value,
) {
    let mut resolved =
        std::collections::HashMap::<String, Option<HerdrWslWorkspaceLocation>>::new();
    for collection in ["workspaces", "agents", "panes"] {
        let Some(records) = snapshot
            .get_mut(collection)
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for record in records {
            let Some(item) = record.as_object_mut() else {
                continue;
            };
            let runtime_path = ["runtime_path", "cwd", "foreground_cwd", "path"]
                .iter()
                .find_map(|key| item.get(*key).and_then(serde_json::Value::as_str))
                .filter(|path| path.starts_with('/'))
                .map(str::to_string);
            let Some(runtime_path) = runtime_path else {
                continue;
            };
            item.insert(
                "runtime_path".into(),
                serde_json::Value::String(runtime_path.clone()),
            );
            let location = if let Some(existing) = resolved.get(&runtime_path) {
                existing.clone()
            } else if resolved.len() < MAX_SNAPSHOT_PATH_CONVERSIONS {
                let next = runtime_to_host_path_with(executor, distro, &runtime_path).ok();
                resolved.insert(runtime_path.clone(), next.clone());
                next
            } else {
                None
            };
            if let Some(location) = location {
                item.insert(
                    "host_path".into(),
                    serde_json::Value::String(location.host_path),
                );
                item.insert(
                    "display_path".into(),
                    serde_json::Value::String(location.display_path),
                );
            }
        }
    }
}

fn validate_runtime_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err("WSL runtime path must be an absolute Linux path".into());
    }
    Ok(())
}

fn validate_host_path_distro(distro: &str, path: &str) -> Result<(), String> {
    if path.contains('\0') {
        return Err("WSL host path is invalid".into());
    }
    let trimmed = path.trim_start_matches(['\\', '/']);
    let mut segments = trimmed
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty());
    let Some(server) = segments.next() else {
        return Err("WSL host path must be a \\wsl.localhost or \\wsl$ UNC path".into());
    };
    if !server.eq_ignore_ascii_case("wsl.localhost") && !server.eq_ignore_ascii_case("wsl$") {
        return Err("WSL host path must be a \\wsl.localhost or \\wsl$ UNC path".into());
    }
    let Some(path_distro) = segments.next() else {
        return Err("WSL host path is missing its distribution".into());
    };
    if !path_distro.eq_ignore_ascii_case(distro) {
        return Err(format!(
            "WSL host path distribution {path_distro:?} does not match selected distribution {distro:?}"
        ));
    }
    Ok(())
}

fn normalize_wsl_host_path(distro: &str, path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.starts_with("\\\\") || path.starts_with("//") {
        validate_host_path_distro(distro, path)?;
        return Ok(path.to_string());
    }
    // wslpath normally returns a drive path. Keep that operational spelling;
    // do not guess a `/mnt/c` inverse. Windows file APIs understand it.
    if path.len() >= 3 && path.as_bytes()[1] == b':' && matches!(path.as_bytes()[2], b'\\' | b'/') {
        return Ok(path.to_string());
    }
    Err("wslpath returned an unsupported host path".into())
}

fn unavailable_capabilities(reason: &str) -> HerdrCapabilities {
    HerdrCapabilities {
        binary_path: None,
        binary_version: None,
        binary_protocol: None,
        channel: None,
        binary_source: HerdrBinarySourceInfo {
            configured: HerdrBinarySource::Global,
            active: HerdrBinarySource::Global,
            resolved: None,
            available: false,
            path: None,
            reason: Some(reason.to_string()),
            version: None,
            protocol: None,
            configured_available: false,
            configured_path: None,
            configured_reason: Some(reason.to_string()),
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
            agent_get: false,
            agent_read: false,
            events_subscribe: false,
            worktree_list: false,
            methods: Vec::new(),
            schema_protocol: None,
            schema_version: None,
            reason: Some(reason.to_string()),
        },
        terminal: terminal_capability(false, reason),
        events: HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some(reason.to_string()),
        },
        transport: Some(HerdrTransportDiagnostics {
            mode: "wsl-cli-fallback".into(),
            state: "degraded".into(),
            generation: None,
            pending_requests: 0,
            event_listeners: 0,
            active_children: 0,
            requests: 0,
            responses: 0,
            events_delivered: 0,
            stale_events_dropped: 0,
            cold_start_ms: None,
            last_request_ms: None,
            max_request_ms: 0,
            last_event_dispatch_ms: None,
            max_event_dispatch_ms: 0,
            failure: Some(reason.to_string()),
        }),
    }
}

fn apply_proxy_method_flags(
    api: &mut HerdrApiCapability,
    methods: &std::collections::HashSet<String>,
) {
    let has = |method: &str| methods.contains(method);
    api.snapshot = has("session.snapshot");
    api.ping = has("ping") || has("session.ping");
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
    api.agent_get = has("agent.get");
    api.agent_read = has("agent.read");
    api.events_subscribe = has("events.subscribe");
    api.worktree_list = has("worktree.list");
}

fn terminal_capability(available: bool, reason: &str) -> HerdrTerminalCapability {
    HerdrTerminalCapability {
        observe: available,
        control: available,
        takeover: available,
        input: available,
        resize: available,
        scroll: available,
        release: available,
        create: false,
        reason: Some(reason.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[cfg(unix)]
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    use super::*;

    /// Host-runnable substitute for `wsl.exe`. It records the argv-only plan
    /// and yields queued stdout/failure outcomes without ever launching WSL.
    #[derive(Default)]
    struct FakeWslExecutor {
        plans: Mutex<Vec<WslCommandPlan>>,
        outcomes: Mutex<VecDeque<Result<Vec<u8>, WslCommandFailure>>>,
    }

    impl FakeWslExecutor {
        fn with_outcomes(outcomes: impl IntoIterator<Item = Result<Vec<u8>, String>>) -> Self {
            Self::with_detailed_outcomes(
                outcomes
                    .into_iter()
                    .map(|outcome| outcome.map_err(WslCommandFailure::Started)),
            )
        }

        fn with_detailed_outcomes(
            outcomes: impl IntoIterator<Item = Result<Vec<u8>, WslCommandFailure>>,
        ) -> Self {
            Self {
                plans: Mutex::new(Vec::new()),
                outcomes: Mutex::new(outcomes.into_iter().collect()),
            }
        }

        fn plans(&self) -> Vec<WslCommandPlan> {
            self.plans.lock().unwrap().clone()
        }
    }

    impl WslCommandExecutor for FakeWslExecutor {
        fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, WslCommandFailure> {
            self.plans.lock().unwrap().push(plan.clone());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    Err(WslCommandFailure::Started(
                        "fake wsl.exe received an unexpected launch".into(),
                    ))
                })
        }
    }

    #[test]
    fn decodes_utf16_distro_output_and_rejects_duplicates() {
        let bytes = "Ubuntu\r\nDebian\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_wsl_list_output(&bytes).unwrap(),
            vec!["Ubuntu".to_string(), "Debian".to_string()]
        );
        assert!(decode_wsl_list_output(b"Ubuntu\nUbuntu\n").is_err());
    }

    #[test]
    fn command_plan_keeps_distro_and_session_as_argv_values() {
        assert_eq!(
            wsl_exec_args("Ubuntu Dev", Some("default"), &["api", "snapshot"]).unwrap(),
            vec![
                "--distribution",
                "Ubuntu Dev",
                "--exec",
                "env",
                "HERDR_SESSION=default",
                "/bin/sh",
                "-c",
                WSL_HERDR_LAUNCHER,
                WSL_HERDR_LAUNCHER_ARG0,
                "api",
                "snapshot",
            ]
        );
    }

    #[test]
    fn launcher_source_is_constant_and_dynamic_values_remain_argv_or_env() {
        let distro = "Ubuntu Dev; touch /tmp/yuzora-wsl-distro";
        let session = "session; touch /tmp/yuzora-wsl-session";
        let target = "target; touch /tmp/yuzora-wsl-target";
        let command_substitution = "$(touch /tmp/yuzora-wsl-substitution)";
        let args = wsl_exec_args(
            distro,
            Some(session),
            &["terminal", target, command_substitution],
        )
        .unwrap();

        assert_eq!(args[1], distro);
        assert_eq!(args[4], format!("HERDR_SESSION={session}"));
        assert_eq!(args[5], "/bin/sh");
        assert_eq!(args[6], "-c");
        assert_eq!(args[7], WSL_HERDR_LAUNCHER);
        assert_eq!(args[8], WSL_HERDR_LAUNCHER_ARG0);
        assert_eq!(&args[9..], ["terminal", target, command_substitution]);
        assert!(!WSL_HERDR_LAUNCHER.contains(distro));
        assert!(!WSL_HERDR_LAUNCHER.contains(session));
        assert!(!WSL_HERDR_LAUNCHER.contains(target));
        assert!(!WSL_HERDR_LAUNCHER.contains(command_substitution));
    }

    #[cfg(unix)]
    fn write_executable(path: &std::path::Path, source: &str) {
        fs::write(path, source).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn write_launcher_tools(dir: &std::path::Path) {
        write_executable(
            &dir.join("readlink"),
            r#"#!/bin/sh
last=''
for value in "$@"; do last="$value"; done
if [ -n "${HERDR_TEST_TOOL_LOG:-}" ]; then
  printf 'readlink <%s>\n' "$last" >> "$HERDR_TEST_TOOL_LOG"
fi
printf '%s\n' "${HERDR_TEST_READLINK_TARGET:-$last}"
"#,
        );
        write_executable(
            &dir.join("od"),
            r#"#!/bin/sh
last=''
for value in "$@"; do last="$value"; done
if [ -n "${HERDR_TEST_TOOL_LOG:-}" ]; then
  printf 'od <%s>\n' "$last" >> "$HERDR_TEST_TOOL_LOG"
fi
case "$last" in
  *renamed-pe*) printf '%s\n' '4d 5a 90 00' ;;
  *script-wrapper*) printf '%s\n' '23 21 2f 62' ;;
  *) printf '%s\n' '7f 45 4c 46' ;;
esac
"#,
        );
    }

    #[cfg(unix)]
    fn launcher_command(home: &std::path::Path, tools: &std::path::Path) -> Command {
        let mut command = Command::new("/usr/bin/env");
        command
            .env_clear()
            .env("HOME", home)
            .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
            .args(["/bin/sh", "-c", WSL_HERDR_LAUNCHER, WSL_HERDR_LAUNCHER_ARG0]);
        command
    }

    #[cfg(unix)]
    #[test]
    fn noninteractive_launcher_finds_official_install_and_forwards_hostile_values_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let local_bin = home.join(".local/bin");
        let tools = dir.path().join("tools");
        let tool_log = dir.path().join("tool.log");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        write_launcher_tools(&tools);
        let herdr = local_bin.join("herdr");
        write_executable(
            &herdr,
            "#!/bin/sh\nprintf 'session=<%s>\\n' \"${HERDR_SESSION:-}\"\nfor value do printf 'arg=<%s>\\n' \"$value\"; done\n",
        );
        let marker = dir.path().join("shell-interpreted");
        let session = format!("session; touch {}", marker.display());
        let hostile = format!("target; touch {}", marker.display());
        let substitution = format!("$(touch {})", marker.display());

        let direct = Command::new("/usr/bin/env")
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin")
            .args(["herdr", "--version"])
            .output()
            .unwrap();
        assert_eq!(direct.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&direct.stderr).contains("herdr"));

        let output = launcher_command(&home, &tools)
            .env("HERDR_TEST_TOOL_LOG", &tool_log)
            .env("HERDR_SESSION", &session)
            .args(["--version", &hostile, &substitution])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(&format!("session=<{session}>")), "{stdout}");
        assert!(stdout.contains("arg=<--version>"), "{stdout}");
        assert!(stdout.contains(&format!("arg=<{hostile}>")), "{stdout}");
        assert!(
            stdout.contains(&format!("arg=<{substitution}>")),
            "{stdout}"
        );
        assert!(
            fs::read_to_string(&tool_log)
                .unwrap()
                .contains(&format!("readlink <{}>", herdr.display())),
            "the test must use its portable readlink fixture"
        );
        assert!(!marker.exists(), "launcher interpreted a dynamic value");
    }

    #[cfg(unix)]
    #[test]
    fn noninteractive_launcher_rejects_mounted_renamed_pe_and_script_wrappers() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let local_bin = home.join(".local/bin");
        let tools = dir.path().join("tools");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&tools).unwrap();
        write_launcher_tools(&tools);
        write_executable(&local_bin.join("herdr"), "#!/bin/sh\nexit 99\n");
        let renamed_pe = dir.path().join("renamed-pe");
        let script_wrapper = dir.path().join("script-wrapper");
        write_executable(&renamed_pe, "#!/bin/sh\nexit 98\n");
        write_executable(&script_wrapper, "#!/bin/sh\nexit 97\n");

        for (resolved, expected) in [
            (
                "/mnt/c/Herdr.exe".to_string(),
                "refusing Windows-interoperability Herdr executable: /mnt/c/Herdr.exe".to_string(),
            ),
            (
                renamed_pe.display().to_string(),
                format!(
                    "refusing non-Linux-ELF Herdr executable: {} (magic 4d5a9000)",
                    renamed_pe.display()
                ),
            ),
            (
                script_wrapper.display().to_string(),
                format!(
                    "refusing non-Linux-ELF Herdr executable: {} (magic 23212f62)",
                    script_wrapper.display()
                ),
            ),
        ] {
            let output = launcher_command(&home, &tools)
                .env("HERDR_TEST_READLINK_TARGET", &resolved)
                .arg("--version")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(127));
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.contains(&expected), "{stderr}");
            assert!(stderr.contains(WSL_HERDR_NOT_FOUND_DIAGNOSTIC), "{stderr}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn noninteractive_launcher_resolves_mise_to_the_actual_elf_not_its_shim() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let local_bin = home.join(".local/bin");
        let mise_shims = home.join(".local/share/mise/shims");
        let actual_dir = dir.path().join("actual");
        let tools = dir.path().join("tools");
        let shim_marker = dir.path().join("mise-shim-ran");
        fs::create_dir_all(&local_bin).unwrap();
        fs::create_dir_all(&mise_shims).unwrap();
        fs::create_dir_all(&actual_dir).unwrap();
        fs::create_dir_all(&tools).unwrap();
        write_launcher_tools(&tools);
        let actual_herdr = actual_dir.join("herdr");
        write_executable(
            &actual_herdr,
            "#!/bin/sh\nprintf 'actual=<%s>\\n' \"$0\"\nprintf 'session=<%s>\\n' \"${HERDR_SESSION:-}\"\nfor value do printf 'arg=<%s>\\n' \"$value\"; done\n",
        );
        write_executable(
            &local_bin.join("mise"),
            "#!/bin/sh\n[ \"${1:-}\" = which ] && [ \"${2:-}\" = herdr ] || exit 2\nprintf '%s\\n' \"$HERDR_MISE_TARGET\"\n",
        );
        write_executable(
            &mise_shims.join("herdr"),
            &format!("#!/bin/sh\ntouch '{}'\nexit 99\n", shim_marker.display()),
        );

        let output = launcher_command(&home, &tools)
            .env("HERDR_MISE_TARGET", &actual_herdr)
            .env("HERDR_MISE_SHIM_MARKER", &shim_marker)
            .env("HERDR_SESSION", "mise session")
            .args(["--version", "literal; no shell"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains(&format!("actual=<{}>", actual_herdr.display())),
            "{stdout}"
        );
        assert!(stdout.contains("session=<mise session>"), "{stdout}");
        assert!(stdout.contains("arg=<literal; no shell>"), "{stdout}");
        assert!(!shim_marker.exists(), "the generic mise shim was executed");
    }

    #[test]
    fn launcher_requires_a_linux_elf_and_preserves_wsl_interop_for_descendants() {
        assert!(WSL_HERDR_LAUNCHER.contains("od -An -N4 -tx1"));
        assert!(WSL_HERDR_LAUNCHER.contains("7f454c46"));
        assert!(WSL_HERDR_LAUNCHER.contains("\"$mise\" which herdr"));
        assert!(!WSL_HERDR_LAUNCHER.contains("mise/shims/herdr"));
        assert!(!WSL_HERDR_LAUNCHER.contains("unset WSL_INTEROP"));
    }

    #[test]
    fn cross_distro_unc_paths_fail_closed() {
        assert!(validate_host_path_distro("Ubuntu", r"\\wsl.localhost\Ubuntu\home\yuuzu").is_ok());
        assert!(validate_host_path_distro("Ubuntu", r"\\wsl$\Debian\home\yuuzu").is_err());
    }

    #[test]
    fn runtime_path_must_be_absolute() {
        assert!(validate_runtime_path("/home/yuuzu/project").is_ok());
        assert!(validate_runtime_path("relative/project").is_err());
    }

    #[test]
    fn distro_list_rejects_invalid_utf8_and_preserves_unicode_whitespace_names() {
        assert!(decode_wsl_list_output(&[0xff]).is_err());
        assert_eq!(
            decode_wsl_list_output(" Ubuntu Dev \n開発環境\n".as_bytes()).unwrap(),
            vec!["Ubuntu Dev", "開発環境"]
        );
        assert!(validate_distro(" \t ").is_err());
        assert!(validate_distro(" Ubuntu ").is_err());
        assert!(wsl_exec_args(" Ubuntu ", None, &["api", "snapshot"]).is_err());
        assert!(validate_distro("Ubuntu\0invalid").is_err());
    }

    #[test]
    fn path_model_accepts_custom_drive_mount_and_rejects_verbatim_cross_distro_unc() {
        assert_eq!(
            normalize_wsl_host_path("Ubuntu", r"D:\\workspace\\repo").unwrap(),
            r"D:\\workspace\\repo"
        );
        assert_eq!(
            normalize_wsl_host_path("Ubuntu", r"\\\\wsl.localhost\\Ubuntu\\home\\me").unwrap(),
            r"\\\\wsl.localhost\\Ubuntu\\home\\me"
        );
        assert!(
            normalize_wsl_host_path("Ubuntu", r"\\\\?\\UNC\\wsl.localhost\\Debian\\home").is_err()
        );
        assert!(
            validate_host_path_distro("Ubuntu", r"\\\\wsl.localhost\\ubuntu\\home\\me").is_ok()
        );
    }

    #[test]
    fn fake_wsl_list_is_host_only_and_preserves_utf16_unicode_and_whitespace() {
        let utf16 = "Ubuntu Dev\r\n開発環境\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let fake = FakeWslExecutor::with_outcomes([Ok(utf16)]);
        let distributions = list_distributions_with(&fake).unwrap();
        assert_eq!(
            distributions,
            vec![
                HerdrWslDistribution {
                    distro: "Ubuntu Dev".into()
                },
                HerdrWslDistribution {
                    distro: "開発環境".into()
                },
            ]
        );
        assert_eq!(
            fake.plans(),
            vec![list_distributions_plan(WSL_COMMAND_TIMEOUT)]
        );
        assert!(!fake.plans()[0].starts_selected_distro);
    }

    #[test]
    fn official_wsl_connector_plan_preserves_control_takeover_and_dimensions() {
        assert_eq!(
            wsl_terminal_connector_args(
                "Ubuntu Dev",
                "default",
                HerdrTerminalMode::Control,
                "terminal id with spaces",
                140,
                42,
                true,
            )
            .unwrap(),
            vec![
                "--distribution",
                "Ubuntu Dev",
                "--exec",
                "env",
                "HERDR_SESSION=default",
                "/bin/sh",
                "-c",
                WSL_HERDR_LAUNCHER,
                WSL_HERDR_LAUNCHER_ARG0,
                "terminal",
                "session",
                "control",
                "terminal id with spaces",
                "--cols",
                "140",
                "--rows",
                "42",
                "--takeover",
            ]
        );
        assert_eq!(
            wsl_terminal_connector_args(
                "Ubuntu",
                "default",
                HerdrTerminalMode::Observe,
                "term",
                80,
                24,
                false,
            )
            .unwrap()[9..],
            ["terminal", "session", "observe", "term", "--cols", "80", "--rows", "24"]
        );
    }

    #[test]
    fn fake_wsl_selected_herdr_launch_uses_only_argv_and_environment_argument() {
        let fake = FakeWslExecutor::with_outcomes([Ok(br#"{"sessions":[]}"#.to_vec())]);
        let _ = run_herdr_json_with(
            &fake,
            "Ubuntu Dev",
            Some("session with spaces"),
            &["session", "list", "--json"],
        );
        let plans = fake.plans();
        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert!(plan.starts_selected_distro);
        assert_eq!(
            plan.args,
            vec![
                "--distribution",
                "Ubuntu Dev",
                "--exec",
                "env",
                "HERDR_SESSION=session with spaces",
                "/bin/sh",
                "-c",
                WSL_HERDR_LAUNCHER,
                WSL_HERDR_LAUNCHER_ARG0,
                "session",
                "list",
                "--json",
            ]
        );
        assert_eq!(plan.stdout_limit, MAX_NDJSON_LINE_BYTES);
        assert_eq!(plan.stderr_limit, MAX_NDJSON_LINE_BYTES);
        assert_eq!(plan.timeout, WSL_COMMAND_TIMEOUT);
    }

    #[test]
    fn fake_wsl_path_conversion_is_distro_scoped_and_rejects_invalid_output() {
        let fake = FakeWslExecutor::with_outcomes([Ok(b"D:\\\\custom mount\\\\repo\n".to_vec())]);
        let location = runtime_to_host_path_with(&fake, "Ubuntu Dev", "/work/repo").unwrap();
        assert_eq!(location.host_path, r"D:\\custom mount\\repo");
        assert_eq!(
            fake.plans()[0].args,
            vec![
                "--distribution",
                "Ubuntu Dev",
                "--exec",
                "wslpath",
                "-w",
                "--",
                "/work/repo"
            ]
        );

        let invalid = FakeWslExecutor::with_outcomes([Ok(vec![0xff])]);
        assert!(runtime_to_host_path_with(&invalid, "Ubuntu", "/work/repo").is_err());

        let reverse = FakeWslExecutor::with_outcomes([Ok(b"/home/yuuzu/repo\n".to_vec())]);
        let location = host_to_runtime_path_with(
            &reverse,
            "Ubuntu",
            r"\\wsl.localhost\\Ubuntu\\home\\yuuzu\\repo",
        )
        .unwrap();
        assert_eq!(location.runtime_path, "/home/yuuzu/repo");
        assert_eq!(
            location.host_path,
            r"\\wsl.localhost\\Ubuntu\\home\\yuuzu\\repo"
        );
        assert_eq!(
            reverse.plans()[0].args,
            vec![
                "--distribution",
                "Ubuntu",
                "--exec",
                "wslpath",
                "-u",
                "--",
                r"\\wsl.localhost\\Ubuntu\\home\\yuuzu\\repo",
            ]
        );

        let cross_distro = FakeWslExecutor::default();
        assert!(host_to_runtime_path_with(
            &cross_distro,
            "Ubuntu",
            r"\\wsl.localhost\\Debian\\home\\repo"
        )
        .is_err());
        assert!(
            host_to_runtime_path_with(&cross_distro, "Ubuntu", r"C:\\wrong-distro\\repo").is_err()
        );
        assert!(cross_distro.plans().is_empty());
    }

    #[test]
    fn fake_wsl_snapshot_enrichment_is_deduplicated_and_bounded() {
        let fake = FakeWslExecutor::with_outcomes(
            (0..MAX_SNAPSHOT_PATH_CONVERSIONS)
                .map(|index| Ok(format!("D:\\workspace\\{index}\n").into_bytes())),
        );
        let mut snapshot = serde_json::json!({
            "workspaces": (0..(MAX_SNAPSHOT_PATH_CONVERSIONS + 3))
                .map(|index| serde_json::json!({ "cwd": format!("/workspace/{index}") }))
                .collect::<Vec<_>>(),
            "agents": [{ "cwd": "/workspace/0" }],
            "panes": []
        });
        enrich_snapshot_paths_with(&fake, "Ubuntu", &mut snapshot);
        assert_eq!(fake.plans().len(), MAX_SNAPSHOT_PATH_CONVERSIONS);
        assert_eq!(
            snapshot["workspaces"][0]["host_path"],
            serde_json::Value::String(r"D:\workspace\0".into())
        );
        assert!(snapshot["workspaces"][MAX_SNAPSHOT_PATH_CONVERSIONS]["host_path"].is_null());
        assert_eq!(
            snapshot["agents"][0]["host_path"],
            serde_json::Value::String(r"D:\workspace\0".into())
        );
    }

    #[test]
    fn wsl_failure_classes_are_explicit_and_never_imply_native_fallback() {
        for (stderr, expected) in [
            (
                "wsl.exe is not recognized as an internal or external command",
                "WSL executable unavailable",
            ),
            (
                "There are no installed distributions.",
                "WSL distribution unavailable",
            ),
            (
                "There is no distribution with the supplied name.",
                "WSL distribution unavailable",
            ),
            (
                WSL_HERDR_NOT_FOUND_DIAGNOSTIC,
                "Herdr unavailable to non-interactive selected WSL runtime",
            ),
            (
                "herdr: command not found",
                "Herdr unavailable in selected WSL distribution",
            ),
            (
                "session default not running",
                "Herdr session unavailable in selected WSL distribution",
            ),
            (
                "protocol incompatible",
                "Herdr protocol incompatible in selected WSL distribution",
            ),
        ] {
            let message = classify_wsl_process_failure(1, stderr);
            assert!(message.starts_with(expected), "{message}");
            assert!(!message.to_ascii_lowercase().contains("native fallback"));
        }
        let timeout = "WSL command timed out; Yuzora-owned child tree was terminated";
        let fake = FakeWslExecutor::with_outcomes([Err(timeout.into())]);
        let error = list_distributions_with(&fake).unwrap_err();
        assert!(error.contains("child tree was terminated"));
    }

    #[test]
    fn fake_wsl_rejects_duplicate_and_oversized_json_without_process_storms() {
        let duplicates = FakeWslExecutor::with_outcomes([Ok(b"Ubuntu\nUbuntu\n".to_vec())]);
        assert!(list_distributions_with(&duplicates).is_err());
        assert_eq!(duplicates.plans().len(), 1);

        let oversized = FakeWslExecutor::with_outcomes([Ok(vec![b'x'; MAX_NDJSON_LINE_BYTES + 1])]);
        assert!(run_herdr_json_with(&oversized, "Ubuntu", None, &["status", "--json"]).is_err());
        assert_eq!(oversized.plans().len(), 1);
    }

    #[test]
    fn official_v080_cli_map_is_closed_and_preserves_hostile_values_as_argv() {
        assert!(is_official_cli_v080("0.8.0"));
        assert!(!is_official_cli_v080("v0.8.0"));
        assert!(!is_official_cli_v080("0.8.9"));
        assert!(!is_official_cli_v080("0.8.0-rc.1"));
        assert!(!is_official_cli_v080("0.8.0+fork"));
        assert!(!is_official_cli_v080("0.9.0"));
        assert!(!is_official_cli_v080("0.8"));

        let hostile = "label; $(touch never)";
        let (workspace, mutation) = cli_v080_command_args(
            "workspace.create",
            &serde_json::json!({ "cwd": "/home/yuuzu/repo", "label": hostile, "focus": true }),
        )
        .unwrap();
        assert!(mutation);
        assert_eq!(
            workspace,
            vec![
                "workspace",
                "create",
                "--cwd",
                "/home/yuuzu/repo",
                "--label",
                hostile,
                "--focus"
            ]
        );

        let (split, mutation) = cli_v080_command_args(
            "pane.split",
            &serde_json::json!({ "target_pane_id": "w1:p1; $(oops)", "workspace_id": "w1", "direction": "right", "cwd": "/home/yuuzu/repo", "ratio": 0.333, "focus": false }),
        )
        .unwrap();
        assert!(mutation);
        assert_eq!(split[0..4], ["pane", "split", "--pane", "w1:p1; $(oops)"]);
        assert_eq!(
            split[4..],
            [
                "--direction",
                "right",
                "--ratio",
                "0.333",
                "--cwd",
                "/home/yuuzu/repo",
                "--no-focus"
            ]
        );

        let (swap, _) = cli_v080_command_args(
            "pane.swap",
            &serde_json::json!({ "source_pane_id": "w1:p1", "target_pane_id": "w1:p2" }),
        )
        .unwrap();
        assert_eq!(
            swap,
            vec![
                "pane",
                "swap",
                "--source-pane",
                "w1:p1",
                "--target-pane",
                "w1:p2"
            ]
        );
        assert!(
            cli_v080_command_args("pane.focus", &serde_json::json!({ "pane_id": "w1:p1" }))
                .is_err()
        );
        assert!(cli_v080_command_args(
            "tab.move",
            &serde_json::json!({ "tab_id": "w1:t1", "insert_index": 0 })
        )
        .is_err());
        assert!(cli_v080_command_args(
            "workspace.create",
            &serde_json::json!({ "cwd": "/home/x", "env": {} })
        )
        .is_err());
        assert!(cli_v080_command_args(
            "pane.rename",
            &serde_json::json!({ "pane_id": "w1:p1", "label": "--clear" })
        )
        .is_err());
        assert_eq!(
            cli_v080_command_args(
                "pane.rename",
                &serde_json::json!({ "pane_id": "w1:p1", "label": null })
            )
            .unwrap()
            .0,
            vec!["pane", "rename", "w1:p1", "--clear"]
        );
    }

    #[test]
    fn cli_capability_matrix_exposes_only_documented_command_map() {
        let mut api = unavailable_capabilities("test").api;
        apply_cli_v080_method_flags(&mut api);
        assert!(api.workspace_create && api.workspace_focus && api.tab_create && api.pane_split);
        assert!(api.pane_rename && api.pane_zoom && api.pane_swap && api.layout_export);
        assert!(api.worktree_list);
        assert!(
            !api.events_subscribe
                && !api.tab_move
                && !api.pane_focus
                && !api.layout_set_split_ratio
        );
        assert!(!api.methods.iter().any(|method| method == "pane.focus"));
    }

    #[test]
    fn terminal_connector_plan_fails_closed_for_read_only_runtime() {
        assert!(ensure_terminal_connector_plan(&WslControlPlan::Proxy).is_ok());
        assert!(
            ensure_terminal_connector_plan(&WslControlPlan::OfficialCliV080 {
                version: "0.8.0".into(),
                protocol: 20,
            })
            .is_ok()
        );
        let error = ensure_terminal_connector_plan(&WslControlPlan::ReadOnly {
            reason: "unverified dialect".into(),
        })
        .unwrap_err();
        assert!(error.contains("terminal connector unavailable"));
        assert!(error.contains("unverified dialect"));
    }

    fn running_wsl_session() -> HerdrNamedSession {
        HerdrNamedSession {
            name: "default".into(),
            default: true,
            running: true,
            session_dir: "/tmp/herdr/default".into(),
            socket_path: "/tmp/herdr/default.sock".into(),
        }
    }

    fn official_v080_status(
        client_version: &str,
        client_protocol: u32,
        server_version: &str,
        server_protocol: u32,
        compatible: bool,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "client": { "version": client_version, "protocol": client_protocol },
            "server": {
                "version": server_version,
                "protocol": server_protocol,
                "compatible": compatible,
            },
        }))
        .unwrap()
    }

    fn official_v080_schema(protocol: u32, schema_version: u32, methods: &[&str]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "protocol": protocol,
            "schema_version": schema_version,
            "methods": methods,
        }))
        .unwrap()
    }

    fn probe_cli_plan_for_test(
        status: Vec<u8>,
        schema: Vec<u8>,
    ) -> (WslControlPlan, HerdrCapabilities) {
        let fake = FakeWslExecutor::with_outcomes([Ok(status), Ok(schema)]);
        probe_control_plan_with(&fake, "Ubuntu", &running_wsl_session(), false)
    }

    #[test]
    fn writable_cli_plan_requires_the_exact_official_v080_dialect_and_complete_schema() {
        let complete_schema = official_v080_schema(20, 1, OFFICIAL_CLI_V080_METHODS);
        let (plan, caps) = probe_cli_plan_for_test(
            official_v080_status("0.8.0", 20, "0.8.0", 20, true),
            complete_schema.clone(),
        );
        assert!(matches!(
            plan,
            WslControlPlan::OfficialCliV080 {
                ref version,
                protocol: 20
            } if version == "0.8.0"
        ));
        assert!(verified_official_cli_v080(
            &caps,
            &collect_schema_methods(
                &serde_json::from_slice::<serde_json::Value>(&complete_schema).unwrap()
            )
        ));

        for (status, schema, reason) in [
            (
                official_v080_status("0.8.9", 20, "0.8.0", 20, true),
                complete_schema.clone(),
                "unreviewed patch",
            ),
            (
                official_v080_status("0.8.0-rc.1", 20, "0.8.0", 20, true),
                complete_schema.clone(),
                "prerelease",
            ),
            (
                official_v080_status("0.8.0", 19, "0.8.0", 20, true),
                complete_schema.clone(),
                "client protocol mismatch",
            ),
            (
                official_v080_status("0.8.0", 20, "0.8.0", 19, true),
                complete_schema.clone(),
                "server protocol mismatch",
            ),
            (
                official_v080_status("0.8.0", 20, "0.8.0", 20, false),
                complete_schema.clone(),
                "incompatible server",
            ),
            (
                official_v080_status("0.8.0", 20, "0.8.0", 20, true),
                official_v080_schema(19, 1, OFFICIAL_CLI_V080_METHODS),
                "schema protocol mismatch",
            ),
            (
                official_v080_status("0.8.0", 20, "0.8.0", 20, true),
                official_v080_schema(20, 2, OFFICIAL_CLI_V080_METHODS),
                "schema version mismatch",
            ),
            (
                official_v080_status("0.8.0", 20, "0.8.0", 20, true),
                official_v080_schema(20, 1, &["session.snapshot", "workspace.create"]),
                "incomplete schema",
            ),
        ] {
            let (plan, caps) = probe_cli_plan_for_test(status, schema);
            assert!(matches!(plan, WslControlPlan::ReadOnly { .. }), "{reason}");
            assert!(!caps.terminal.observe, "{reason}");
            assert!(!caps.terminal.control, "{reason}");
            assert!(!caps.terminal.takeover, "{reason}");
        }
    }

    #[test]
    fn control_plans_and_topology_queues_are_runtime_key_scoped() {
        let manager = HerdrManager::new();
        manager.set_wsl_control_plan(
            "Ubuntu",
            "default",
            WslControlPlan::OfficialCliV080 {
                version: "0.8.0".into(),
                protocol: 20,
            },
        );
        manager.set_wsl_control_plan(
            "Debian",
            "default",
            WslControlPlan::ReadOnly {
                reason: "unknown binary".into(),
            },
        );
        assert!(matches!(
            manager.wsl_control_plan("Ubuntu", "default"),
            Some(WslControlPlan::OfficialCliV080 { .. })
        ));
        assert!(matches!(
            manager.wsl_control_plan("Debian", "default"),
            Some(WslControlPlan::ReadOnly { .. })
        ));
        let ubuntu_lock = manager.wsl_cli_topology_lock("Ubuntu", "default").unwrap();
        let ubuntu_lock_again = manager.wsl_cli_topology_lock("Ubuntu", "default").unwrap();
        let debian_lock = manager.wsl_cli_topology_lock("Debian", "default").unwrap();
        assert!(std::sync::Arc::ptr_eq(&ubuntu_lock, &ubuntu_lock_again));
        assert!(!std::sync::Arc::ptr_eq(&ubuntu_lock, &debian_lock));
        manager.set_wsl_control_plan(
            " Ubuntu ",
            "default",
            WslControlPlan::ReadOnly {
                reason: "must not create a whitespace alias".into(),
            },
        );
        assert!(manager.wsl_control_plan(" Ubuntu ", "default").is_none());
        assert!(manager
            .wsl_cli_topology_lock(" Ubuntu ", "default")
            .is_none());
        assert!(manager
            .wsl_proxy_request(" Ubuntu ", "default", "ping", serde_json::json!({}))
            .is_err());
        assert!(matches!(
            manager.wsl_control_plan("Ubuntu", "default"),
            Some(WslControlPlan::OfficialCliV080 { .. })
        ));
    }

    fn raw_snapshot_response_with_paths(path_count: usize) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "result": {
                "type": "session_snapshot",
                "snapshot": {
                    "protocol": 20,
                    "version": "0.8.0",
                    "workspaces": (0..path_count)
                        .map(|index| serde_json::json!({ "path": format!("/work/{index}") }))
                        .collect::<Vec<_>>(),
                    "tabs": [],
                    "panes": [],
                    "agents": [],
                    "layouts": [],
                },
            },
        }))
        .unwrap()
    }

    fn successful_mutation_response() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "result": { "type": "workspace_created" },
        }))
        .unwrap()
    }

    #[test]
    fn cli_mutation_verifies_one_raw_snapshot_without_wslpath_fanout() {
        let fake = FakeWslExecutor::with_outcomes([
            Ok(successful_mutation_response()),
            Ok(raw_snapshot_response_with_paths(
                MAX_SNAPSHOT_PATH_CONVERSIONS,
            )),
        ]);
        let result = dispatch_official_cli_v080_with(
            &fake,
            "Ubuntu",
            "default",
            "workspace.create",
            serde_json::json!({ "cwd": "/work/0", "focus": true }),
            &HerdrManager::new(),
        )
        .unwrap();
        assert_eq!(result["result"]["type"], "workspace_created");
        let plans = fake.plans();
        assert_eq!(plans.len(), 2, "one mutation plus one raw reconciliation");
        assert!(plans[0].args.ends_with(&[
            "workspace".into(),
            "create".into(),
            "--cwd".into(),
            "/work/0".into(),
            "--focus".into()
        ]));
        assert!(plans[1].args.ends_with(&["api".into(), "snapshot".into()]));
        assert!(
            plans
                .iter()
                .flat_map(|plan| plan.args.iter())
                .all(|arg| arg != "wslpath"),
            "mutation verification must not perform optional path enrichment"
        );
    }

    #[test]
    fn cli_mutation_started_failures_are_reconciled_once_and_never_reported_retry_safe() {
        for (name, mutation_outcome) in [
            (
                "timeout",
                Err(WslCommandFailure::Started(
                    "WSL command timed out after side effect".into(),
                )),
            ),
            (
                "nonzero",
                Err(WslCommandFailure::Started(
                    "WSL command failed (wsl.exe exit 1) after side effect".into(),
                )),
            ),
            ("malformed", Ok(b"{not-json".to_vec())),
            ("oversized", Ok(vec![b'x'; MAX_NDJSON_LINE_BYTES + 1])),
            ("invalid utf8", Ok(vec![0xff])),
        ] {
            let fake = FakeWslExecutor::with_detailed_outcomes([
                mutation_outcome,
                Ok(raw_snapshot_response_with_paths(0)),
            ]);
            let error = dispatch_official_cli_v080_with(
                &fake,
                "Ubuntu",
                "default",
                "workspace.create",
                serde_json::json!({ "cwd": "/work/0", "focus": true }),
                &HerdrManager::new(),
            )
            .unwrap_err();
            assert!(
                error.contains("mutation outcome unknown"),
                "{name}: {error}"
            );
            assert!(
                error.contains("may have applied; do not retry blindly"),
                "{name}: {error}"
            );
            assert!(
                error.contains("raw snapshot reconciliation succeeded"),
                "{name}: {error}"
            );
            assert_eq!(fake.plans().len(), 2, "{name} must reconcile exactly once");
            assert!(fake.plans()[1]
                .args
                .ends_with(&["api".into(), "snapshot".into()]));
        }
    }

    #[test]
    fn cli_mutation_spawn_failure_is_definite_and_does_not_resnapshot() {
        let fake = FakeWslExecutor::with_detailed_outcomes([Err(WslCommandFailure::NotStarted(
            "spawn wsl.exe failed: missing executable".into(),
        ))]);
        let error = dispatch_official_cli_v080_with(
            &fake,
            "Ubuntu",
            "default",
            "workspace.create",
            serde_json::json!({ "cwd": "/work/0", "focus": true }),
            &HerdrManager::new(),
        )
        .unwrap_err();
        assert!(error.contains("spawn wsl.exe failed"));
        assert!(!error.contains("mutation outcome unknown"));
        assert_eq!(fake.plans().len(), 1);
    }

    fn rect(x: u32, y: u32, width: u32, height: u32) -> serde_json::Value {
        serde_json::json!({ "x": x, "y": y, "width": width, "height": height })
    }

    fn geometry_layout(
        panes: serde_json::Value,
        splits: serde_json::Value,
        focused: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "zoomed": true,
            "focused_pane_id": focused,
            "area": rect(0, 0, 100, 100),
            "panes": panes,
            "splits": splits,
        })
    }

    #[test]
    fn geometry_layout_converts_single_nested_and_repeated_axis_trees() {
        let single = geometry_layout(
            serde_json::json!([{ "pane_id": "w1:p1", "rect": rect(0, 0, 100, 100) }]),
            serde_json::json!([]),
            "w1:p1",
        );
        let single = layout_from_geometry(&single).unwrap();
        assert_eq!(
            single["root"],
            serde_json::json!({ "type": "pane", "pane_id": "w1:p1" })
        );
        assert_eq!(single["zoomed"], true);

        let nested = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 50, 100) },
                { "pane_id": "w1:p2", "rect": rect(50, 0, 50, 50) },
                { "pane_id": "w1:p3", "rect": rect(50, 50, 50, 50) }
            ]),
            serde_json::json!([
                { "id": "opaque-root", "direction": "right", "ratio": 0.5, "rect": rect(0, 0, 100, 100) },
                { "id": "opaque-child", "direction": "down", "ratio": 0.5, "rect": rect(50, 0, 50, 100) }
            ]),
            "w1:p3",
        );
        let nested = layout_from_geometry(&nested).unwrap();
        assert_eq!(nested["root"]["type"], "split");
        assert_eq!(nested["root"]["direction"], "right");
        assert_eq!(nested["root"]["second"]["direction"], "down");
        assert_eq!(nested["focused_pane_id"], "w1:p3");

        let repeated_axis = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 25, 100) },
                { "pane_id": "w1:p2", "rect": rect(25, 0, 37, 100) },
                { "pane_id": "w1:p3", "rect": rect(62, 0, 38, 100) }
            ]),
            serde_json::json!([
                { "id": "ignored", "direction": "right", "ratio": 0.25, "rect": rect(0, 0, 100, 100) },
                { "id": "still-ignored", "direction": "right", "ratio": 0.5, "rect": rect(25, 0, 75, 100) }
            ]),
            "w1:p2",
        );
        let repeated_axis = layout_from_geometry(&repeated_axis).unwrap();
        assert_eq!(repeated_axis["root"]["second"]["direction"], "right");

        // Exact official apply_pane_chrome geometry for gaps=true and
        // borders=false: only panes preceding a right/below neighbor shrink
        // by one cell; split rects retain the full layout regions.
        let pane_chrome = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 49, 100) },
                { "pane_id": "w1:p2", "rect": rect(50, 0, 50, 49) },
                { "pane_id": "w1:p3", "rect": rect(50, 50, 50, 50) }
            ]),
            serde_json::json!([
                { "id": "root-with-gaps", "direction": "right", "ratio": 0.5, "rect": rect(0, 0, 100, 100) },
                { "id": "child-with-gaps", "direction": "down", "ratio": 0.5, "rect": rect(50, 0, 50, 100) }
            ]),
            "w1:p2",
        );
        let pane_chrome = layout_from_geometry(&pane_chrome).unwrap();
        assert_eq!(pane_chrome["root"]["direction"], "right");
        assert_eq!(pane_chrome["root"]["second"]["direction"], "down");
    }

    #[test]
    fn geometry_layout_rejects_malformed_or_ambiguous_public_geometry() {
        let malformed_count = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 50, 100) },
                { "pane_id": "w1:p2", "rect": rect(50, 0, 50, 100) }
            ]),
            serde_json::json!([]),
            "w1:p1",
        );
        assert!(layout_from_geometry(&malformed_count).is_err());

        let ambiguous_root = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 50, 100) },
                { "pane_id": "w1:p2", "rect": rect(50, 0, 50, 100) }
            ]),
            serde_json::json!([
                { "id": "a", "direction": "right", "ratio": 0.5, "rect": rect(0, 0, 100, 100) },
                { "id": "b", "direction": "right", "ratio": 0.5, "rect": rect(0, 0, 100, 100) }
            ]),
            "w1:p1",
        );
        assert!(layout_from_geometry(&ambiguous_root).is_err());

        let boundary_ambiguous = geometry_layout(
            serde_json::json!([
                { "pane_id": "w1:p1", "rect": rect(0, 0, 100, 100) },
                { "pane_id": "w1:p2", "rect": rect(50, 0, 50, 100) }
            ]),
            serde_json::json!([{ "id": "root", "direction": "right", "ratio": 0.5, "rect": rect(0, 0, 100, 100) }]),
            "w1:p1",
        );
        assert!(layout_from_geometry(&boundary_ambiguous).is_err());
    }

    #[test]
    fn snapshot_layout_adaptation_synthesizes_existing_layout_export_shape() {
        let response = serde_json::json!({
            "result": {
                "type": "session_snapshot",
                "snapshot": {
                    "focused_tab_id": "w1:t1",
                    "layouts": [geometry_layout(
                        serde_json::json!([{ "pane_id": "w1:p1", "rect": rect(0, 0, 100, 100) }]),
                        serde_json::json!([]),
                        "w1:p1"
                    )]
                }
            }
        });
        let layout = layout_from_snapshot_response(&response, None, None)
            .unwrap()
            .unwrap();
        let adapted = synthetic_layout_export_response(layout);
        assert_eq!(adapted["result"]["type"], "layout_export");
        assert_eq!(adapted["result"]["layout"]["root"]["pane_id"], "w1:p1");
    }
}
