//! Typed remote counterparts of desktop HERDR commands. Both call the same facade.
use crate::herdr_service::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "command",
    content = "args",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HerdrCommand {
    #[serde(rename = "herdr_sessions")]
    Sessions,
    #[serde(rename = "herdr_capabilities")]
    Capabilities { session_name: Option<String> },
    #[serde(rename = "herdr_snapshot")]
    Snapshot { session_name: Option<String> },
    #[serde(rename = "herdr_terminal_create")]
    TerminalCreate {
        session_name: Option<String>,
        workspace_id: Option<String>,
        title: Option<String>,
    },
    #[serde(rename = "herdr_workspace_focus")]
    WorkspaceFocus {
        session_name: Option<String>,
        workspace_id: String,
    },
    #[serde(rename = "herdr_workspace_create")]
    WorkspaceCreate {
        session_name: Option<String>,
        cwd: Option<String>,
        label: Option<String>,
        focus: Option<bool>,
    },
    #[serde(rename = "herdr_workspace_rename")]
    WorkspaceRename {
        session_name: Option<String>,
        workspace_id: String,
        label: String,
    },
    #[serde(rename = "herdr_workspace_close")]
    WorkspaceClose {
        session_name: Option<String>,
        workspace_id: String,
    },
    #[serde(rename = "herdr_worktree_list")]
    WorktreeList {
        session_name: Option<String>,
        cwd: Option<String>,
        workspace_id: Option<String>,
    },
    #[serde(rename = "herdr_tab_create")]
    TabCreate {
        session_name: Option<String>,
        workspace_id: Option<String>,
        label: Option<String>,
        cwd: Option<String>,
        focus: Option<bool>,
    },
    #[serde(rename = "herdr_tab_focus")]
    TabFocus {
        session_name: Option<String>,
        tab_id: String,
    },
    #[serde(rename = "herdr_tab_rename")]
    TabRename {
        session_name: Option<String>,
        tab_id: String,
        label: String,
    },
    #[serde(rename = "herdr_tab_close")]
    TabClose {
        session_name: Option<String>,
        tab_id: String,
    },
    #[serde(rename = "herdr_tab_move")]
    TabMove {
        session_name: Option<String>,
        tab_id: String,
        insert_index: u32,
    },
    #[serde(rename = "herdr_pane_scroll_state")]
    PaneScrollState {
        session_name: Option<String>,
        pane_id: String,
    },
    #[serde(rename = "herdr_pane_scroll_to")]
    PaneScrollTo {
        session_name: Option<String>,
        pane_id: String,
        offset_from_bottom: u64,
    },
    #[serde(rename = "herdr_pane_focus")]
    PaneFocus {
        session_name: Option<String>,
        pane_id: String,
    },
    #[serde(rename = "herdr_pane_rename")]
    PaneRename {
        session_name: Option<String>,
        pane_id: String,
        label: Option<String>,
    },
    #[serde(rename = "herdr_pane_split")]
    PaneSplit {
        session_name: Option<String>,
        direction: HerdrSplitDirection,
        target_pane_id: Option<String>,
        workspace_id: Option<String>,
        cwd: Option<String>,
        ratio: Option<f64>,
        focus: Option<bool>,
    },
    #[serde(rename = "herdr_pane_zoom")]
    PaneZoom {
        session_name: Option<String>,
        pane_id: Option<String>,
        mode: Option<HerdrPaneZoomMode>,
    },
    #[serde(rename = "herdr_pane_swap")]
    PaneSwap {
        session_name: Option<String>,
        source_pane_id: Option<String>,
        target_pane_id: Option<String>,
        pane_id: Option<String>,
        direction: Option<String>,
    },
    #[serde(rename = "herdr_pane_close")]
    PaneClose {
        session_name: Option<String>,
        pane_id: String,
    },
    #[serde(rename = "herdr_layout_export")]
    LayoutExport {
        session_name: Option<String>,
        tab_id: Option<String>,
        pane_id: Option<String>,
    },
    #[serde(rename = "herdr_layout_set_split_ratio")]
    LayoutSetSplitRatio {
        session_name: Option<String>,
        tab_id: Option<String>,
        pane_id: Option<String>,
        path: Vec<bool>,
        ratio: f64,
    },
    #[serde(rename = "herdr_agent_get")]
    AgentGet {
        session_name: Option<String>,
        target: String,
    },
    #[serde(rename = "herdr_agent_read")]
    AgentRead {
        session_name: Option<String>,
        target: String,
        source: HerdrReadSource,
        format: Option<HerdrReadFormat>,
        lines: Option<u32>,
        strip_ansi: Option<bool>,
    },
}

