//! Release benchmark for the production result registry, with no database or Git I/O.
//! cargo run --locked --release --manifest-path src-tauri/host/Cargo.toml --example performance_result_accounting
use std::hint::black_box;
use std::time::Instant;
use yuzora_host::db_result_session::{PushRowOutcome, ResultSessionRegistry};
use yuzora_host::db_service::{
    ConnectionGeneration, ConnectionId, ConnectionIdentity, DbValue, DescriptorId, EffectOutcome,
    QueryRunId, QueryRunOwner, ResultSessionId, ResultSessionOwner, StatementExecutionId,
};

fn owner(descriptor: usize, pass: usize) -> ResultSessionOwner {
    ResultSessionOwner {
        descriptor_id: DescriptorId(format!("descriptor-{descriptor:04}")),
        connection_id: ConnectionId(format!("connection-{descriptor:04}")),
        connection_generation: ConnectionGeneration("generation-0001".into()),
        query_run_id: QueryRunId(format!("run-{pass:04}")),
        statement_execution_id: StatementExecutionId("statement-0001".into()),
        result_session_id: ResultSessionId(format!("result-{descriptor:04}-{pass:04}")),
    }
}

fn begin(registry: &mut ResultSessionRegistry, owner: &ResultSessionOwner, columns: usize) {
    registry
        .begin_run(&QueryRunOwner {
            descriptor_id: owner.descriptor_id.clone(),
            connection_id: owner.connection_id.clone(),
            connection_generation: owner.connection_generation.clone(),
            query_run_id: owner.query_run_id.clone(),
        })
        .unwrap();
    registry
        .begin_session(
            owner.clone(),
            (0..columns).map(|i| format!("column-{i:03}")).collect(),
        )
        .unwrap();
}

fn row(index: usize, columns: usize, text_bytes: usize) -> Vec<DbValue> {
    (0..columns)
        .map(|column| match column % 4 {
            0 => DbValue::Integer {
                value: index.to_string(),
            },
            1 => DbValue::Text {
                value: "x".repeat(text_bytes),
            },
            2 => DbValue::Null,
            _ => DbValue::Binary {
                hex: "ab".repeat(text_bytes / 2),
            },
        })
        .collect()
}

#[cfg(unix)]
fn usage() -> (Option<f64>, Option<i64>) {
    let usage = unsafe {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()), 0);
        usage.assume_init()
    };
    (
        Some(
            (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
                + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0,
        ),
        Some(usage.ru_maxrss),
    )
}

#[cfg(not(unix))]
fn usage() -> (Option<f64>, Option<i64>) {
    (None, None)
}

fn main() {
    if cfg!(debug_assertions) {
        panic!("Run with --release");
    }
    println!(
        "{}",
        serde_json::json!({"kind":"environment","profile":"release","platform":std::env::consts::OS,"warmSamples":7,"note":"Actual result registry; output and memory accounting are checked. Timings include row construction, page clone and cleanup. No real database, UI or cold-cache claim."})
    );
    for (name, rows, columns, text_bytes, background) in [
        ("small", 32, 4, 16, 0),
        ("single-page", 500, 4, 32, 0),
        ("multi-page", 1500, 4, 32, 0),
        ("large-result", 10000, 4, 32, 0),
        ("wide-page", 500, 64, 32, 0),
        ("text-page", 500, 4, 1024, 0),
        ("eight-background", 500, 4, 32, 8),
        ("twenty-four-background", 500, 4, 32, 24),
    ] {
        let mut registry = ResultSessionRegistry::default();
        for descriptor in 0..background {
            let owner = owner(descriptor, 0);
            begin(&mut registry, &owner, 4);
            for i in 0..500 {
                assert_eq!(
                    registry.push_row(&owner, row(i, 4, 32)).unwrap(),
                    PushRowOutcome::Stored
                );
            }
            registry
                .finish_session(&owner, EffectOutcome::None)
                .unwrap();
        }
        let background_bytes = registry.total_bytes();
        // Repeat small batches to avoid treating sub-millisecond scheduler noise as a gain.
        let batches = if rows == 32 { 100 } else { 1 };
        for pass in 0..8 {
            let (cpu_before, _) = usage();
            let started = Instant::now();
            let mut retained_bytes = 0;
            let mut released_bytes = 0;
            for batch in 0..batches {
                let owner = owner(99, pass * batches + batch);
                begin(&mut registry, &owner, columns);
                for i in 0..rows {
                    assert_eq!(
                        registry
                            .push_row(&owner, row(i, columns, text_bytes))
                            .unwrap(),
                        PushRowOutcome::Stored
                    );
                }
                retained_bytes = registry.total_bytes();
                let result = registry
                    .finish_session(&owner, EffectOutcome::None)
                    .unwrap();
                assert_eq!(result.initial_page.rows.len(), rows.min(500));
                assert_eq!(
                    result.initial_page.rows[0][0],
                    DbValue::Integer { value: "0".into() }
                );
                black_box(&result);
                registry.release(&owner).unwrap();
                released_bytes = registry.total_bytes();
                assert_eq!(
                    released_bytes, retained_bytes,
                    "cursor release preserves loaded pages"
                );
                registry
                    .release_connection(&ConnectionIdentity {
                        descriptor_id: owner.descriptor_id,
                        connection_id: owner.connection_id,
                        connection_generation: owner.connection_generation,
                    })
                    .unwrap();
                assert_eq!(registry.session_count(), background);
                // Removal can change HashMap's reported spare capacity even
                // when the same background sessions remain. Empty is exact.
                if background == 0 {
                    assert_eq!(registry.total_bytes(), 0);
                }
            }
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (cpu_after, peak_rss) = usage();
            println!(
                "{}",
                serde_json::json!({"kind":"sample","scenario":name,"pass":pass,"warmup":pass==0,"rows":rows,"columns":columns,"backgroundSessions":background,"batches":batches,"wallMs":wall_ms,"cpuMs":cpu_after.zip(cpu_before).map(|(a,b)|a-b),"retainedBytes":retained_bytes,"releasedBytes":released_bytes,"afterCloseBytes":registry.total_bytes(),"backgroundBytes":background_bytes,"peakRss":peak_rss})
            );
        }
    }
}
