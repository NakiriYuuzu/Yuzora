//! Tauri adapters for the shared official HERDR runtime facade.
use std::sync::Arc;
use yuzora_host::herdr_limits::bounded_ipc;
pub use yuzora_host::herdr_service::*;

async fn with_herdr_manager<T: Send + 'static>(
    state: &HerdrState,
    operation: impl FnOnce(Arc<HerdrManager>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || operation(manager))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_client_open(
    state: tauri::State<'_, HerdrState>,
    session_name: String,
    size: HerdrClientSize,
    on_event: tauri::ipc::Channel<HerdrTerminalEvent>,
) -> Result<HerdrTerminalOpenResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.open_native_client(
            &session_name,
            size,
            Arc::new(move |event| on_event.send(event).map_err(|e| e.to_string())),
        )
    })
    .await
}

#[tauri::command]
pub async fn herdr_feature(
    state: tauri::State<'_, HerdrState>,
    session_name: String,
    request: HerdrFeatureRequest,
) -> Result<serde_json::Value, String> {
    with_herdr_manager(&state, move |manager| {
        manager.feature(&session_name, request)
    })
    .await
}

#[tauri::command]
pub async fn herdr_sessions(
    state: tauri::State<'_, HerdrState>,
    cached: Option<bool>,
) -> Result<Vec<HerdrNamedSession>, String> {
    with_herdr_manager(&state, move |manager| {
        if cached == Some(true) {
            manager.list_sessions_polled()
        } else {
            manager.list_sessions()
        }
    })
    .await
}

#[tauri::command]
pub async fn herdr_capabilities(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrCapabilities, String> {
    with_herdr_manager(&state, move |manager| {
        bounded_ipc(manager.capabilities_for_session(session_name.as_deref()))
    })
    .await
}

#[tauri::command]
pub async fn herdr_snapshot(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
) -> Result<HerdrSnapshotResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.snapshot(session_name.as_deref())
    })
    .await
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
    let mode = mode.unwrap_or(HerdrTerminalMode::Observe);
    let takeover = takeover.unwrap_or(false);
    with_herdr_manager(&state, move |manager| {
        let channel = on_event;
        let on_event: OnTerminalEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.open_terminal(target, mode, takeover, cols, rows, session_name, on_event)
    })
    .await
}

#[tauri::command]
pub async fn herdr_terminal_input(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    text: Option<String>,
    bytes_base64: Option<String>,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.terminal_input(&session_id, text, bytes_base64)
    })
    .await
}

#[tauri::command]
pub async fn terminal_clipboard_image(png_base64: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || yuzora_host::clipboard_image::stage(&png_base64))
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
    with_herdr_manager(&state, move |manager| {
        manager.terminal_resize(&session_id, cols, rows)
    })
    .await
}

#[tauri::command]
pub async fn herdr_terminal_scroll(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    direction: HerdrScrollDirection,
    lines: u32,
    column: Option<u16>,
    row: Option<u16>,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.terminal_scroll(&session_id, direction, lines, column, row)
    })
    .await
}

#[tauri::command]
pub async fn herdr_terminal_mouse(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
    action: HerdrMouseAction,
    column: u16,
    row: u16,
    modifiers: u8,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.terminal_mouse(&session_id, action, column, row, modifiers)
    })
    .await
}

#[tauri::command]
pub async fn herdr_terminal_release(
    state: tauri::State<'_, HerdrState>,
    session_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| manager.terminal_release(&session_id)).await
}

#[tauri::command]
pub async fn herdr_terminal_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: Option<String>,
    title: Option<String>,
) -> Result<HerdrTerminalCreateResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.create_terminal(session_name.as_deref(), workspace_id, title)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.workspace_focus(session_name.as_deref(), workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_create(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    label: Option<String>,
    focus: Option<bool>,
) -> Result<HerdrWorkspaceCreateResult, String> {
    let focus = focus.unwrap_or(true);
    with_herdr_manager(&state, move |manager| {
        manager.workspace_create(session_name.as_deref(), cwd, label, focus)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_move(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    insert_index: u32,
) -> Result<yuzora_host::herdr_service::HerdrWorkspaceOrderResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.workspace_move(session_name.as_deref(), workspace_id, insert_index)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_move_block(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_ids: Vec<String>,
    before_workspace_id: Option<String>,
) -> Result<yuzora_host::herdr_service::HerdrWorkspaceOrderResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.workspace_move_block(session_name.as_deref(), workspace_ids, before_workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
    label: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.workspace_rename(session_name.as_deref(), workspace_id, label)
    })
    .await
}

#[tauri::command]
pub async fn herdr_workspace_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    workspace_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.workspace_close(session_name.as_deref(), workspace_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_worktree_list(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    cwd: Option<String>,
    workspace_id: Option<String>,
) -> Result<HerdrWorktreeListResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.worktree_list(session_name.as_deref(), cwd, workspace_id)
    })
    .await
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
    let focus = focus.unwrap_or(true);
    with_herdr_manager(&state, move |manager| {
        manager.tab_create(session_name.as_deref(), workspace_id, label, cwd, focus)
    })
    .await
}

#[tauri::command]
pub async fn herdr_tab_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.tab_focus(session_name.as_deref(), tab_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_tab_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    label: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.tab_rename(session_name.as_deref(), tab_id, label)
    })
    .await
}

