//! Windows WSL runtime helpers.
//!
//! WSL is an execution boundary, not a path spelling. This module never opens a
//! WSL Unix socket from Windows. It only launches `wsl.exe` with an argv array,
//! and every selected-distro request runs `herdr` inside that distro.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use crate::herdr_limits::{parse_herdr_cli_stdout, MAX_NDJSON_LINE_BYTES};
use crate::herdr_service::{
    collect_schema_methods, parse_session_list_json, parse_snapshot_response, wait_bounded_child,
    HerdrApiCapability, HerdrBinarySource, HerdrBinarySourceInfo, HerdrCapabilities,
    HerdrEventsCapability, HerdrEventsStatus, HerdrManager, HerdrNamedSession,
    HerdrScrollDirection, HerdrServerCapability, HerdrSnapshotResult, HerdrTerminalCapability,
    HerdrTerminalMode, HerdrTerminalOpenResult, HerdrTransportDiagnostics, OnSubscriptionEvent,
    OnTerminalEvent,
};
use crate::process_kill;

use super::{HerdrRuntimeProvider, HerdrRuntimeTarget};

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

pub(crate) trait WslCommandExecutor {
    fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, String>;
}

struct SystemWslCommandExecutor;

impl WslCommandExecutor for SystemWslCommandExecutor {
    fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, String> {
        require_windows()?;
        let mut command = Command::new("wsl.exe");
        command
            .args(&plan.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process_kill::configure_hidden_process(&mut command);
        wait_for_wsl_command(command, plan.timeout).map_err(|error| {
            if error.starts_with("spawn wsl.exe failed") {
                format!("WSL executable unavailable: {error}")
            } else if error.contains("timed out") {
                format!("WSL command timed out; Yuzora-owned child tree was terminated: {error}")
            } else {
                error
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
        manager.wsl_proxy_request(distro, &session.name, method, params)
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
        manager.open_wsl_terminal(
            distro,
            target,
            mode,
            takeover,
            cols,
            rows,
            session_name,
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
        manager.wsl_events_subscribe(distro, session_name, on_event)
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
    let output = executor.execute(&list_distributions_plan(WSL_COMMAND_TIMEOUT))?;
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
/// yet installed a Herdr version with `api proxy --stdio`.
pub fn snapshot(distro: &str, session_name: Option<&str>) -> Result<HerdrSnapshotResult, String> {
    let session = resolve_named_session(distro, session_name)?;
    if !session.running {
        return Err(stopped_session_message(&session));
    }
    let response = run_herdr_json(distro, Some(&session.name), &["api", "snapshot"])?;
    let mut parsed = parse_snapshot_response(response)?;
    enrich_snapshot_paths(distro, &mut parsed.snapshot);
    Ok(parsed)
}

/// Read-only/fallback capability document. The CLI fallback intentionally only
/// advertises `session.snapshot` and official terminal connector support. Full
/// mutations/events require a successfully launched stdio proxy.
pub fn capabilities(
    distro: &str,
    session_name: Option<&str>,
    manager: &HerdrManager,
) -> HerdrCapabilities {
    let session = match resolve_named_session(distro, session_name) {
        Ok(session) => session,
        Err(error) => return unavailable_capabilities(&error),
    };
    let mut caps = unavailable_capabilities("Herdr in selected WSL distro is unavailable");
    caps.binary_path = Some(format!("wsl:{distro}:herdr"));
    caps.binary_source.available = true;
    caps.binary_source.path = caps.binary_path.clone();
    caps.binary_source.reason = None;
    caps.server.running = session.running;
    caps.server.socket_path = session.running.then_some(session.socket_path.clone());
    if !session.running {
        let reason = stopped_session_message(&session);
        caps.api.reason = Some(reason.clone());
        caps.terminal.reason = Some(reason);
        caps.transport = Some(manager.wsl_proxy_diagnostics(distro, &session.name));
        return caps;
    }

    let status = run_herdr_json(distro, Some(&session.name), &["status", "--json"]);
    if let Ok(status) = status {
        let client = status.get("client").unwrap_or(&status);
        caps.binary_version = client
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        caps.binary_protocol = client
            .get("protocol")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as u32);
        let server = status.get("server").unwrap_or(&status);
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

    let schema = run_herdr_json(distro, Some(&session.name), &["api", "schema", "--json"]);
    let Ok(schema) = schema else {
        caps.api.reason = Some("herdr api schema unavailable in selected WSL distro".into());
        caps.terminal = terminal_capability(
            true,
            "WSL CLI fallback: events and mutations require api proxy --stdio",
        );
        return caps;
    };
    let methods = collect_schema_methods(&schema);
    caps.api.methods = methods.iter().cloned().collect();
    caps.api.methods.sort();
    caps.api.schema_protocol = schema
        .get("protocol")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    caps.api.schema_version = schema
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value as u32);
    caps.api.snapshot = methods.contains("session.snapshot");
    caps.api.ping = methods.contains("ping") || methods.contains("session.ping");
    let proxy_available = manager
        .wsl_proxy_request(distro, &session.name, "ping", serde_json::json!({}))
        .is_ok();
    caps.transport = Some(manager.wsl_proxy_diagnostics(distro, &session.name));
    if proxy_available {
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
    } else {
        caps.api.reason = Some(
            "WSL CLI fallback active: mutations and events require herdr api proxy --stdio".into(),
        );
        caps.terminal = terminal_capability(
            true,
            "WSL official terminal connector is available; control plane requires api proxy --stdio",
        );
        caps.events = HerdrEventsCapability {
            status: HerdrEventsStatus::Unavailable,
            reason: Some("WSL CLI fallback does not support events.subscribe".into()),
        };
    }
    caps
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
    let trimmed = distro.trim();
    if trimmed.is_empty() || trimmed.len() > WSL_DISTRO_MAX_LEN || trimmed.contains('\0') {
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
        distro.trim().to_string(),
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
    executor.execute(&herdr_command_plan(distro, session_name, args, timeout)?)
}

fn run_wsl_text_with(
    executor: &impl WslCommandExecutor,
    distro: &str,
    args: &[&str],
) -> Result<String, String> {
    let output = executor.execute(&selected_distro_command_plan(
        distro,
        args,
        WSL_COMMAND_TIMEOUT,
    )?)?;
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
        outcomes: Mutex<VecDeque<Result<Vec<u8>, String>>>,
    }

    impl FakeWslExecutor {
        fn with_outcomes(outcomes: impl IntoIterator<Item = Result<Vec<u8>, String>>) -> Self {
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
        fn execute(&self, plan: &WslCommandPlan) -> Result<Vec<u8>, String> {
            self.plans.lock().unwrap().push(plan.clone());
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("fake wsl.exe received an unexpected launch".into()))
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
}
