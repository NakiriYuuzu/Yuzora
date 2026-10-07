//! Owned SQLite resource probe using the existing debug-only production-driver seam.
//! cargo run --locked --manifest-path src-tauri/host/Cargo.toml --example performance_sqlite_lifecycle
//! Add -- --soak for 1000 warmup +1000 measured cycles in the same runtime.
//! Timings are diagnostic only: this is not a release-build throughput benchmark.
#[cfg(debug_assertions)]
#[tokio::main(flavor = "current_thread")]
async fn main() {
    use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System};
    use yuzora_host::db_service::{
        integration_harness::IntegrationRuntime, DbValue, ResultPageDirection,
        ResultSessionLifecycle, StatementExecutionResult,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("owned.sqlite");
    {
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch("CREATE TABLE fixture (id INTEGER PRIMARY KEY)")
            .unwrap();
    }
    let runtime = IntegrationRuntime::default();
    let mut system = System::new();
    let pid = get_current_pid().unwrap();
    let soak = std::env::args().any(|arg| arg == "--soak");
    let warmup = if soak { 1000 } else { 100 };
    let measured = if soak { 1000 } else { 100 };
    println!(
        "{}",
        serde_json::json!({"kind":"environment","profile":"debug","platform":std::env::consts::OS,"warmupCycles":warmup,"measuredCycles":measured,"note":"Existing debug-only real-driver seam. Owned SQLite file; no user data, network, Git, forced collection or runtime reset. Timings are not release-performance claims."})
    );
    for cycle in 0..warmup + measured {
        let started = std::time::Instant::now();
        let connection = runtime
            .open_sqlite("lifecycle", path.to_str().unwrap())
            .await
            .unwrap();
        let run = connection.run_primary(
            format!("cycle-{cycle:04}"),
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<501) SELECT x, 'fixture' FROM n",
        ).await.unwrap();
        let StatementExecutionResult::Rows {
            result_session: Some(session),
            ..
        } = &run.statements[0].result
        else {
            panic!("expected a row result: {:?}", run.statements[0].result);
        };
        assert_eq!(session.initial_page.rows.len(), 500);
        assert!(session.initial_page.has_next);
        assert_eq!(
            session.initial_page.rows[0][0],
            DbValue::Integer { value: "1".into() }
        );
        let owner = session.owner.clone();
        let consume_last_page = cycle % 2 == 1;
        if consume_last_page {
            let last = connection
                .result_page(owner.clone(), ResultPageDirection::Next)
                .await
                .unwrap();
            assert_eq!(last.rows.len(), 1);
            assert!(!last.has_next);
            assert_eq!(
                last.rows[0][0],
                DbValue::Integer {
                    value: "501".into()
                }
            );
        }
        let released = connection.release_result(owner).await.unwrap();
        assert_eq!(released.lifecycle, ResultSessionLifecycle::Released);
        assert!(!released.has_next);
        assert_eq!(released.rows.len(), if consume_last_page { 1 } else { 500 });
        drop(released);
        drop(run);
        let report = connection.close().unwrap();
        assert!(report.closed);
        assert!(!connection.is_registered());
        drop(connection);
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let descriptor_path = if cfg!(target_os = "linux") {
            "/proc/self/fd"
        } else {
            "/dev/fd"
        };
        let descriptors = std::fs::read_dir(descriptor_path)
            .ok()
            .map(|entries| entries.count());
        println!(
            "{}",
            serde_json::json!({"kind":"lifecycle","cycle":cycle,"warmup":cycle<warmup,"consumedLastPage":consume_last_page,"wallMs":wall_ms,"rssBytes":system.process(pid).map(|p|p.memory()),"descriptorsAfterClose":descriptors,"connectionRegistered":false})
        );
    }
}

#[cfg(not(debug_assertions))]
fn main() {
    eprintln!("Run without --release: the existing real-driver integration seam is debug-only.");
    std::process::exit(2);
}
