//! Yuzora L8（效能監控）: what this computer's Yuzora uses, for the chip in the
//! status bar's right corner (mod `yuzora-perf`).
//!
//! Owned by lane L8. Scripts reach it as `import { … } from "yuzora:perf"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs).
//!
//! - [`tree`]: which processes count, and as the app, herdr-core or a tool.
//! - [`core`]: finding this app's herdr-core server (`hc server status`).
//! - [`sample`]: one sample, with a `System` kept between samples.
//! - [`health`]: the last 30 polls' outcomes and what a sample cost.
//!
//! Sampling runs on one worker thread that owns the `System`; the script's
//! poll only hands it a request and awaits the answer, so the UI thread never
//! waits on the process table.

pub mod core;
pub mod health;
pub mod sample;
pub mod tree;

use health::{Health, Outcome};
use sample::{Sampler, Snapshot};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};
use yuzora_core::host::to_host;
use yuzora_core::{HostModule, HostValue};

/// The import specifier: `yuzora:perf`.
pub const NAME: &str = "yuzora:perf";

/// How often the mod polls.
pub const INTERVAL: Duration = Duration::from_secs(2);

/// A poll closer than this to the last one only reads: two copies of the mod
/// still sample once every 2 s.
pub const MIN_GAP: Duration = Duration::from_millis(1500);

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
export interface PerfUsage { cpu: number; memory: number; count: number }
export interface PerfSnapshot {
  /** The three below added up. cpu: one core = 100, summed over processes (can pass 100); memory: RSS, bytes. */
  total: PerfUsage;
  /** The app's own process. */
  app: PerfUsage;
  /** This app's herdr-core: its server and holders and what runs in them (terminals, agents). */
  core: PerfUsage;
  /** Everything else the app started (git, ssh, database helpers, hc connect), and what can't be told apart. */
  tools: PerfUsage;
  /** The herdr-core server measured; null when it couldn't be found (core_error says why: then core is 0). */
  core_pid: number | null;
  core_error: string | null;
}
/** The last 30 polls: failed and empty samples, and polls skipped while the window wasn't in front. */
export interface PerfHealth { attempts: number; failures: number; empty: number; skipped: number; last_error: string | null }
/** What one sample took, over the last 30 samples. */
export interface PerfCost { last_ms: number; median_ms: number; p95_ms: number; samples: number }
export interface PerfState { snapshot: PerfSnapshot | null; health: PerfHealth; cost: PerfCost | null }
/** The yuzora-perf crate's version (Yuzora's). */
export function version(): string;
/** One poll (every 2 s). focused: the window is in front, so this computer's usage is sampled on a worker thread;
 *  otherwise the poll counts as skipped. A poll within 1.5 s of the last, or while a sample runs, only reads. */
export function tick(focused: boolean): Promise<PerfState>;
/** The last sample and the last 30 polls, without sampling. */
export function state(): PerfState;
"#;

/// What a poll does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begin {
    /// Too soon after the last, or a sample is running: read only.
    Folded,
    /// The window wasn't in front: counted, not sampled.
    Skipped,
    /// Sample now; [`Monitor::finish`] follows.
    Sample,
}

/// The last sample and how sampling went, shared by every copy of the mod.
#[derive(Debug, Default)]
pub struct Monitor {
    snapshot: Option<Snapshot>,
    health: Health,
    last_poll: Option<Instant>,
    busy: bool,
}

impl Monitor {
    pub fn begin(&mut self, focused: bool, now: Instant) -> Begin {
        if self.busy
            || self
                .last_poll
                .is_some_and(|at| now.duration_since(at) < MIN_GAP)
        {
            return Begin::Folded;
        }
        self.last_poll = Some(now);
        if !focused {
            self.health.record(Outcome::SkippedNoFocus, None);
            return Begin::Skipped;
        }
        self.busy = true;
        Begin::Sample
    }

    /// A sample's result. An empty or failed one keeps the last good snapshot
    /// on screen (with a warning) rather than blanking it.
    pub fn finish(&mut self, result: Result<Option<Snapshot>, String>, took: Option<Duration>) {
        self.busy = false;
        match result {
            Ok(Some(snapshot)) => {
                self.snapshot = Some(snapshot);
                self.health.record(Outcome::Ok, None);
            }
            Ok(None) => self.health.record(Outcome::Empty, None),
            Err(e) => self.health.record(Outcome::Failed, Some(e)),
        }
        if let Some(took) = took {
            self.health.record_cost(took);
        }
    }

    pub fn state(&self) -> serde_json::Value {
        serde_json::json!({ "snapshot": self.snapshot, "health": self.health.summary(), "cost": self.health.cost() })
    }
}

static MONITOR: LazyLock<Mutex<Monitor>> = LazyLock::new(Mutex::default);

fn monitor() -> MutexGuard<'static, Monitor> {
    MONITOR.lock().unwrap_or_else(PoisonError::into_inner)
}

type Reply = async_channel::Sender<()>;

