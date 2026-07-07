// F1 performance monitor: sample this process's own CPU% and memory.
//
// A persistent `System` lives in Tauri managed state so successive
// `perf_snapshot` calls (driven by the 2s frontend poll) are spaced far enough
// apart to satisfy sysinfo's minimum CPU update interval. Only the app's own PID
// is refreshed — on macOS the WKWebView WebContent/GPU helpers live in separate
// XPC processes that sysinfo can't reliably attribute, so both platforms report
// just the main process.

use std::sync::Mutex;

use sysinfo::{get_current_pid, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// The persistent `System`, refreshed once per `perf_snapshot`. CPU% is a delta
/// from the previous refresh, so the first sample after startup reads 0.
pub struct PerfState(pub Mutex<System>);

#[derive(serde::Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PerfSnapshot {
    /// Raw sysinfo CPU usage; can exceed 100 on multi-core machines.
    pub cpu_percent: f32,
    pub memory_bytes: u64,
}

fn sample(system: &mut System, pid: Pid) -> Option<PerfSnapshot> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    let process = system.process(pid)?;
    Some(PerfSnapshot {
        cpu_percent: process.cpu_usage(),
        memory_bytes: process.memory(),
    })
}

#[tauri::command]
pub fn perf_snapshot(state: tauri::State<'_, PerfState>) -> Result<Option<PerfSnapshot>, String> {
    let pid = get_current_pid().map_err(|e| e.to_string())?;
    let mut system = state.0.lock().map_err(|e| e.to_string())?;
    Ok(sample(&mut system, pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_reports_memory_and_finite_cpu_for_own_process() {
        let pid = get_current_pid().unwrap();
        let mut system = System::new();
        // First refresh primes the CPU delta baseline; the second, after a delay
        // past the minimum update interval, yields a real percentage.
        let _ = sample(&mut system, pid);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let snapshot = sample(&mut system, pid).expect("own process is alive");
        assert!(snapshot.memory_bytes > 0, "memory should be > 0");
        // `>= 0.0` is false for NaN, so this also rules out a garbage CPU reading.
        assert!(
            snapshot.cpu_percent >= 0.0,
            "cpu should be non-negative and finite"
        );
    }

    #[test]
    fn sample_returns_none_for_missing_pid() {
        let mut system = System::new();
        // A PID that is overwhelmingly unlikely to exist must return None, not panic.
        assert!(sample(&mut system, Pid::from_u32(u32::MAX)).is_none());
    }
}
