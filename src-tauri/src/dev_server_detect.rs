use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevServerCandidate {
    pub script_name: String,
    pub command: String,
    pub likely_port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevServerDetect {
    pub candidates: Vec<DevServerCandidate>,
    pub running_ports: Vec<u16>,
}

pub fn parse_package_scripts(package_json: &str) -> Vec<DevServerCandidate> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(package_json) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for name in ["dev", "start", "serve", "preview"] {
        let Some(command) = scripts.get(name).and_then(|v| v.as_str()) else {
            continue;
        };
        candidates.push(DevServerCandidate {
            script_name: name.to_string(),
            command: command.to_string(),
            likely_port: likely_port_for_command(command),
        });
    }
    candidates
}

pub fn common_dev_ports() -> &'static [u16] {
    &[3000, 5173, 8080, 4321, 5000, 8000, 4200, 4173, 5174, 9000]
}

pub fn probe_port(port: u16, timeout_ms: u64) -> bool {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms)).is_ok()
}

#[tauri::command]
pub async fn dev_server_detect(
    workspace: String,
    extra_ports: Option<Vec<u16>>,
) -> Result<DevServerDetect, String> {
    tauri::async_runtime::spawn_blocking(move || {
        detect_workspace(&workspace, extra_ports.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn detect_workspace(
    workspace: &str,
    extra_ports: Option<&[u16]>,
) -> Result<DevServerDetect, String> {
    let package_json = Path::new(workspace).join("package.json");
    let content = std::fs::read_to_string(package_json).unwrap_or_default();
    let candidates = parse_package_scripts(&content);

    let mut ports: BTreeSet<u16> = common_dev_ports().iter().copied().collect();
    ports.extend(
        candidates
            .iter()
            .filter_map(|candidate| candidate.likely_port),
    );
    if let Some(extra_ports) = extra_ports {
        ports.extend(extra_ports.iter().copied());
    }
    let running_ports = ports
        .into_iter()
        .filter(|port| probe_port(*port, 150))
        .collect();

    Ok(DevServerDetect {
        candidates,
        running_ports,
    })
}

fn likely_port_for_command(command: &str) -> Option<u16> {
    explicit_port(command).or_else(|| framework_port(command))
}

fn explicit_port(command: &str) -> Option<u16> {
    let mut previous_wants_port = false;
    for token in command.split_whitespace() {
        if previous_wants_port {
            if let Ok(port) = token.parse::<u16>() {
                return Some(port);
            }
            previous_wants_port = false;
        }

        if matches!(token, "--port" | "-p" | "--listen") {
            previous_wants_port = true;
            continue;
        }

        if let Some(value) = token
            .strip_prefix("--port=")
            .or_else(|| token.strip_prefix("-p="))
            .or_else(|| token.strip_prefix("PORT="))
        {
            if let Ok(port) = value.parse::<u16>() {
                return Some(port);
            }
        }
    }
    None
}

fn framework_port(command: &str) -> Option<u16> {
    let command = command.to_ascii_lowercase();
    if command.contains("vite") {
        Some(5173)
    } else if command.contains("astro") {
        Some(4321)
    } else if command.contains("next")
        || command.contains("nuxt")
        || command.contains("react-scripts")
        || command.contains("webpack")
    {
        Some(3000)
    } else if command.contains("ng serve") || command.starts_with("ng ") {
        Some(4200)
    } else if command.contains("serve") {
        Some(5000)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parse_package_scripts_detects_vite_dev_port() {
        let candidates = parse_package_scripts(
            r#"{"scripts":{"dev":"vite --host 0.0.0.0","build":"vite build"}}"#,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].script_name, "dev");
        assert_eq!(candidates[0].command, "vite --host 0.0.0.0");
        assert_eq!(candidates[0].likely_port, Some(5173));
    }

    #[test]
    fn parse_package_scripts_ignores_missing_scripts_and_bad_json() {
        assert!(parse_package_scripts(r#"{"name":"demo"}"#).is_empty());
        assert!(parse_package_scripts("{not json").is_empty());
    }

    #[test]
    fn common_dev_ports_contains_representatives() {
        let ports = common_dev_ports();
        for port in [3000, 5173, 8080, 4321, 5000, 8000] {
            assert!(ports.contains(&port), "missing common port {port}");
        }
    }

    #[test]
    fn probe_port_reports_open_and_closed_ports() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let open_port = listener.local_addr().unwrap().port();
        assert!(probe_port(open_port, 250));

        let closed_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let closed_port = closed_listener.local_addr().unwrap().port();
        drop(closed_listener);
        assert!(!probe_port(closed_port, 50));
    }

    #[test]
    fn detect_workspace_probes_extra_ports() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let open_port = listener.local_addr().unwrap().port();
        let detected =
            detect_workspace("/definitely/missing/workspace", Some(&[open_port])).unwrap();

        assert!(detected.running_ports.contains(&open_port));
    }
}
