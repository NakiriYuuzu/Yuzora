//! Tauri adapters for HERDR machines (saved SSH machines). Always the local
//! manager: no session name and no runtime-scope routing.
use std::sync::Arc;
use yuzora_host::herdr_machines::{
    self, HerdrMachine, HerdrMachineInteractiveOpened, HerdrMachineInteractiveSpec,
    HerdrMachineSnapshot, HerdrMachineStatus, HerdrMachinesCapabilities,
};
use yuzora_host::herdr_service::{HerdrClientSize, HerdrManager, HerdrState, HerdrTerminalEvent};

async fn with_manager<T: Send + 'static>(
    state: &HerdrState,
    operation: impl FnOnce(Arc<HerdrManager>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let manager = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || operation(manager))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn herdr_machines_capabilities(
    state: tauri::State<'_, HerdrState>,
) -> Result<HerdrMachinesCapabilities, String> {
    with_manager(&state, |manager| {
        Ok(herdr_machines::machines_capabilities(&manager))
    })
    .await
}

#[tauri::command]
pub async fn herdr_machines_list(
    state: tauri::State<'_, HerdrState>,
) -> Result<Vec<HerdrMachine>, String> {
    with_manager(&state, |manager| herdr_machines::machines_list(&manager)).await
}

#[tauri::command]
pub async fn herdr_machines_status(
    state: tauri::State<'_, HerdrState>,
    machine_id: String,
) -> Result<HerdrMachineStatus, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_status(&manager, &machine_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_machines_agents(
    state: tauri::State<'_, HerdrState>,
    machine_id: String,
) -> Result<HerdrMachineSnapshot, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_agents(&manager, &machine_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_machines_rename(
    state: tauri::State<'_, HerdrState>,
    machine_id: String,
    label: String,
) -> Result<Vec<HerdrMachine>, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_rename(&manager, &machine_id, &label)
    })
    .await
}

#[tauri::command]
pub async fn herdr_machines_set_enabled(
    state: tauri::State<'_, HerdrState>,
    machine_id: String,
    enabled: bool,
) -> Result<Vec<HerdrMachine>, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_set_enabled(&manager, &machine_id, enabled)
    })
    .await
}

#[tauri::command]
pub async fn herdr_machines_remove(
    state: tauri::State<'_, HerdrState>,
    machine_id: String,
) -> Result<Vec<HerdrMachine>, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_remove(&manager, &machine_id)
    })
    .await
}

#[tauri::command]
pub async fn herdr_machine_interactive_open(
    state: tauri::State<'_, HerdrState>,
    spec: HerdrMachineInteractiveSpec,
    size: HerdrClientSize,
    on_event: tauri::ipc::Channel<HerdrTerminalEvent>,
) -> Result<HerdrMachineInteractiveOpened, String> {
    with_manager(&state, move |manager| {
        herdr_machines::machines_interactive_open(
            &manager,
            &spec,
            size,
            Arc::new(move |event| on_event.send(event).map_err(|e| e.to_string())),
        )
    })
    .await
}
