//! Actual Host/SFTP failure paths with the owned SSH fixture's default denial.
use super::*;

async fn retention_action(fixture: &mut Fixture, mode: &str, expected: &str) {
    match mode {
        "host" | "sftp" => denied_open(fixture, mode, expected).await,
        "fence" => fixture.fence().await,
        "sampling" => tokio::task::yield_now().await,
        _ => panic!("unknown retention mode"),
    }
}

fn retention_setting(name: &str, default: usize) -> usize {
    let value = std::env::var(name)
        .map(|value| value.parse::<usize>().unwrap())
        .unwrap_or(default);
    assert!(value <= 100_000);
    value
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual owned SSH retention diagnosis with sampling controls"]
async fn performance_owned_rejected_open_retention() {
    tokio::time::timeout(Duration::from_secs(120), async {
        let mode = std::env::var("YUZORA_RETENTION_MODE").unwrap_or_else(|_| "host".into());
        assert!(["host", "sftp", "fence", "sampling"].contains(&mode.as_str()));
        let warmup = retention_setting("YUZORA_RETENTION_WARM", 5000);
        let cycles = retention_setting("YUZORA_RETENTION_CYCLES", 20_000);
        let sample_every = retention_setting("YUZORA_RETENTION_SAMPLE_EVERY", 100);
        assert!(cycles >= 100 && cycles.is_multiple_of(100));
        assert!(sample_every == 0 || sample_every.is_multiple_of(100) && cycles.is_multiple_of(sample_every));
        let expected = russh::Error::ChannelOpenFailure(
            russh::ChannelOpenFailure::AdministrativelyProhibited,
        )
        .to_string();
        // Nonzero initialization touches measurement pages before the first sample.
        let mut latencies = vec![u64::MAX; cycles];
        let sample_count = cycles.checked_div(sample_every).map_or(2, |count| count + 1);
        let mut samples = vec![ResourceSample { cycle: usize::MAX, ..ResourceSample::default() }; sample_count];
        let mut fixture = Fixture::new().await;
        let mut memory = sysinfo::System::new();
        for _ in 0..warmup {
            retention_action(&mut fixture, &mode, &expected).await;
        }
        fixture.fence().await;
        for _ in 0..20 {
            std::hint::black_box(fixture.sample(0, &mut memory));
        }
        samples[0] = fixture.sample(0, &mut memory);
        let cpu = process_cpu_ns();
        let wall = std::time::Instant::now();
        for cycle in 1..=cycles {
            let started = std::time::Instant::now();
            retention_action(&mut fixture, &mode, &expected).await;
            latencies[cycle - 1] = started.elapsed().as_nanos() as u64;
            if cycle.is_multiple_of(100) {
                fixture.fence().await;
                if sample_every != 0 && cycle.is_multiple_of(sample_every) {
                    samples[cycle / sample_every] = fixture.sample(cycle, &mut memory);
                }
            }
        }
        if sample_every == 0 {
            samples[1] = fixture.sample(cycles, &mut memory);
        }
        let wall_ns = wall.elapsed().as_nanos() as u64;
        let cpu_ns = process_cpu_ns() - cpu;
        fixture.assert_only_control().await;
        assert!(samples.iter().all(|sample| sample.cycle != usize::MAX));
        assert!(latencies.iter().all(|&latency| latency != u64::MAX));
        println!(
            "SSH_RETENTION_RESULT {}",
            serde_json::json!({
                "mode":mode,"warmup":warmup,"cycles":cycles,"sampleEvery":sample_every,
                "samplerWarmup":20,"preTouchedMeasurementStorage":true,
                "cpuNs":cpu_ns,"wallNs":wall_ns,"latencyNs":latencies,"samples":samples,
                "callerFailures":if mode == "host" || mode == "sftp" { warmup + cycles } else { 0 },
                "scope":"Current-best production, owned persistent SSH session; diagnostic controls are different workloads, not performance improvements. Fence cadence100 remains fixed even when resource sampling is sparse."
            })
        );
        fixture.stop().await;
        println!("SSH_RETENTION_END {{\"ownedSocketsReleased\":true}}");
    })
    .await
    .expect("owned SSH retention deadline");
}

async fn denied_open(fixture: &Fixture, mode: &str, expected: &str) {
    let error = if mode == "host" {
        match fixture.manager.open_host_exec(&fixture.session, ":").await {
            Ok(_) => panic!("fixture session must reject before exec"),
            Err(error) => error,
        }
    } else {
        match fixture.manager.open_sftp_raw(&fixture.session).await {
            Ok(_) => panic!("fixture session must reject before subsystem setup"),
            Err(error) => error,
        }
    };
    assert_eq!(error, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual owned Host/SFTP rejected-open CPU and memory measurement"]
async fn performance_owned_rejected_opens() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mode = std::env::var("YUZORA_039_MODE").unwrap_or_else(|_| "host".into());
        assert!(["host", "sftp"].contains(&mode.as_str()));
        const WARMUP: usize = 100;
        const CYCLES: usize = 1000;
        let expected = russh::Error::ChannelOpenFailure(
            russh::ChannelOpenFailure::AdministrativelyProhibited,
        )
        .to_string();
        let mut latencies = vec![0_u64; CYCLES];
        let mut samples = vec![ResourceSample::default(); 11];
        let mut fixture = Fixture::new().await;
        let mut memory = sysinfo::System::new();
        for _ in 0..WARMUP {
            denied_open(&fixture, &mode, &expected).await;
        }
        fixture.fence().await;
        samples[0] = fixture.sample(0, &mut memory);
        let cpu = process_cpu_ns();
        let wall = std::time::Instant::now();
        for cycle in 1..=CYCLES {
            let started = std::time::Instant::now();
            denied_open(&fixture, &mode, &expected).await;
            latencies[cycle - 1] = started.elapsed().as_nanos() as u64;
            if cycle % 100 == 0 {
                fixture.fence().await;
                samples[cycle / 100] = fixture.sample(cycle, &mut memory);
            }
        }
        let wall_ns = wall.elapsed().as_nanos() as u64;
        let cpu_ns = process_cpu_ns() - cpu;
        fixture.assert_only_control().await;
        println!(
            "SSH_REJECTED_RESULT {}",
            serde_json::json!({
                "mode": mode, "warmup": WARMUP, "cycles": CYCLES,
                "cpuNs": cpu_ns, "wallNs": wall_ns, "latencyNs": latencies,
                "samples": samples, "callerFailures": WARMUP + CYCLES,
                "scope": "Actual SshManager Host/SFTP opens, persistent owned SSH session. Server rejects before any exec/subsystem. CPU/RSS/FDs include owned server/backend; healthy TCP control retained."
            })
        );
        fixture.stop().await;
        println!("SSH_REJECTED_END {{\"ownedSocketsReleased\":true}}");
    })
    .await
    .expect("owned rejected-open deadline");
}
