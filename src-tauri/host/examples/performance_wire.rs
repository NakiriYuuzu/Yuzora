//! Isolated native frame-reader benchmark; no helper, HERDR or user workspace.
//! cargo run --locked --release --manifest-path src-tauri/host/Cargo.toml --example performance_wire
//! Add -- --lifecycle to measure 10 warmup +100 local socket open/read/close cycles.
//! --soak extends this to 100 warmup +1000 cycles; --sampler-only skips socket work.
use std::hint::black_box;
use std::time::Instant;
use tokio::io::BufReader;
use yuzora_host::wire::read_frame;

#[cfg(unix)]
fn process_usage() -> (Option<f64>, Option<i64>) {
    let usage = unsafe {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()), 0);
        usage.assume_init()
    };
    let cpu_ms = (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.0
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.0;
    (Some(cpu_ms), Some(usage.ru_maxrss))
}

#[cfg(not(unix))]
fn process_usage() -> (Option<f64>, Option<i64>) {
    (None, None)
}

#[cfg(unix)]
fn descriptor_count() -> Option<usize> {
    let path = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    std::fs::read_dir(path).ok().map(|entries| entries.count())
}

fn frame_bytes(size: usize, unicode: bool) -> Vec<u8> {
    let mut bytes = b"{\"data\":\"".to_vec();
    if unicode {
        while bytes.len() + "資料😀".len() + 3 <= size {
            bytes.extend_from_slice("資料😀".as_bytes());
        }
    }
    bytes.resize(size - 3, b'x');
    bytes.extend_from_slice(b"\"}\n");
    assert_eq!(bytes.len(), size);
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
    bytes
}

async fn buffered_samples() {
    for (name, size, chunk, unicode) in [
        ("small-32", 32, 8192, false),
        ("reply-128", 128, 8192, false),
        ("event-512", 512, 8192, false),
        ("frame-8k", 8192, 8192, false),
        ("frame-64k", 65536, 8192, false),
        ("frame-1m", 1048576, 8192, false),
        ("unicode-8k", 8192, 8192, true),
        ("fragmented-8k", 8192, 31, false),
    ] {
        let frame = frame_bytes(size, unicode);
        let frames = (64 * 1024 * 1024 / size).clamp(64, 400_000);
        let input = frame.repeat(frames);
        for pass in 0..8 {
            let mut reader = BufReader::with_capacity(chunk, input.as_slice());
            let (cpu_before, _) = process_usage();
            let started = Instant::now();
            let mut max_capacity = 0;
            for _ in 0..frames {
                let result = read_frame(&mut reader).await.unwrap().unwrap();
                assert_eq!(result.len(), frame.len());
                assert_eq!(result.last(), Some(&b'\n'));
                max_capacity = max_capacity.max(result.capacity());
                if pass == 0 {
                    assert_eq!(result, frame);
                }
                black_box(&result);
            }
            assert!(read_frame(&mut reader).await.unwrap().is_none());
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (cpu_after, peak_rss) = process_usage();
            println!(
                "{}",
                serde_json::json!({
                    "kind":"sample", "scenario":name, "pass":pass, "warmup":pass==0,
                    "frameBytes":size, "bufferBytes":chunk, "frames":frames,
                    "wallMs":wall_ms, "wallNsPerFrame":wall_ms*1e6/frames as f64,
                    "cpuMs":cpu_after.zip(cpu_before).map(|(after,before)|after-before),
                    "maxFrameCapacity":max_capacity, "peakRss":peak_rss
                })
            );
        }
    }
}

#[cfg(unix)]
async fn socket_lifecycle() {
    use sysinfo::{get_current_pid, ProcessRefreshKind, ProcessesToUpdate, System};
    use tokio::io::AsyncWriteExt;
    let pid = get_current_pid().unwrap();
    let mut system = System::new();
    let frame = frame_bytes(16384, true);
    let soak = std::env::args().any(|arg| arg == "--soak");
    let sampler_only = std::env::args().any(|arg| arg == "--sampler-only");
    let warmup = if soak { 100 } else { 10 };
    let measured = if soak { 1000 } else { 100 };
    for cycle in 0..warmup + measured {
        let started = Instant::now();
        if !sampler_only {
            let (mut sender, receiver) = tokio::net::UnixStream::pair().unwrap();
            let expected = frame.clone();
            let write_task = tokio::spawn(async move {
                for _ in 0..8 {
                    sender.write_all(&expected).await.unwrap();
                }
                sender.shutdown().await.unwrap();
            });
            let mut reader = BufReader::new(receiver);
            for _ in 0..8 {
                assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), frame);
            }
            assert!(read_frame(&mut reader).await.unwrap().is_none());
            write_task.await.unwrap();
            drop(reader);
        }
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        println!(
            "{}",
            serde_json::json!({
                "kind":"lifecycle", "cycle":cycle, "warmup":cycle<warmup,
                "samplerOnly":sampler_only, "frames":if sampler_only {0}else{8},
                "wallMs":wall_ms, "rssBytes":system.process(pid).map(|p|p.memory()),
                "descriptorsAfterClose":descriptor_count()
            })
        );
    }
}

fn main() {
    if cfg!(debug_assertions) {
        panic!("Run with --release");
    }
    let lifecycle = std::env::args().any(|arg| arg == "--lifecycle");
    println!(
        "{}",
        serde_json::json!({
            "kind":"environment", "profile":"release", "platform":std::env::consts::OS,
            "lifecycle":lifecycle, "warmSamples":7,
            "peakRssUnits":if cfg!(target_os="macos"){"bytes"}else{"KiB on Linux; platform-dependent otherwise"},
            "note":"Buffered samples isolate the production decoder. Local socket cycles include transport. No OS cold-cache claim, forced GC, user data or existing server."
        })
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    if lifecycle {
        #[cfg(unix)]
        runtime.block_on(socket_lifecycle());
        #[cfg(not(unix))]
        panic!("Socket lifecycle mode requires Unix");
    } else {
        runtime.block_on(buffered_samples());
    }
}
