use serde::{Deserialize, Serialize};
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AdapterRegistry {
    agent_id: &'static str,
    registry_url: &'static str,
}

const ADAPTERS: [AdapterRegistry; 3] = [
    AdapterRegistry {
        agent_id: "pi",
        registry_url: "https://registry.npmjs.org/pi-acp/latest",
    },
    AdapterRegistry {
        agent_id: "claude",
        registry_url: "https://registry.npmjs.org/@agentclientprotocol%2Fclaude-agent-acp/latest",
    },
    AdapterRegistry {
        agent_id: "codex",
        registry_url: "https://registry.npmjs.org/@agentclientprotocol%2Fcodex-acp/latest",
    },
];

#[derive(Debug, Deserialize)]
struct RegistryLatest {
    version: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLatestVersion {
    agent_id: &'static str,
    version: String,
}

#[tauri::command]
pub async fn agent_latest_versions() -> Vec<AgentLatestVersion> {
    let checks = ADAPTERS
        .map(|adapter| tauri::async_runtime::spawn_blocking(move || fetch_latest_version(adapter)));
    let mut versions = Vec::with_capacity(ADAPTERS.len());
    for check in checks {
        if let Ok(Some(version)) = check.await {
            versions.push(version);
        }
    }
    versions
}

fn fetch_latest_version(adapter: AdapterRegistry) -> Option<AgentLatestVersion> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent.get(adapter.registry_url).call().ok()?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .ok()?;
    let version = parse_latest_version(&body)?;
    Some(AgentLatestVersion {
        agent_id: adapter.agent_id,
        version,
    })
}

fn parse_latest_version(body: &str) -> Option<String> {
    let version = serde_json::from_str::<RegistryLatest>(body)
        .ok()?
        .version
        .trim()
        .to_string();
    (!version.is_empty()).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_targets_are_fixed_and_complete() {
        assert_eq!(
            ADAPTERS.map(|adapter| adapter.agent_id),
            ["pi", "claude", "codex"]
        );
        assert!(ADAPTERS.iter().all(|adapter| {
            adapter
                .registry_url
                .starts_with("https://registry.npmjs.org/")
                && adapter.registry_url.ends_with("/latest")
        }));
    }

    #[test]
    fn latest_response_requires_a_non_empty_version() {
        assert_eq!(
            parse_latest_version(r#"{"version":" 0.0.32 "}"#),
            Some("0.0.32".to_string())
        );
        assert_eq!(parse_latest_version(r#"{"version":" "}"#), None);
        assert_eq!(parse_latest_version(r#"{"name":"pi-acp"}"#), None);
        assert_eq!(parse_latest_version("not-json"), None);
    }
}
