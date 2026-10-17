//! Companion over Tailscale Serve: locate the CLI, read node and Serve state,
//! and plan the one HTTPS handler the desktop may add or remove.
//!
//! CLI behaviour is pinned to tailscale v1.104.1 (`ipn/ipnstate`, `ipn/serve.go`,
//! `cmd/tailscale/cli/serve_v2.go`):
//! - `serve --https` silently replaces another handler on the same port and
//!   ignores foreground sessions, so conflicts are refused here, before it runs.
//! - Without the node's `https` capability, `serve` may print an enablement URL
//!   and exit 0 without applying, or block. Apply must require the capability
//!   and re-read Serve state afterwards.
//! - `funnel status --json` prints the same ServeConfig as `serve status --json`.
//! - Services are scoped by service name and never share the node's ports.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const MAC_APP: &str = "/Applications/Tailscale.app";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Variant {
    /// Sandboxed; the CLI exists only inside the bundle.
    MacAppStore,
    MacStandalone,
    /// Any other install: open-source tailscaled, Windows, Linux packages.
    Cli,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cli {
    pub path: PathBuf,
    pub variant: Variant,
}

pub fn discover() -> Option<Cli> {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    discover_in(
        std::env::consts::OS,
        &dirs,
        program_files.as_deref(),
        Path::exists,
    )
}

