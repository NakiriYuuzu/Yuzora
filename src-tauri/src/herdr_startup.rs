//! Non-blocking local Herdr bootstrap and its observable completion state.
use std::sync::{Arc, Mutex};
use tauri::Emitter;

use crate::herdr_service::HerdrManager;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HerdrStartupStatus {
    state: HerdrStartupPhase,
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum HerdrStartupPhase {
    Starting,
    Ready,
    Failed,
}

pub struct HerdrStartupState(Arc<Mutex<HerdrStartupStatus>>);

impl HerdrStartupState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HerdrStartupStatus {
            state: HerdrStartupPhase::Starting,
            error: None,
        })))
    }

    pub fn launch(&self, app: tauri::AppHandle, manager: Arc<HerdrManager>) {
        self.spawn(
            move || manager.ensure_server_running_on_startup(),
            move |status| {
                if let Some(error) = &status.error {
                    eprintln!("herdr server startup failed: {error}");
                }
                if let Err(error) = app.emit("herdr:startup", status) {
                    eprintln!("herdr startup event failed: {error}");
                }
            },
        );
    }

    fn spawn(
        &self,
        startup: impl FnOnce() -> Result<bool, String> + Send + 'static,
        emit: impl FnOnce(HerdrStartupStatus) + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        let state = self.0.clone();
        std::thread::spawn(move || {
            let error = startup().err();
            let status = HerdrStartupStatus {
                state: if error.is_some() {
                    HerdrStartupPhase::Failed
                } else {
                    HerdrStartupPhase::Ready
                },
                error,
            };
            *state.lock().unwrap() = status.clone();
            emit(status);
        })
    }
}

#[tauri::command]
pub fn herdr_startup_status(state: tauri::State<'_, HerdrStartupState>) -> HerdrStartupStatus {
    state.0.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn startup_is_non_blocking_and_publishes_one_completion_after_state_update() {
        for outcome in [Ok(true), Ok(false), Err("startup failed".to_string())] {
            let state = HerdrStartupState::new();
            assert_eq!(
                serde_json::to_value(state.0.lock().unwrap().clone()).unwrap(),
                serde_json::json!({"state": "starting", "error": null})
            );
            let (release, wait) = mpsc::channel();
            let (send, receive) = mpsc::channel();
            let observed = state.0.clone();
            let expected = outcome.clone();
            let worker = state.spawn(
                move || {
                    wait.recv().unwrap();
                    outcome
                },
                move |status| {
                    assert_eq!(*observed.lock().unwrap(), status);
                    send.send(status).unwrap();
                },
            );
            assert_eq!(state.0.lock().unwrap().state, HerdrStartupPhase::Starting);
            assert!(receive.try_recv().is_err());
            release.send(()).unwrap();
            worker.join().unwrap();
            let status = receive.recv().unwrap();
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!({
                    "state": if expected.is_ok() { "ready" } else { "failed" },
                    "error": expected.err(),
                })
            );
            assert!(
                receive.recv().is_err(),
                "completion must only be emitted once"
            );
        }
    }
}
