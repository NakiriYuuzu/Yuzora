use super::*;

async fn drain_owned_subsystems(fixture: &Fixture) {
    fixture.barrier().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.probe.children.lock().unwrap().len() != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    tokio::task::yield_now().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH lifecycle diagnosis; requires YUZORA_SFTP_SERVER"]
async fn performance_sftp_session_lifecycle() {
    let smoke = std::env::var_os("YUZORA_SFTP_SESSION_SMOKE").is_some();
    let mode = std::env::var("YUZORA_SFTP_SESSION_MODE").unwrap_or_else(|_| "upload".into());
    assert!(matches!(mode.as_str(), "upload" | "raw" | "download"));
    let fixture = Fixture::new(false).await;
    let cached_child = fixture.probe.children.lock().unwrap().clone();
    let mut memory = sysinfo::System::new();
    let bytes = vec![b'x'; 4096];
    drain_owned_subsystems(&fixture).await;
    println!("SFTP_SESSION_INITIAL {}", fixture.resources(&mut memory));
    let mut batch_cpu = parent_cpu_ms();
    let mut batch_started = std::time::Instant::now();
    let mut latency = Vec::new();
    for cycle in 0..if smoke { 120 } else { 1100 } {
        let started = std::time::Instant::now();
        match mode.as_str() {
            "upload" => {
                fixture.healthy_transfer(true, &bytes).await;
            }
            "download" => {
                fixture.healthy_transfer(false, &bytes).await;
            }
            "raw" => {
                let raw = fixture
                    .manager
                    .open_sftp_raw(&fixture.session)
                    .await
                    .unwrap();
                raw.init().await.unwrap();
                raw.close_session().unwrap();
            }
            _ => unreachable!(),
        }
        drain_owned_subsystems(&fixture).await;
        latency.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(*fixture.probe.children.lock().unwrap(), cached_child);
        if (cycle + 1) % if smoke { 10 } else { 100 } == 0 {
            println!(
                "SFTP_SESSION_SAMPLE {}",
                serde_json::json!({
                    "mode": mode, "cycle": cycle + 1, "warmup": if smoke {20} else {100},
                    "batchCpuMs": parent_cpu_ms() - batch_cpu,
                    "batchWallMs": batch_started.elapsed().as_secs_f64() * 1000.0,
                    "operationAndDrainLatencyMs": latency,
                    "resources": fixture.resources(&mut memory),
                })
            );
            latency.clear();
            batch_cpu = parent_cpu_ms();
            batch_started = std::time::Instant::now();
        }
    }
    // Same connection, no GC or restart: distinguish delayed reclamation from
    // state that remains while the SSH session is still in normal use.
    tokio::time::sleep(Duration::from_millis(250)).await;
    drain_owned_subsystems(&fixture).await;
    println!("SFTP_SESSION_DRAINED {}", fixture.resources(&mut memory));
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH close handshake guard; requires YUZORA_SFTP_SERVER"]
async fn raw_session_eof_completes_peer_close() {
    let fixture = Fixture::new(false).await;
    for _ in 0..3 {
        let raw = fixture
            .manager
            .open_sftp_raw(&fixture.session)
            .await
            .unwrap();
        raw.init().await.unwrap();
        raw.close_session().unwrap();
        drain_owned_subsystems(&fixture).await;
    }
    assert_eq!(fixture.probe.channels_opened.load(Ordering::Relaxed), 4);
    assert_eq!(fixture.probe.channel_eofs.load(Ordering::Relaxed), 3);
    assert_eq!(fixture.probe.channel_closes.load(Ordering::Relaxed), 3);
    fixture.close().await;
}
