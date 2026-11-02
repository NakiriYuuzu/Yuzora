//! What one sample costs, measured from outside a running app: samples the
//! app's pid the way the app does, every 2 s, and prints the median and p95.
//!
//! Run it with the app's environment (HC_SESSION, XDG_*), so `hc` names the
//! same core:
//!
//! ```sh
//! cargo run --release -p yuzora-perf --example cost -- <app pid> [samples]
//! ```

use std::time::{Duration, Instant};
use yuzora_perf::health::{median, percentile};
use yuzora_perf::sample::Sampler;

fn main() {
    let mut args = std::env::args().skip(1);
    let pid: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .expect("usage: cost <app pid> [samples]");
    let count: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(30);
    let mut sampler = Sampler::new(
        sysinfo::Pid::from_u32(pid),
        &["hc", "herdr-core"],
        yuzora_perf::core::ask,
    );
    let mut ms = Vec::new();
    for i in 0..=count {
        let started = Instant::now();
        let snap = sampler
            .sample()
            .expect("sample")
            .expect("the app is running");
        let took = started.elapsed().as_secs_f64() * 1000.0;
        // The first sample primes CPU and asks hc: reported, not counted.
        if i > 0 {
            ms.push(took);
        }
        let u = snap.usage;
        println!(
            "{i:>3} {took:>7.2} ms  total {:>6.1}% {:>7.1} MB {:>3} | app {:>6.1} MB | core {:>3} {:>7.1} MB (pid {:?}) | tools {:>3} {:>6.1} MB",
            u.total.cpu,
            u.total.memory as f64 / 1e6,
            u.total.count,
            u.app.memory as f64 / 1e6,
            u.core.count,
            u.core.memory as f64 / 1e6,
            snap.core_pid,
            u.tools.count,
            u.tools.memory as f64 / 1e6,
        );
        if i < count {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    println!(
        "samples {}  median {:.2} ms  p95 {:.2} ms  max {:.2} ms",
        ms.len(),
        median(&ms),
        percentile(&ms, 95.0),
        ms.iter().cloned().fold(0.0, f64::max)
    );
}
