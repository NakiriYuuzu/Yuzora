use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
pub use yuzora_host::search::*;
pub struct SearchState(pub std::sync::Arc<AtomicU64>);

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
