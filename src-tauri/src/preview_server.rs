//! Desktop commands use the same allowlist and server core as Unix hosts.
use tauri::State;
pub use yuzora_host::preview_server::{PreviewServerState, PreviewSessionInfo};

#[tauri::command(async)]
pub fn preview_create(
    path: String,
    state: State<'_, PreviewServerState>,
) -> Result<PreviewSessionInfo, String> {
    state.create_session(&path)
}

#[tauri::command(async)]
pub fn preview_revoke(token: String, state: State<'_, PreviewServerState>) -> Result<(), String> {
    state.revoke_session(&token);
    Ok(())
}

#[tauri::command(async)]
pub fn preview_stop_all(state: State<'_, PreviewServerState>) -> Result<(), String> {
    state.stop_all();
    Ok(())
}
