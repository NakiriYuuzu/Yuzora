#![cfg(target_os = "macos")]

use super::*;

fn owned_child_descriptors(fixture: &Fixture) -> usize {
    let children = fixture.probe.children.lock().unwrap().clone();
    assert_eq!(children.len(), 1, "one cached owned SFTP child");
    let pid = children.iter().next().unwrap().to_string();
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &pid, "-Ff"])
        .output()
        .unwrap();
    assert!(output.status.success(), "owned child lsof failed");
    let text = String::from_utf8(output.stdout).unwrap();
    let count = text
        .lines()
        .filter(|line| {
            line.strip_prefix('f')
                .is_some_and(|fd| fd.parse::<u32>().is_ok())
        })
        .count();
    assert!(count >= 3, "child stdin/stdout/stderr must be visible");
    count
}

async fn cancel_revision_open(fixture: &Fixture, before_promote: bool) -> serde_json::Value {
    let id = fixture.manager.transfers.reserve(&fixture.session).unwrap();
    // The third OPEN is the second revision read, after the upload scratch.
    let (entered, release) = fixture.probe.arm(OPEN, if before_promote { 2 } else { 0 });
    let target = fixture.remote.join("upload.bin");
    let key = path_capability::remote_dest_key(&fixture.session, target.to_str().unwrap());
    let request = SftpUploadRequest {
        transfer_id: id.clone(),
        source: SftpUploadSource::Selected {
            capability_id: fixture
                .selected
                .grant(fixture.local.join("upload.bin").to_str().unwrap())
                .unwrap(),
        },
        remote_dir: fixture.remote.to_string_lossy().into_owned(),
        expected_revision: Some(fixture.expected.clone()),
    };
    let operation = async {
        let result = fixture
            .manager
            .sftp_upload(
                &|_: u64, _: u64, done: bool| assert!(!done, "cancelled upload must not promote"),
                &fixture.selected,
                &fixture.workspaces,
                &fixture.trust,
                &fixture.session,
                request,
            )
            .await;
        (std::time::Instant::now(), result)
    };
    tokio::pin!(operation);
    let early = tokio::select! {
        signal = entered => { signal.unwrap(); None },
        completed = &mut operation => Some(completed),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("revision OPEN gate not reached"),
    };
    if let Some((_, result)) = early {
        // Resource exhaustion in the baseline is an outcome, not a sample to
        // hide by reconnecting. Clear only this unused test packet gate.
        fixture.probe.gate.lock().unwrap().take();
        assert!(fixture.manager.transfer_dests.acquire(key).is_ok());
        assert_eq!(fixture.scratch_count(), 0);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep-remote");
        return serde_json::json!({
            "stage": if before_promote { "before-promote" } else { "before-upload" },
            "outcome": "failed-before-gate", "error": result.unwrap_err(),
            "openCloseRequestImbalance": fixture.probe.count(OPEN) - fixture.probe.count(CLOSE),
        });
    }
    assert!(fixture.manager.transfer_dests.acquire(key.clone()).is_err());
    let started = std::time::Instant::now();
    fixture
        .manager
        .transfers
        .cancel(&fixture.session, &id)
        .unwrap();
    let release_gate = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.send(()).unwrap();
    };
    let ((finished, result), ()) = tokio::join!(&mut operation, release_gate);
    assert_eq!(result.unwrap_err(), "sftp-transfer-cancelled");
    fixture.barrier().await;
    // Give a late OPEN owner time to queue CLOSE, then drain that request too.
    tokio::time::sleep(Duration::from_millis(10)).await;
    fixture.barrier().await;
    // The replacement-extension channel is also fixture-owned and must close.
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.probe.children.lock().unwrap().len() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(fixture.manager.transfer_dests.acquire(key).is_ok());
    assert_eq!(fixture.scratch_count(), 0);
    assert_eq!(std::fs::read(&target).unwrap(), b"keep-remote");
    assert_eq!(
        std::fs::read(fixture.local.join("upload.bin")).unwrap(),
        b"upload-fixture"
    );
    serde_json::json!({
        "stage": if before_promote { "before-promote" } else { "before-upload" },
        "outcome": "cancelled",
        "cancelReplyMs": finished.duration_since(started).as_secs_f64() * 1000.0,
        "openCloseRequestImbalance": fixture.probe.count(OPEN) - fixture.probe.count(CLOSE),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned macOS OpenSSH lifetime probe; requires YUZORA_SFTP_SERVER and lsof"]
async fn performance_revision_open_cancellation() {
    let smoke = std::env::var_os("YUZORA_SFTP_REVISION_SMOKE").is_some();
    assert!(
        smoke || !cfg!(debug_assertions),
        "Use release for measurements"
    );
    let fixture = Fixture::new(false).await;
    let initial_children = fixture.probe.children.lock().unwrap().clone();
    let mut memory = sysinfo::System::new();
    println!(
        "SFTP_REVISION_INITIAL {}",
        serde_json::json!({
            "childFds": owned_child_descriptors(&fixture), "resources": fixture.resources(&mut memory),
        })
    );
    for before_promote in [false, true] {
        for cycle in 0..if smoke { 3 } else { 110 } {
            let mut sample = cancel_revision_open(&fixture, before_promote).await;
            sample["cycle"] = cycle.into();
            sample["warmup"] = (cycle < 10).into();
            println!("SFTP_REVISION_CANCEL {sample}");
            assert_eq!(
                *fixture.probe.children.lock().unwrap(),
                initial_children,
                "cached child must not restart"
            );
            if smoke || (cycle + 1) % 10 == 0 {
                println!(
                    "SFTP_REVISION_RESOURCES {}",
                    serde_json::json!({
                        "beforePromote": before_promote, "cycle": cycle,
                        "childFds": owned_child_descriptors(&fixture), "resources": fixture.resources(&mut memory),
                    })
                );
            }
        }
    }
    fixture.close().await;

    // Separate fresh-session controls measure the normal path even when the
    // preceding baseline lifetime test has exhausted its server handles.
    let fixture = Fixture::new(false).await;
    println!(
        "SFTP_REVISION_HEALTHY_INITIAL {}",
        serde_json::json!({
            "childFds": owned_child_descriptors(&fixture), "resources": fixture.resources(&mut memory),
        })
    );
    for upload in [false, true] {
        for size in [0, 4096, 262144] {
            let bytes = vec![b'x'; size];
            for batch in 0..if smoke { 1 } else { 12 } {
                let count = if smoke { 1 } else { 32 };
                let cpu = parent_cpu_ms();
                let mut latency = Vec::new();
                for _ in 0..count {
                    latency.push(fixture.healthy_transfer(upload, &bytes).await);
                }
                println!(
                    "SFTP_REVISION_HEALTHY {}",
                    serde_json::json!({
                        "upload": upload, "bytes": size, "batch": batch, "warmup": batch < 5,
                        "iterations": count, "parentCpuMs": parent_cpu_ms() - cpu,
                        "methodLatencyMs": latency,
                    })
                );
            }
            println!(
                "SFTP_REVISION_HEALTHY_RESOURCES {}",
                serde_json::json!({
                    "upload": upload, "bytes": size,
                    "childFds": owned_child_descriptors(&fixture), "resources": fixture.resources(&mut memory),
                })
            );
        }
    }
    fixture.close().await;
}

async fn wait_for_late_handle_cleanup(fixture: &Fixture) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            fixture.barrier().await;
            if fixture.probe.count(OPEN) == fixture.probe.count(CLOSE) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("late revision OPEN must acquire a cleanup owner");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH handle guard; requires YUZORA_SFTP_SERVER and macOS lsof"]
async fn cancelled_upload_revision_closes_late_handle() {
    let fixture = Fixture::new(false).await;
    fixture.barrier().await;
    let initial_fds = owned_child_descriptors(&fixture);
    for before_promote in [false, true] {
        let sample = cancel_revision_open(&fixture, before_promote).await;
        assert_eq!(sample["outcome"], "cancelled");
        wait_for_late_handle_cleanup(&fixture).await;
        assert_eq!(owned_child_descriptors(&fixture), initial_fds);
    }
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "owned OpenSSH future-drop guard; requires YUZORA_SFTP_SERVER and macOS lsof"]
async fn dropped_revision_future_closes_late_handle() {
    let fixture = Fixture::new(false).await;
    fixture.barrier().await;
    let initial_fds = owned_child_descriptors(&fixture);
    let sftp = fixture.manager.ensure_sftp(&fixture.session).await.unwrap();
    let path = fixture.remote.join("upload.bin");
    let (entered, release) = fixture.probe.arm(OPEN, 0);
    {
        let revision = crate::sftp_edit::remote_revision(&sftp, path.to_str().unwrap());
        tokio::pin!(revision);
        tokio::select! {
            signal = entered => signal.unwrap(),
            _ = &mut revision => panic!("revision completed before OPEN gate"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("revision gate not reached"),
        }
    }
    // No transfer token is involved: dropping the entire revision is also safe.
    release.send(()).unwrap();
    wait_for_late_handle_cleanup(&fixture).await;
    assert_eq!(owned_child_descriptors(&fixture), initial_fds);
    assert_eq!(std::fs::read(&path).unwrap(), b"keep-remote");
    drop(sftp);
    fixture.close().await;
}
