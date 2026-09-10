//! Desktop picker adapters for the shared descriptor-relative filesystem core.
pub use yuzora_host::path_capability::*;

#[tauri::command(async)]
pub async fn sftp_pick_download_destination(
    app: tauri::AppHandle,
    state: tauri::State<'_, DownloadDestinationState>,
    suggested_leaf: String,
) -> Result<Option<DownloadDestinationGrant>, String> {
    use tauri_plugin_dialog::DialogExt;
    let leaf = SafeLeafName::parse(&suggested_leaf).map_err(String::from)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_can_create_directories(false)
        .set_file_name(leaf.as_str())
        .save_file(move |file| {
            let _ = tx.send(file);
        });
    let Some(file) = rx
        .await
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?
    else {
        return Ok(None);
    };
    let path = file
        .into_path()
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?;
    state.0.grant(&path).map(Some).map_err(String::from)
}

#[tauri::command(async)]
pub async fn sftp_pick_selected_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, SelectedPathState>,
) -> Result<Vec<SelectedPathGrant>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_can_create_directories(false)
        .pick_files(move |files| {
            let _ = tx.send(files);
        });
    let Some(files) = rx
        .await
        .map_err(|_| PathCapabilityError::Io.as_code().to_string())?
    else {
        return Ok(Vec::new());
    };
    let mut grants = Vec::with_capacity(files.len());
    for file in files {
        let path = file
            .into_path()
            .map_err(|_| PathCapabilityError::Io.as_code().to_string())?;
        grants.push(grant_native_selection(&state.0, &path).map_err(String::from)?);
    }
    Ok(grants)
}
