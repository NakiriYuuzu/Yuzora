use crate::lsp_config::{self, LspConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LspConfigCommand {
    Get,
    Stale,
    Detect {
        language: String,
        global: bool,
    },
    Set {
        language: String,
        server_id: String,
        global: bool,
    },
    ClearStale {
        path: String,
    },
}

impl LspConfigCommand {
    pub fn execute(self, workspace: &str) -> Result<serde_json::Value, String> {
        let path = lsp_config::config_path();
        let value = match self {
            Self::Get => serde_json::to_value(lsp_config::load_from(&path)),
            Self::Stale => {
                serde_json::to_value(lsp_config::stale_workspaces(&lsp_config::load_from(&path)))
            }
            Self::Detect { language, global } => {
                serde_json::to_value(crate::lsp_service::detect_host_server_scope(
                    (!global).then_some(workspace),
                    &language,
                )?)
            }
            Self::Set {
                language,
                server_id,
                global,
            } => serde_json::to_value(lsp_config::update(&path, |config| {
                set_profile(
                    config,
                    (!global).then_some(workspace),
                    &language,
                    &server_id,
                )
            })?),
            Self::ClearStale { path: stale } => {
                serde_json::to_value(lsp_config::update(&path, |config| {
                    if std::path::Path::new(&stale).exists() {
                        return Err("lsp-workspace-is-not-stale".into());
                    }
                    lsp_config::clear_workspace(config, &stale);
                    Ok(())
                })?)
            }
        };
        value.map_err(|e| e.to_string())
    }
}

pub fn set_profile(
    config: &mut LspConfig,
    workspace: Option<&str>,
    language: &str,
    server_id: &str,
) -> Result<(), String> {
    crate::lsp_adapters::adapter(language, server_id).ok_or("lsp-server-unsupported")?;
    lsp_config::set_server(config, workspace, language, server_id);
    Ok(())
}
