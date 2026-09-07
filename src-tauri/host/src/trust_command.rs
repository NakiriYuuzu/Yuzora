#[cfg(unix)]
use crate::files::WorkspaceFiles;
#[cfg(unix)]
use crate::workspace_trust::WorkspaceTrustState;
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum TrustCommand {
    Status { workspace: String },
    List,
    Challenge { workspace: String },
    ExecutionChallenge { workspace: String, command: String },
    Grant { challenge: String },
    Revoke { path: String },
}
#[cfg(unix)]
impl TrustCommand {
    pub fn execute(
        self,
        files: &WorkspaceFiles,
        trust: &WorkspaceTrustState,
    ) -> Result<Value, String> {
        fn value<T: Serialize>(result: Result<T, String>) -> Result<Value, String> {
            serde_json::to_value(result?).map_err(|e| e.to_string())
        }
        match self {
            Self::Status { workspace } => value(trust.0.status(files.canonical_root(&workspace)?)),
            Self::List => value(trust.0.list()),
            Self::Challenge { workspace } => value(
                trust
                    .0
                    .issue_workspace_challenge(files.canonical_root(&workspace)?),
            ),
            Self::ExecutionChallenge { workspace, command } => value(
                trust
                    .0
                    .issue_execution_challenge(files.canonical_root(&workspace)?, &command),
            ),
            Self::Grant { challenge } => value(trust.0.grant(&challenge)),
            Self::Revoke { path } => {
                trust.0.revoke(&path)?;
                value(trust.0.list())
            }
        }
    }
}

#[cfg(unix)]
pub fn host_trust(host_id: &str) -> Result<WorkspaceTrustState, String> {
    use sha2::Digest;
    let namespace = sha2::Sha256::digest(host_id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let home = dirs::home_dir().ok_or("host-trust-unavailable")?;
    Ok(WorkspaceTrustState::at(
        home.join(".yuzora/hosts")
            .join(namespace)
            .join("workspace-trust.json"),
    ))
}