impl HerdrCommand {
    pub fn execute(self, manager: &Arc<HerdrManager>) -> Result<serde_json::Value, String> {
        match self {
            Self::Sessions => {
                serde_json::to_value(manager.list_sessions()?).map_err(|e| e.to_string())
            }
            Self::Capabilities { session_name } => {
                serde_json::to_value(manager.capabilities_for_session(session_name.as_deref()))
                    .map_err(|e| e.to_string())
            }
            Self::Snapshot { session_name } => {
                serde_json::to_value(manager.snapshot(session_name.as_deref())?)
                    .map_err(|e| e.to_string())
            }
            Self::TerminalCreate {
                session_name,
                workspace_id,
                title,
            } => serde_json::to_value(manager.create_terminal(
                session_name.as_deref(),
                workspace_id,
                title,
            )?)
            .map_err(|e| e.to_string()),
            Self::WorkspaceFocus {
                session_name,
                workspace_id,
            } => serde_json::to_value(
                manager.workspace_focus(session_name.as_deref(), workspace_id)?,
            )
            .map_err(|e| e.to_string()),
            Self::WorkspaceCreate {
                session_name,
                cwd,
                label,
                focus,
            } => serde_json::to_value(manager.workspace_create(
                session_name.as_deref(),
                cwd,
                label,
                focus.unwrap_or(true),
            )?)
            .map_err(|e| e.to_string()),
            Self::WorkspaceRename {
                session_name,
                workspace_id,
                label,
            } => serde_json::to_value(manager.workspace_rename(
                session_name.as_deref(),
                workspace_id,
                label,
            )?)
            .map_err(|e| e.to_string()),
            Self::WorkspaceClose {
                session_name,
                workspace_id,
            } => serde_json::to_value(
                manager.workspace_close(session_name.as_deref(), workspace_id)?,
            )
            .map_err(|e| e.to_string()),
            Self::WorktreeList {
                session_name,
                cwd,
                workspace_id,
            } => serde_json::to_value(manager.worktree_list(
                session_name.as_deref(),
                cwd,
                workspace_id,
            )?)
            .map_err(|e| e.to_string()),
            Self::TabCreate {
                session_name,
                workspace_id,
                label,
                cwd,
                focus,
            } => serde_json::to_value(manager.tab_create(
                session_name.as_deref(),
                workspace_id,
                label,
                cwd,
                focus.unwrap_or(true),
            )?)
            .map_err(|e| e.to_string()),
            Self::TabFocus {
                session_name,
                tab_id,
            } => serde_json::to_value(manager.tab_focus(session_name.as_deref(), tab_id)?)
                .map_err(|e| e.to_string()),
            Self::TabRename {
                session_name,
                tab_id,
                label,
            } => {
                serde_json::to_value(manager.tab_rename(session_name.as_deref(), tab_id, label)?)
                    .map_err(|e| e.to_string())
            }
            Self::TabClose {
                session_name,
                tab_id,
            } => serde_json::to_value(manager.tab_close(session_name.as_deref(), tab_id)?)
                .map_err(|e| e.to_string()),
            Self::TabMove {
                session_name,
                tab_id,
                insert_index,
            } => serde_json::to_value(manager.tab_move(
                session_name.as_deref(),
                tab_id,
                insert_index,
            )?)
            .map_err(|e| e.to_string()),
            Self::PaneScrollState {
                session_name,
                pane_id,
            } => serde_json::to_value(manager.pane_scroll_state(session_name.as_deref(), pane_id)?)
                .map_err(|e| e.to_string()),
            Self::PaneScrollTo {
                session_name,
                pane_id,
                offset_from_bottom,
            } => serde_json::to_value(manager.pane_scroll_to(
                session_name.as_deref(),
                pane_id,
                offset_from_bottom,
            )?)
            .map_err(|e| e.to_string()),
            Self::PaneFocus {
                session_name,
                pane_id,
            } => serde_json::to_value(manager.pane_focus(session_name.as_deref(), pane_id)?)
                .map_err(|e| e.to_string()),
            Self::PaneRename {
                session_name,
                pane_id,
                label,
            } => serde_json::to_value(manager.pane_rename(
                session_name.as_deref(),
                pane_id,
                label,
            )?)
            .map_err(|e| e.to_string()),
            Self::PaneSplit {
                session_name,
                direction,
                target_pane_id,
                workspace_id,
                cwd,
                ratio,
                focus,
            } => serde_json::to_value(manager.pane_split(
                session_name.as_deref(),
                direction,
                target_pane_id,
                workspace_id,
                cwd,
                ratio,
                focus.unwrap_or(true),
            )?)
            .map_err(|e| e.to_string()),
            Self::PaneZoom {
                session_name,
                pane_id,
                mode,
            } => serde_json::to_value(manager.pane_zoom(session_name.as_deref(), pane_id, mode)?)
                .map_err(|e| e.to_string()),
            Self::PaneSwap {
                session_name,
                source_pane_id,
                target_pane_id,
                pane_id,
                direction,
            } => serde_json::to_value(manager.pane_swap(
                session_name.as_deref(),
                source_pane_id,
                target_pane_id,
                pane_id,
                direction,
            )?)
            .map_err(|e| e.to_string()),
            Self::PaneClose {
                session_name,
                pane_id,
            } => serde_json::to_value(manager.pane_close(session_name.as_deref(), pane_id)?)
                .map_err(|e| e.to_string()),
            Self::LayoutExport {
                session_name,
                tab_id,
                pane_id,
            } => serde_json::to_value(manager.layout_export(
                session_name.as_deref(),
                tab_id,
                pane_id,
            )?)
            .map_err(|e| e.to_string()),
            Self::LayoutSetSplitRatio {
                session_name,
                tab_id,
                pane_id,
                path,
                ratio,
            } => serde_json::to_value(manager.layout_set_split_ratio(
                session_name.as_deref(),
                tab_id,
                pane_id,
                path,
                ratio,
            )?)
            .map_err(|e| e.to_string()),
            Self::AgentGet {
                session_name,
                target,
            } => serde_json::to_value(manager.agent_get(session_name.as_deref(), target)?)
                .map_err(|e| e.to_string()),
            Self::AgentRead {
                session_name,
                target,
                source,
                format,
                lines,
                strip_ansi,
            } => serde_json::to_value(manager.agent_read(
                session_name.as_deref(),
                target,
                source,
                format,
                lines,
                strip_ansi,
            )?)
            .map_err(|e| e.to_string()),
        }
    }
}