fn discover_in(
    os: &str,
    dirs: &[PathBuf],
    program_files: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<Cli> {
    let exe = if os == "windows" {
        "tailscale.exe"
    } else {
        "tailscale"
    };
    let on_path = dirs.iter().map(|dir| dir.join(exe)).find(|p| exists(p));
    match os {
        "macos" => {
            let app = Path::new(MAC_APP);
            let bundled = app.join("Contents/MacOS/Tailscale");
            let variant = if exists(&app.join("Contents/_MASReceipt")) {
                Variant::MacAppStore
            } else if exists(&bundled) {
                Variant::MacStandalone
            } else {
                Variant::Cli
            };
            // Standalone's CLI integration and open-source installs live on PATH.
            let path = on_path.or_else(|| exists(&bundled).then_some(bundled))?;
            Some(Cli { path, variant })
        }
        "windows" => program_files
            .map(|dir| dir.join("Tailscale").join(exe))
            .filter(|p| exists(p))
            .or(on_path)
            .map(|path| Cli {
                path,
                variant: Variant::Cli,
            }),
        _ => on_path.map(|path| Cli {
            path,
            variant: Variant::Cli,
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    #[serde(default)]
    version: String,
    #[serde(default)]
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<SelfNode>,
    cert_domains: Option<Vec<String>>,
    current_tailnet: Option<Tailnet>,
    user: Option<HashMap<String, UserProfile>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SelfNode {
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "UserID", default)]
    user_id: i64,
    tags: Option<Vec<String>>,
    cap_map: Option<HashMap<String, Value>>,
}
#[derive(Deserialize)]
struct Tailnet {
    #[serde(rename = "MagicDNSEnabled", default)]
    magic_dns_enabled: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UserProfile {
    #[serde(default)]
    login_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub version: String,
    pub backend_state: String,
    /// Without the trailing dot, as Serve keys and certificates use it.
    pub dns_name: Option<String>,
    pub login: Option<String>,
    /// Serve injects no identity headers for tagged nodes.
    pub tagged: bool,
    pub magic_dns: bool,
    /// The `https` node capability the CLI checks, plus a certificate domain.
    pub https_ready: bool,
}

/// Parses `tailscale status --json`. Keep peers: before v1.104 the self
/// user's profile is only present when peers are included.
pub fn parse_status(json: &str) -> Result<Node, String> {
    let status: Status =
        serde_json::from_str(json).map_err(|e| format!("tailscale-invalid-status: {e}"))?;
    let self_node = status.self_node.as_ref();
    let dns_name = self_node
        .map(|node| node.dns_name.trim_end_matches('.'))
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let login = self_node
        .and_then(|node| status.user.as_ref()?.get(&node.user_id.to_string()))
        .map(|user| user.login_name.clone())
        .filter(|login| !login.is_empty());
    let has_https = self_node
        .and_then(|node| node.cap_map.as_ref())
        .is_some_and(|caps| caps.contains_key("https"));
    let certified = dns_name.as_ref().is_some_and(|name| {
        status
            .cert_domains
            .iter()
            .flatten()
            .any(|domain| domain.eq_ignore_ascii_case(name))
    });
    Ok(Node {
        version: status.version,
        backend_state: status.backend_state,
        dns_name,
        login,
        tagged: self_node.is_some_and(|node| node.tags.as_ref().is_some_and(|t| !t.is_empty())),
        magic_dns: status
            .current_tailnet
            .is_some_and(|tailnet| tailnet.magic_dns_enabled),
        https_ready: has_https && certified,
    })
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct ServeConfig {
    #[serde(rename = "TCP")]
    tcp: HashMap<String, Value>,
    web: HashMap<String, Value>,
    allow_funnel: HashMap<String, bool>,
    foreground: HashMap<String, ServeConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PortState {
    Free,
    /// Exactly the handler `serve_args` creates for this gateway port.
    Ours,
    /// Our `/` proxy, plus mounts someone added on the same origin.
    Shared,
    Taken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Serve {
    /// The background config: the only one `serve --bg` writes and `off` edits.
    pub port: PortState,
    /// A running foreground `tailscale serve` overrides this port for now.
    pub foreground: bool,
    /// Some handler on this node is public on the internet.
    pub funnel: bool,
    /// Other handlers on this hostname can read the Companion's cookies.
    pub other_ports: Vec<u16>,
}

/// Parses `tailscale serve status --json`, which prints `null` without a config.
pub fn check_serve(
    json: &str,
    host: &str,
    https_port: u16,
    gateway_port: u16,
) -> Result<Serve, String> {
    let config = serde_json::from_str::<Option<ServeConfig>>(json)
        .map_err(|e| format!("tailscale-invalid-serve: {e}"))?
        .unwrap_or_default();
    let port_key = https_port.to_string();
    let web_key = format!("{host}:{https_port}");
    let all: Vec<&ServeConfig> = std::iter::once(&config)
        .chain(config.foreground.values())
        .collect();
    // A foreground handler belongs to a running `tailscale serve`, never to us.
    // It hides the background one only until it exits, so both are reported.
    let foreground = config
        .foreground
        .values()
        .any(|fg| fg.tcp.contains_key(&port_key) || fg.web.contains_key(&web_key));
    let ours_root = json!({"Proxy": proxy_target(gateway_port)});
    let port = match (config.tcp.get(&port_key), config.web.get(&web_key)) {
        (None, None) => PortState::Free,
        (Some(tcp), Some(web))
            if *tcp == json!({"HTTPS": true})
                && web.as_object().is_some_and(|web| web.len() == 1)
                && web["Handlers"]["/"] == ours_root =>
        {
            if web["Handlers"].as_object().is_some_and(|h| h.len() == 1) {
                PortState::Ours
            } else {
                PortState::Shared
            }
        }
        _ => PortState::Taken,
    };
    let mut other_ports: Vec<u16> = all
        .iter()
        .flat_map(|sc| sc.tcp.keys())
        .filter_map(|key| key.parse().ok())
        .filter(|port| *port != https_port)
        .collect();
    other_ports.sort_unstable();
    other_ports.dedup();
    Ok(Serve {
        port,
        foreground,
        funnel: all.iter().any(|sc| sc.allow_funnel.values().any(|on| *on)),
        other_ports,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Refusal {
    NotRunning,
    NoLogin,
    Tagged,
    HttpsNotReady,
    Funnel,
    PortTaken,
}

/// Why the Companion path cannot be enabled. Re-check right before applying.
pub fn refusal(node: &Node, serve: &Serve) -> Option<Refusal> {
    if node.backend_state != "Running" {
        Some(Refusal::NotRunning)
    } else if node.tagged {
        Some(Refusal::Tagged)
    } else if node.login.is_none() || node.dns_name.is_none() {
        Some(Refusal::NoLogin)
    } else if !node.https_ready {
        Some(Refusal::HttpsNotReady)
    } else if serve.funnel {
        Some(Refusal::Funnel)
    } else if serve.foreground || matches!(serve.port, PortState::Taken | PortState::Shared) {
        // Another mount on the same origin can read the Companion's cookies.
        Some(Refusal::PortTaken)
    } else {
        None
    }
}

/// The gateway binds IPv4 loopback only; `localhost` may resolve to `::1`.
fn proxy_target(gateway_port: u16) -> String {
    format!("http://127.0.0.1:{gateway_port}")
}

pub fn serve_args(https_port: u16, gateway_port: u16) -> Vec<String> {
    vec![
        "serve".into(),
        "--bg".into(),
        format!("--https={https_port}"),
        proxy_target(gateway_port),
    ]
}

/// Removes only our `/` mount. Mounts someone added on the port stay, and so
/// does the port while it has any; without `--set-path` the CLI would remove
/// every mount (after a prompt).
pub fn off_args(serve: &Serve, https_port: u16) -> Option<Vec<String>> {
    matches!(serve.port, PortState::Ours | PortState::Shared).then(|| {
        vec![
            "serve".into(),
            format!("--https={https_port}"),
            "--set-path=/".into(),
            "off".into(),
        ]
    })
}

pub async fn run(cli: &Path, args: &[String]) -> Result<String, String> {
    run_within(cli, args, COMMAND_TIMEOUT).await
}

async fn run_within(cli: &Path, args: &[String], limit: Duration) -> Result<String, String> {
    let mut command = tokio::process::Command::new(cli);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = tokio::time::timeout(limit, command.output())
        .await
        .map_err(|_| "tailscale-timeout".to_string())?
        .map_err(|e| format!("tailscale-unavailable: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "tailscale-failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "tailscale-invalid-output".to_string())
}

#[cfg(test)]
mod tests;