#[tauri::command]
pub async fn herdr_tab_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.tab_close(session_name.as_deref(), tab_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_tab_move(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: String,
    insert_index: u32,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.tab_move(session_name.as_deref(), tab_id, insert_index)
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_focus(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_focus(session_name.as_deref(), pane_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_rename(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
    label: Option<String>,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_rename(session_name.as_deref(), pane_id, label)
    })
    .await
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
    let focus = focus.unwrap_or(true);
    with_herdr_manager(&state, move |manager| {
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
}

#[tauri::command]
pub async fn herdr_pane_zoom(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: Option<String>,
    mode: Option<HerdrPaneZoomMode>,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_zoom(session_name.as_deref(), pane_id, mode)
    })
    .await
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
    with_herdr_manager(&state, move |manager| {
        manager.pane_swap(
            session_name.as_deref(),
            source_pane_id,
            target_pane_id,
            pane_id,
            direction,
        )
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_close(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_close(session_name.as_deref(), pane_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_layout_export(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
) -> Result<HerdrLayoutDescription, String> {
    with_herdr_manager(&state, move |manager| {
        manager.layout_export(session_name.as_deref(), tab_id, pane_id)
    })
    .await
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
    with_herdr_manager(&state, move |manager| {
        manager.layout_set_split_ratio(session_name.as_deref(), tab_id, pane_id, path, ratio)
    })
    .await
}

#[tauri::command]
pub async fn herdr_binary_source_get(
    state: tauri::State<'_, HerdrState>,
) -> Result<HerdrBinarySourceInfo, String> {
    with_herdr_manager(&state, move |manager| Ok(manager.binary_source_info())).await
}

#[tauri::command]
pub async fn herdr_binary_source_set(
    state: tauri::State<'_, HerdrState>,
    source: HerdrBinarySource,
    custom_path: Option<String>,
) -> Result<HerdrBinarySourceSetResult, String> {
    with_herdr_manager(&state, move |manager| {
        manager.set_binary_source_with_path(source, custom_path)
    })
    .await
}

#[tauri::command]
pub async fn herdr_binary_source_check(
    state: tauri::State<'_, HerdrState>,
    source: HerdrBinarySource,
    custom_path: Option<String>,
) -> Result<yuzora_host::herdr_runtime::RuntimeBinaryCheck, String> {
    with_herdr_manager(&state, move |manager| {
        manager.check_binary_source(source, custom_path)
    })
    .await
}

#[tauri::command]
pub async fn herdr_events_subscribe(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_ids: Option<Vec<String>>,
    on_event: tauri::ipc::Channel<HerdrSubscriptionEvent>,
) -> Result<String, String> {
    with_herdr_manager(&state, move |manager| {
        let channel = on_event;
        let on_event: OnSubscriptionEvent =
            Arc::new(move |event| channel.send(event).map_err(|e| e.to_string()));
        manager.events_subscribe(session_name, pane_ids.unwrap_or_default(), on_event)
    })
    .await
}

#[tauri::command]
pub async fn herdr_events_release(
    state: tauri::State<'_, HerdrState>,
    subscription_id: String,
) -> Result<(), String> {
    with_herdr_manager(&state, move |manager| {
        manager.events_release(&subscription_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_scroll_state(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
) -> Result<Option<yuzora_host::herdr_scroll::HerdrPaneScrollInfo>, String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_scroll_state(session_name.as_deref(), pane_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_scroll_to(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
    offset_from_bottom: u64,
) -> Result<Option<yuzora_host::herdr_scroll::HerdrPaneScrollInfo>, String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_scroll_to(session_name.as_deref(), pane_id, offset_from_bottom)
    })
    .await
}

#[tauri::command]
pub async fn herdr_pane_selection_read(
    state: tauri::State<'_, HerdrState>,
    session_name: Option<String>,
    pane_id: String,
    anchor: yuzora_host::herdr_scroll::HerdrPaneTextPoint,
    cursor: yuzora_host::herdr_scroll::HerdrPaneTextPoint,
) -> Result<String, String> {
    with_herdr_manager(&state, move |manager| {
        manager.pane_selection_read(session_name.as_deref(), pane_id, anchor, cursor)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_adapter_clones_the_manager_and_preserves_results() {
        let state = HerdrState(Arc::new(HerdrManager::new()));
        let expected = state.0.clone();
        let caller = std::thread::current().id();
        let result = tauri::async_runtime::block_on(with_herdr_manager(&state, move |manager| {
            assert!(Arc::ptr_eq(&manager, &expected));
            assert_ne!(std::thread::current().id(), caller);
            Ok(42)
        }));
        assert_eq!(result, Ok(42));
        let error = tauri::async_runtime::block_on(with_herdr_manager(&state, |_| {
            Err::<(), _>("herdr-operation-error".to_string())
        }));
        assert_eq!(error, Err("herdr-operation-error".to_string()));
    }

    #[test]
    fn blocking_adapter_stringifies_join_errors() {
        let state = HerdrState(Arc::new(HerdrManager::new()));
        let error =
            tauri::async_runtime::block_on(with_herdr_manager(&state, |_| -> Result<(), String> {
                panic!("herdr-adapter-panic");
            }))
            .unwrap_err();
        assert!(error.contains("herdr-adapter-panic"));
    }

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
            "herdr_service::herdr_pane_scroll_state",
            "herdr_service::herdr_pane_scroll_to",
            "herdr_service::herdr_pane_selection_read",
            "herdr_service::herdr_pane_rename",
            "herdr_service::herdr_pane_split",
            "herdr_service::herdr_pane_zoom",
            "herdr_service::herdr_pane_swap",
            "herdr_service::herdr_pane_close",
            "herdr_service::herdr_layout_export",
            "herdr_service::herdr_layout_set_split_ratio",
            "herdr_service::herdr_binary_source_get",
            "herdr_service::herdr_binary_source_set",
            "herdr_service::herdr_events_subscribe",
            "herdr_service::herdr_events_release",
        ] {
            assert!(source.contains(cmd), "missing command registration: {cmd}");
        }
    }
}