/// The worker thread that owns the `System`; started on the first sample.
fn worker() -> Result<&'static mpsc::Sender<Reply>, String> {
    static WORKER: OnceLock<Result<mpsc::Sender<Reply>, String>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Reply>();
            std::thread::Builder::new()
                .name("yuzora-perf".into())
                .spawn(move || {
                    let mut sampler = Sampler::for_this_app();
                    for reply in rx {
                        let started = Instant::now();
                        let result = match &mut sampler {
                            Ok(s) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                s.sample()
                            }))
                            .unwrap_or_else(|_| Err("the sampler panicked".into())),
                            Err(e) => Err(e.clone()),
                        };
                        monitor().finish(result, Some(started.elapsed()));
                        let _ = reply.send_blocking(());
                    }
                })
                .map_err(|e| format!("cannot start the sampler: {e}"))?;
            Ok(tx)
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// The `yuzora:perf` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| {
            Ok(HostValue::Str(yuzora_core::VERSION.into()))
        })
        .function("state", |_| Ok(to_host(&monitor().state())))
        .async_function("tick", |a| {
            let focused = matches!(a.get(0), Some(HostValue::Bool(true)));
            let mut answer = None;
            if monitor().begin(focused, Instant::now()) == Begin::Sample {
                let (tx, rx) = async_channel::bounded(1);
                match worker()
                    .and_then(|w| w.send(tx).map_err(|_| "the sampler stopped".to_owned()))
                {
                    Ok(()) => answer = Some(rx),
                    Err(e) => monitor().finish(Err(e), None),
                }
            }
            Ok(async move {
                if let Some(rx) = answer
                    && rx.recv().await.is_err()
                {
                    monitor().finish(Err("the sampler stopped".into()), None);
                }
                Ok(to_host(&monitor().state()))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Usage, Usages};
    use yuzora_core::HostArguments;

    fn snapshot(memory: u64) -> Snapshot {
        let app = Usage {
            cpu: 1.0,
            memory,
            count: 1,
        };
        Snapshot {
            usage: Usages {
                total: app,
                app,
                core: Usage::default(),
                tools: Usage::default(),
            },
            core_pid: Some(7),
            core_error: None,
        }
    }

    #[test]
    fn module_validates_and_reads_without_sampling() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:perf");
        assert_eq!(
            m.call("version", &HostArguments::new([])).unwrap(),
            HostValue::Str(yuzora_core::VERSION.into())
        );
        assert!(m.is_async("tick"));
        let state =
            yuzora_core::host::from_host(&m.call("state", &HostArguments::new([])).unwrap());
        assert!(state["health"]["attempts"].is_number());
        assert!(state.get("snapshot").is_some() && state.get("cost").is_some());
    }

    #[test]
    fn unfocused_polls_are_counted_not_sampled() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        assert_eq!(m.begin(false, t0), Begin::Skipped);
        assert_eq!(m.state()["health"]["skipped"], 1);
        assert_eq!(m.state()["cost"], serde_json::Value::Null);
        assert_eq!(m.state()["snapshot"], serde_json::Value::Null);
    }

    #[test]
    fn close_or_overlapping_polls_only_read() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        assert_eq!(m.begin(true, t0), Begin::Sample);
        // A second copy of the mod, or a slow sample: no second sample.
        assert_eq!(
            m.begin(true, t0 + Duration::from_millis(200)),
            Begin::Folded
        );
        assert_eq!(m.begin(true, t0 + INTERVAL), Begin::Folded, "still busy");
        m.finish(Ok(Some(snapshot(100))), Some(Duration::from_millis(3)));
        assert_eq!(m.begin(true, t0 + INTERVAL), Begin::Sample);
        m.finish(Ok(Some(snapshot(100))), Some(Duration::from_millis(5)));
        assert_eq!(
            m.begin(false, t0 + INTERVAL + Duration::from_millis(100)),
            Begin::Folded
        );
        assert_eq!(m.state()["health"]["attempts"], 2);
        assert_eq!(m.state()["cost"]["samples"], 2);
    }

    #[test]
    fn empty_and_failed_samples_keep_the_last_good_one_and_warn() {
        let t0 = Instant::now();
        let mut m = Monitor::default();
        m.begin(true, t0);
        m.finish(
            Ok(Some(snapshot(4_000_000))),
            Some(Duration::from_millis(2)),
        );
        m.begin(true, t0 + INTERVAL);
        m.finish(Ok(None), Some(Duration::from_millis(2)));
        m.begin(true, t0 + INTERVAL * 2);
        m.finish(Err("denied".into()), Some(Duration::from_millis(2)));
        let s = m.state();
        assert_eq!(s["snapshot"]["total"]["memory"], 4_000_000);
        assert_eq!(s["snapshot"]["core_pid"], 7);
        assert_eq!(
            (
                s["health"]["empty"].clone(),
                s["health"]["failures"].clone()
            ),
            (1.into(), 1.into())
        );
        assert_eq!(s["health"]["last_error"], "denied");
    }
}
