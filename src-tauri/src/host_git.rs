//! Git gets a dedicated channel per workspace; control RPC stays responsive.
use crate::host_service::{open_stream, HostConnection};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use yuzora_host::git_command::GitCommand;
use yuzora_host::protocol::Operation;
use yuzora_host::workspace_trust::WorkspaceIdentity;

pub(crate) type GitChannels = Mutex<HashMap<String, Arc<GitLane>>>;

pub(crate) struct GitLane {
    cancelled: tokio::sync::watch::Sender<bool>,
    state: tokio::sync::Mutex<Option<GitChannel>>,
}
struct GitChannel {
    connection: HostConnection,
    workspace: String,
}
impl Default for GitLane {
    fn default() -> Self {
        Self {
            cancelled: tokio::sync::watch::channel(false).0,
            state: Default::default(),
        }
    }
}
impl GitLane {
    pub(crate) fn cancel(&self) {
        self.cancelled.send_replace(true);
    }
}
impl Drop for GitLane {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl HostConnection {
    pub(crate) fn close_git(&self, workspace: &str) {
        if let Some(lane) = self.git_channels.lock().unwrap().remove(workspace) {
            lane.cancel();
        }
    }
    pub(crate) fn close_all_git(&self) {
        for (_, lane) in self.git_channels.lock().unwrap().drain() {
            lane.cancel();
        }
    }

    pub(crate) async fn git_request(
        &self,
        ssh: &crate::ssh_service::SshManager,
        workspace: String,
        repository_root: Option<String>,
        call: GitCommand,
    ) -> Result<serde_json::Value, String> {
        let _permit = self.git_jobs.try_acquire().map_err(|_| "host-git-limit")?;
        let lane = {
            let mut lanes = self.git_channels.lock().unwrap();
            if !lanes.contains_key(&workspace) && lanes.len() >= 128 {
                return Err("host-git-workspace-limit".into());
            }
            lanes
                .entry(workspace.clone())
                .or_insert_with(|| Arc::new(GitLane::default()))
                .clone()
        };
        // The primary helper owns the caller's capability and trust challenges.
        let identity: WorkspaceIdentity = serde_json::from_value(
            self.request(Operation::WorkspaceAuthorize {
                workspace: workspace.clone(),
            })
            .await?,
        )
        .map_err(|e| e.to_string())?;
        let mut disconnected = self.cancelled.subscribe();
        let mut closed = lane.cancelled.subscribe();
        if *disconnected.borrow() || *closed.borrow() {
            return Err("host-disconnected".into());
        }
        let run = async {
            let mut slot = lane.state.lock().await;
            let discovering = matches!(call, GitCommand::Detect | GitCommand::Bootstrap);
            if slot.is_none() {
                if !discovering && !call.is_read() {
                    return Err("git-refresh-required".into());
                }
                let stream = open_stream(
                    &self.target,
                    &self.helper,
                    crate::host_service::HostLane::Control,
                    ssh,
                )
                .await?;
                let mut connection = HostConnection::new(
                    self.owner.clone(),
                    self.target.clone(),
                    self.helper.clone(),
                    stream,
                );
                connection.cancelled = lane.cancelled.clone();
                connection.request(Operation::Hello).await?;
                let opened = connection
                    .request(Operation::WorkspaceOpen {
                        path: identity.canonical_path.clone(),
                    })
                    .await?;
                let capability = opened["capabilityId"]
                    .as_str()
                    .ok_or("invalid-workspace-capability")?
                    .to_owned();
                let observed: WorkspaceIdentity = serde_json::from_value(
                    connection
                        .request(Operation::WorkspaceAuthorize {
                            workspace: capability.clone(),
                        })
                        .await?,
                )
                .map_err(|e| e.to_string())?;
                if observed != identity {
                    return Err("workspace-identity-changed".into());
                }
                if !discovering {
                    let detected = connection
                        .request(Operation::Git {
                            workspace: capability.clone(),
                            repository_root: None,
                            call: GitCommand::Detect,
                        })
                        .await?;
                    if detected["root"].as_str() != repository_root.as_deref()
                        || detected["status"] != "ready"
                    {
                        return Err("git-repository-identity-mismatch".into());
                    }
                }
                *slot = Some(GitChannel {
                    connection,
                    workspace: capability,
                });
            }
            let channel = slot.as_ref().ok_or("git-channel-unavailable")?;
            let result = channel
                .connection
                .request_with_timeout(
                    Operation::Git {
                        workspace: channel.workspace.clone(),
                        repository_root,
                        call,
                    },
                    Duration::from_secs(140),
                )
                .await;
            if channel.connection.io.lock().await.is_none() {
                *slot = None;
            }
            result
        };
        let result = tokio::select! {
            _ = disconnected.changed() => Err("host-disconnected".into()),
            _ = closed.changed() => Err("workspace-closed".into()),
            result = tokio::time::timeout(Duration::from_secs(150), run) => result.map_err(|_| "git-request-timeout".to_string())?,
        };
        if let Err(error) = &result {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(error) {
                if value.get("challengeId").is_some() {
                    // A revocation/replacement can race authorization. Reissue on
                    // the primary helper so the user can grant its live challenge.
                    self.request(Operation::WorkspaceAuthorize { workspace })
                        .await?;
                }
            }
        }
        result
    }
}
