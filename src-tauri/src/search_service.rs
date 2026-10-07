use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
pub use yuzora_host::search::*;
pub struct SearchState(pub std::sync::Arc<AtomicU64>);

// Filename scans must not cancel the independent content-search stream.
static FILE_NAME_SEARCH_GENERATION: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub async fn search_workspace_file_names(
    root: String,
    query: String,
) -> Result<yuzora_host::file_name_search::FileNameSearchResult, String> {
    let generation = FILE_NAME_SEARCH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || {
        yuzora_host::file_name_search::run_file_name_search(
            Path::new(&root),
            &query,
            generation,
            &FILE_NAME_SEARCH_GENERATION,
            std::time::Duration::from_secs(5),
        )
    })
    .await
    .map_err(|e| format!("file name search task failed: {e}"))?
}

#[tauri::command(async)]
pub fn search_workspace(
    state: tauri::State<SearchState>,
    root: String,
    query: String,
    case_sensitive: bool,
    on_event: tauri::ipc::Channel<SearchEvent>,
) -> Result<(), String> {
    let generation = state.0.fetch_add(1, Ordering::SeqCst) + 1;
    let gen_source = state.0.clone();
    std::thread::spawn(move || {
        run_search(
            Path::new(&root),
            &query,
            case_sensitive,
            generation,
            &gen_source,
            &mut |event| {
                let _ = on_event.send(event);
            },
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_name_search_local_command_uses_shared_core() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir(tmp.path().join("ignored")).unwrap();
        std::fs::write(tmp.path().join("ignored/target.ts"), "").unwrap();
        std::fs::write(tmp.path().join("Target.ts"), "").unwrap();
        tauri::async_runtime::block_on(async {
            let result = super::search_workspace_file_names(
                tmp.path().to_str().unwrap().into(),
                "TARGET".into(),
            )
            .await
            .unwrap();
            assert!(!result.incomplete);
            assert_eq!(result.files.len(), 1);
            assert_eq!(result.files[0].name, "Target.ts");
            assert!(!result.files[0].is_dir);
            assert!(super::search_workspace_file_names(
                tmp.path().join("missing").to_str().unwrap().into(),
                "target".into(),
            )
            .await
            .is_err());
        });
    }
}
