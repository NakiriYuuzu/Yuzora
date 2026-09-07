//! Tauri adapters for the shared official HERDR runtime facade.
use std::sync::Arc;
use yuzora_host::herdr_limits::bounded_ipc;
pub use yuzora_host::herdr_service::*;

#[tauri::command]
pub async fn herdr_sessions(
    state: tauri::State<'_, HerdrState>,
) -> Result<Vec<HerdrNamedSession>, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.list_sessions())
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_capabilities(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrCapabilities, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        bounded_ipc(manager.capabilities_for_session(session_name.as_deref()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_snapshot(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrSnapshotResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.snapshot(session_name.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn herdr_terminal_open(
    state: tauri::State<'_, HerdrState>,
    target: String,
    mode: Option<HerdrTerminalMode>,
    takeover: Option<bool>,
    cols: u16,
    rows: u16,
    session_name: Option<String>,
    on_event: tauri::ipc::Channel<HerdrTerminalEvent>,
) -> Result<HerdrTerminalOpenResult, String> {
    let manager = state.0.clone();
    let mode = mode.unwrap_or(HerdrTerminalMode::Observe);
    let takeover = takeover.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_event;
        let on_event: OnTerminalEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.open_terminal(target, mode, takeover, cols, rows, session_name, on_event)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_input(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    text: Option<String>,
    bytes_base64: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.terminal_input(&session_id, text, bytes_base64)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_resize(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.terminal_resize(&session_id, cols, rows))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_scroll(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    direction: HerdrScrollDirection,
    lines: u32,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.terminal_scroll(&session_id, direction, lines)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_release(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.terminal_release(&session_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_terminal_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: Option<String>,
    title: Option<String>,
) -> Result<HerdrTerminalCreateResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.create_terminal(session_name.as_deref(), workspace_id, title)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_catalog(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<Vec<HerdrAgentCatalogEntry>, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.agent_catalog(session_name.as_deref()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    kind: String,
    bypass_permissions: Option<bool>,
) -> Result<HerdrAgentCreateResult, String> {
    let manager = state.0.clone();
    let bypass_permissions = bypass_permissions.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        manager.agent_create(
            session_name.as_deref(),
            workspace_id,
            kind,
            bypass_permissions,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_focus(session_name.as_deref(), workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    label: Option<String>,
    focus: Option<bool>,
) -> Result<HerdrWorkspaceCreateResult, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_create(session_name.as_deref(), cwd, label, focus)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    label: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_rename(session_name.as_deref(), workspace_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_workspace_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.workspace_close(session_name.as_deref(), workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_worktree_list(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    workspace_id: Option<String>,
) -> Result<HerdrWorktreeListResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.worktree_list(session_name.as_deref(), cwd, workspace_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: Option<String>,
    label: Option<String>,
    cwd: Option<String>,
    focus: Option<bool>,
) -> Result<HerdrTerminalCreateResult, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_create(session_name.as_deref(), workspace_id, label, cwd, focus)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.tab_focus(session_name.as_deref(), tab_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    label: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_rename(session_name.as_deref(), tab_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.tab_close(session_name.as_deref(), tab_id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_tab_move(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    insert_index: u32,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.tab_move(session_name.as_deref(), tab_id, insert_index)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_focus(session_name.as_deref(), pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
    label: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_rename(session_name.as_deref(), pane_id, label)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn herdr_pane_split(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    direction: HerdrSplitDirection,
    target_pane_id: Option<String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
    ratio: Option<f64>,
    focus: Option<bool>,
) -> Result<HerdrPaneIdentity, String> {
    let manager = state.0.clone();
    let focus = focus.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_split(
            session_name.as_deref(),
            direction,
            target_pane_id,
            workspace_id,
            cwd,
            ratio,
            focus,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_zoom(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: Option<String>,
    mode: Option<HerdrPaneZoomMode>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_zoom(session_name.as_deref(), pane_id, mode)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_swap(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    source_pane_id: Option<String>,
    target_pane_id: Option<String>,
    pane_id: Option<String>,
    direction: Option<String>,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_swap(
            session_name.as_deref(),
            source_pane_id,
            target_pane_id,
            pane_id,
            direction,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_pane_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.pane_close(session_name.as_deref(), pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_layout_export(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
) -> Result<HerdrLayoutDescription, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.layout_export(session_name.as_deref(), tab_id, pane_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_layout_set_split_ratio(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    path: Vec<bool>,
    ratio: f64,
) -> Result<HerdrLayoutDescription, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.layout_set_split_ratio(session_name.as_deref(), tab_id, pane_id, path, ratio)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_binary_source_get(
    state: tauri::State<'_, HerdrState>,
) -> Result<HerdrBinarySourceInfo, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.binary_source_info())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn herdr_binary_source_set(
    state: tauri::State<'_, HerdrState>,
    source: HerdrBinarySource,
) -> Result<HerdrBinarySourceSetResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_binary_source(source))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_get(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    target: String,
) -> Result<HerdrAgentDetails, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.agent_get(session_name.as_deref(), target))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_agent_read(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    target: String,
    source: HerdrReadSource,
    format: Option<HerdrReadFormat>,
    lines: Option<u32>,
    strip_ansi: Option<bool>,
) -> Result<HerdrAgentReadResult, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.agent_read(
            session_name.as_deref(),
            target,
            source,
            format,
            lines,
            strip_ansi,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_events_subscribe(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_ids: Option<Vec<String>>,
    on_event: tauri::ipc::Channel<HerdrSubscriptionEvent>,
) -> Result<String, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let channel = on_event;
        let on_event: OnSubscriptionEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.events_subscribe(session_name, pane_ids.unwrap_or_default(), on_event)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_events_release(
    state: tauri::State<'_, HerdrState>,
    subscription_id: String,
) -> Result<(), String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || manager.events_release(&subscription_id))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_interaction_commands_are_registered_in_lib() {
        let source = include_str!("lib.rs");
        for cmd in [
            "herdr_service::herdr_workspace_rename",
            "herdr_service::herdr_workspace_close",
            "herdr_service::herdr_worktree_list",
            "herdr_service::herdr_tab_create",
            "herdr_service::herdr_tab_focus",
            "herdr_service::herdr_tab_rename",
            "herdr_service::herdr_tab_close",
            "herdr_service::herdr_tab_move",
            "herdr_service::herdr_pane_focus",
            "herdr_service::herdr_pane_rename",
            "herdr_service::herdr_pane_split",
            "herdr_service::herdr_pane_zoom",
            "herdr_service::herdr_pane_swap",
            "herdr_service::herdr_pane_close",
            "herdr_service::herdr_layout_export",
            "herdr_service::herdr_layout_set_split_ratio",
            "herdr_service::herdr_binary_source_get",
            "herdr_service::herdr_binary_source_set",
            "herdr_service::herdr_agent_get",
            "herdr_service::herdr_agent_read",
            "herdr_service::herdr_events_subscribe",
            "herdr_service::herdr_events_release",
        ] {
            assert!(source.contains(cmd), "missing command registration: {cmd}");
        }
    }
}
