//! Same-machine local NDJSON benchmark. Only owns its temporary socket and thread.
//! cargo run --locked --release --manifest-path src-tauri/host/Cargo.toml --example performance_local_ndjson
//! Add -- --guards for longer batches of the three small-frame scenarios.
//! --buffered measures the production reader with a complete frame already buffered.

// Compile the private production transport without exposing benchmark-only APIs.
#[allow(dead_code)]
#[path = "../src/herdr_transport.rs"]
mod herdr_transport;
use yuzora_host::herdr_limits;

#[cfg(unix)]
fn main() {
    use herdr_transport::{connect_local_stream, read_local_ndjson_line};
    use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
    use std::hint::black_box;
    use std::io::Write;
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};

    fn usage() -> (f64, i64) {
        let value = unsafe {
            let mut value = std::mem::MaybeUninit::<libc::rusage>::uninit();
            assert_eq!(libc::getrusage(libc::RUSAGE_SELF, value.as_mut_ptr()), 0);
            value.assume_init()
        };
        let cpu_ms = (value.ru_utime.tv_sec + value.ru_stime.tv_sec) as f64 * 1000.0
            + (value.ru_utime.tv_usec + value.ru_stime.tv_usec) as f64 / 1000.0;
        (cpu_ms, value.ru_maxrss)
    }

    if cfg!(debug_assertions) {
        panic!("Run with --release");
    }
    let guards = std::env::args().any(|arg| arg == "--guards");
    let buffered = std::env::args().any(|arg| arg == "--buffered");
    println!(
        "{}",
        serde_json::json!({"kind":"environment","platform":std::env::consts::OS,"profile":"release","warmSamples":7,"buffered":buffered,"guards":guards,"note":"Owned local socket. Buffered mode includes construction of complete pending frames, but no socket I/O in the timed loop. Private production source compiled by path. No cold-cache claim or existing HERDR server."})
    );
    let directory = tempfile::tempdir().unwrap();
    for (name, size, unicode) in [
        ("small-32", 32, false),
        ("event-512", 512, false),
        ("frame-8k", 8192, false),
        ("odd-frame-8k", 8193, false),
        ("frame-64k", 65536, false),
        ("frame-1m", 1048576, false),
        ("at-content-cap", 1048577, false),
        ("unicode-64k", 65536, true),
    ] {
        if guards && size > 8192 {
            continue;
        }
        let mut frame = b"{\"data\":\"".to_vec();
        if unicode {
            while frame.len() + "資料😀".len() + 3 <= size {
                frame.extend_from_slice("資料😀".as_bytes());
            }
        }
        frame.resize(size - 3, b'x');
        frame.extend_from_slice(b"\"}\n");
        serde_json::from_slice::<serde_json::Value>(&frame).unwrap();
        let frames = if guards || buffered {
            (64 * 1024 * 1024 / size).min(524288)
        } else {
            (8 * 1024 * 1024 / size).clamp(8, 8192)
        };
        let batch = Arc::new(if buffered {
            frame.clone()
        } else {
            frame.repeat(frames)
        });
        let path = directory.path().join(format!("{name}.sock"));
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let (send, recv) = mpsc::channel::<()>();
        let bytes = batch.clone();
        let server = std::thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            while recv.recv().is_ok() {
                stream.write_all(&bytes).unwrap();
            }
        });
        let mut stream = connect_local_stream(
            path.to_str().unwrap(),
            Instant::now() + Duration::from_secs(15),
        )
        .unwrap();
        let mut pending = Vec::new();
        for pass in 0..8 {
            let (cpu_before, _) = usage();
            let started = Instant::now();
            let mut max_returned_capacity = 0;
            if !buffered {
                send.send(()).unwrap();
            }
            for _ in 0..frames {
                if buffered {
                    pending = black_box(frame.clone());
                }
                let line = read_local_ndjson_line(
                    &mut stream,
                    &mut pending,
                    Some(Instant::now() + Duration::from_secs(15)),
                    herdr_limits::MAX_NDJSON_LINE_BYTES,
                )
                .unwrap()
                .unwrap();
                assert_eq!(line.len(), size);
                assert_eq!(line.as_bytes().last(), Some(&b'\n'));
                max_returned_capacity = max_returned_capacity.max(line.capacity());
                if pass == 0 {
                    assert_eq!(line.as_bytes(), frame);
                }
                black_box(&line);
            }
            let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
            let (cpu_after, peak_rss) = usage();
            assert!(pending.is_empty());
            println!(
                "{}",
                serde_json::json!({"kind":"sample","scenario":name,"frameBytes":size,"frames":frames,"pass":pass,"warmup":pass==0,"cpuMs":cpu_after-cpu_before,"wallMs":wall_ms,"wallUsPerFrame":wall_ms*1000.0/frames as f64,"pendingBytes":pending.len(),"pendingCapacity":pending.capacity(),"maxReturnedCapacity":max_returned_capacity,"peakRss":peak_rss})
            );
        }
        drop(send);
        server.join().unwrap();
        assert!(read_local_ndjson_line(
            &mut stream,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(15)),
            herdr_limits::MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .is_none());
    }
}

#[cfg(not(unix))]
fn main() {
    panic!("This benchmark requires Unix local sockets");
}
