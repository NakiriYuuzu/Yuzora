use super::classify_bytes;
use crate::files::WorkspaceFiles;
use serde_json::json;

// Frozen pre-optimization implementation, independent of the production detector.
#[path = "tests/legacy.rs"]
mod legacy;

#[test]
fn classification_matches_legacy_for_exhaustive_delimiters() {
    let alphabet = ["x", "\r", "\n", "中😀"];
    for length in 0..=6 {
        for mut code in 0..4usize.pow(length) {
            let mut text = String::new();
            for _ in 0..length {
                text.push_str(alphabet[code % 4]);
                code /= 4;
            }
            assert_eq!(
                classify_bytes(text.as_bytes()),
                legacy::classify_bytes(text.as_bytes()),
                "{text:?}"
            );
        }
    }
}

#[test]
fn classification_preserves_encoding_and_size_contracts() {
    for (bytes, kind, encoding) in [
        (&b""[..], "full", None),
        (&b"\xef\xbb\xbfhello\r\n"[..], "full", None),
        (
            &b"\xff\xfeh\0i\0\r\0\n\0"[..],
            "nonUtf8Readonly",
            Some("UTF-16LE"),
        ),
        (
            &b"\xfe\xff\0h\0i\0\r\0\n"[..],
            "nonUtf8Readonly",
            Some("UTF-16BE"),
        ),
        (&b"hello\xff\n"[..], "nonUtf8Readonly", Some("unknown")),
        (&b"%PDF-1.7"[..], "binary", None),
        (&b"hi\0there"[..], "binary", None),
    ] {
        let actual = classify_bytes(bytes);
        assert_eq!(actual, legacy::classify_bytes(bytes));
        assert_eq!(actual["kind"], kind);
        assert_eq!(actual["encoding"].as_str(), encoding);
        assert_eq!(actual["size"], bytes.len());
    }
    for extra in [0, 1] {
        let bytes = vec![b'x'; crate::file_content::FULL_FEATURE_MAX_BYTES as usize + extra];
        let actual = classify_bytes(&bytes);
        assert_eq!(actual, legacy::classify_bytes(&bytes));
        assert_eq!(actual["kind"], if extra == 0 { "full" } else { "limited" });
    }
}

#[test]
fn owned_reads_preserve_metadata_revision_and_capability_limit() {
    let root = tempfile::tempdir().unwrap();
    let mut files = WorkspaceFiles::default();
    let opened = files.open(root.path().to_str().unwrap()).unwrap();
    let id = opened["capabilityId"].as_str().unwrap();
    for bytes in [
        &b"hello\r\nworld\n"[..],
        &b"\xff\xfeh\0i\0"[..],
        &b"hello\xff"[..],
        &b"hi\0there"[..],
    ] {
        std::fs::write(root.path().join("owned.txt"), bytes).unwrap();
        let read = files.read(id, "owned.txt").unwrap();
        assert_eq!(read["file"], legacy::classify_bytes(bytes));
        assert!(read["revision"].as_str().is_some());
        assert_eq!(read, files.read(id, "owned.txt").unwrap());
    }
    let large = std::fs::File::create(root.path().join("large.txt")).unwrap();
    large.set_len(crate::protocol::MAX_FILE_BYTES + 1).unwrap();
    assert_eq!(
        files.read(id, "large.txt").unwrap(),
        json!({"file":{"kind":"tooLarge","size":crate::protocol::MAX_FILE_BYTES + 1},"revision":null})
    );
    files.close(id);
    assert!(files.canonical_root(id).is_err());
    assert_eq!(
        files.read(id, "owned.txt").unwrap_err(),
        "workspace-capability-missing"
    );
}

#[cfg(target_os = "macos")]
mod probe {
    use super::*;
    use serde_json::Value;
    use std::time::Instant;

    fn payload(size: usize, flavor: &str) -> Vec<u8> {
        let pattern: &[u8] = match flavor {
            "none" => b"abcdefghijklmnopqrstuvwxyz0123456789",
            "lf" | "mixed-early" | "mixed-late" | "bare-cr" => {
                b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnop\n"
            }
            "crlf" => b"abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnop\r\n",
            "dense-crlf" => b"\r\n",
            "unicode-lf" => "中文😀abcdefghijklmnopqrstuvwxyz\n".as_bytes(),
            "unicode-crlf" => "中文😀abcdefghijklmnopqrstuvwxyz\r\n".as_bytes(),
            "binary" | "invalid" => b"abcdefghijklmnopqrstuvwxyz",
            "utf16le" => b"x\0\r\0\n\0",
            "utf16be" => b"\0x\0\r\0\n",
            _ => panic!("unknown flavor"),
        };
        let mut bytes = Vec::with_capacity(size);
        if size >= 2 && flavor.starts_with("utf16") {
            bytes.extend_from_slice(if flavor == "utf16le" {
                &[0xff, 0xfe]
            } else {
                &[0xfe, 0xff]
            });
        }
        while bytes.len() + pattern.len() <= size {
            bytes.extend_from_slice(pattern);
        }
        bytes.resize(size, b'x');
        if size >= 2 {
            match flavor {
                "mixed-early" => bytes[..2].copy_from_slice(b"\r\n"),
                "mixed-late" => bytes[size - 2..].copy_from_slice(b"\r\n"),
                "bare-cr" => bytes[size - 1] = b'\r',
                "binary" => bytes[0] = 0,
                "invalid" => bytes[size - 1] = 0xff,
                _ => (),
            }
        }
        assert_eq!(bytes.len(), size);
        bytes
    }

    #[derive(Clone, Copy, Default)]
    struct Usage {
        cpu_us: f64,
        rss: u64,
        footprint: u64,
        peak_rss: i64,
    }

    fn usage() -> Usage {
        // SAFETY: records have the libc-prescribed size and are read only on success.
        let (cpu, task) = unsafe {
            let mut cpu = std::mem::MaybeUninit::<libc::rusage>::uninit();
            assert_eq!(libc::getrusage(libc::RUSAGE_SELF, cpu.as_mut_ptr()), 0);
            let mut task = std::mem::MaybeUninit::<libc::rusage_info_v0>::uninit();
            assert_eq!(
                libc::proc_pid_rusage(
                    std::process::id() as libc::c_int,
                    libc::RUSAGE_INFO_V0,
                    task.as_mut_ptr().cast()
                ),
                0
            );
            (cpu.assume_init(), task.assume_init())
        };
        Usage {
            cpu_us: (cpu.ru_utime.tv_sec + cpu.ru_stime.tv_sec) as f64 * 1_000_000.0
                + (cpu.ru_utime.tv_usec + cpu.ru_stime.tv_usec) as f64,
            rss: task.ri_resident_size,
            footprint: task.ri_phys_footprint,
            peak_rss: cpu.ru_maxrss,
        }
    }

    fn descriptors() -> usize {
        std::fs::read_dir("/dev/fd").unwrap().count()
    }

    fn setting(name: &str, default: usize) -> usize {
        let value = std::env::var(name)
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(default);
        assert!((1..=20_000).contains(&value));
        value
    }

    #[test]
    #[ignore = "manual release-profile owned content/read measurement"]
    fn content_read_probe() {
        assert!(!cfg!(debug_assertions));
        let mode = std::env::var("YUZORA_CONTENT_MODE").unwrap();
        let size = std::env::var("YUZORA_CONTENT_SIZE")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let flavor = std::env::var("YUZORA_CONTENT_FLAVOR").unwrap();
        assert!(size as u64 <= crate::protocol::MAX_FILE_BYTES);
        let bytes = payload(size, &flavor);
        // Oracle runs before timing for both roles. Its transient peak is part of process RSS.
        let expected = legacy::classify_bytes(&bytes);
        let root = tempfile::Builder::new()
            .prefix("yuzora-content-probe-044-")
            .tempdir()
            .unwrap();
        std::fs::write(root.path().join("owned.txt"), &bytes).unwrap();
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        let call = || -> Value {
            if mode == "classify" {
                classify_bytes(&bytes)
            } else {
                files.read(id, "owned.txt").unwrap()
            }
        };
        let first_before = usage();
        let start = Instant::now();
        let first = call();
        let first_wall_us = start.elapsed().as_secs_f64() * 1_000_000.0;
        let first_cpu_us = usage().cpu_us - first_before.cpu_us;
        assert_eq!(
            if mode == "classify" {
                &first
            } else {
                &first["file"]
            },
            &expected
        );
        drop(first);
        if mode == "classify" || mode == "read" {
            let warm = setting("YUZORA_CONTENT_WARM", 20);
            let samples = setting(
                "YUZORA_CONTENT_SAMPLES",
                if size <= 65536 { 1000 } else { 100 },
            );
            for _ in 0..warm {
                std::hint::black_box(call());
            }
            let mut timings = vec![0.0; samples];
            let before = usage();
            for timing in &mut timings {
                let start = Instant::now();
                let result = call();
                *timing = start.elapsed().as_secs_f64() * 1_000_000.0;
                std::hint::black_box(&result);
                drop(result);
            }
            let after = usage();
            let active_fds = descriptors();
            files.close(id);
            assert!(files.canonical_root(id).is_err());
            let closed_fds = descriptors();
            println!(
                "CONTENT_TIMING {}",
                json!({"mode":mode,"size":size,"flavor":flavor,"warm":warm,"samples":samples,"samplesUs":timings,"cpuUs":after.cpu_us-before.cpu_us,"firstWallUs":first_wall_us,"firstCpuUs":first_cpu_us,"rssBefore":before.rss,"rssAfter":after.rss,"footprintBefore":before.footprint,"footprintAfter":after.footprint,"peakRssBytes":after.peak_rss,"activeFds":active_fds,"closedFds":closed_fds})
            );
        } else {
            assert_eq!(mode, "lifecycle");
            files.close(id);
            let cycle = |files: &mut WorkspaceFiles| {
                let opened = files.open(root.path().to_str().unwrap()).unwrap();
                let id = opened["capabilityId"].as_str().unwrap();
                let result = files.read(id, "owned.txt").unwrap();
                assert_eq!(result["file"]["size"], size);
                std::hint::black_box(&result);
                drop(result);
                files.close(id);
                assert!(files.canonical_root(id).is_err());
            };
            let warm = setting("YUZORA_CONTENT_WARM", 100);
            let cycles = setting("YUZORA_CONTENT_CYCLES", 1000);
            assert_eq!(cycles % 50, 0);
            for _ in 0..warm {
                cycle(&mut files);
            }
            let mut points = vec![(0, Usage::default(), 0); cycles / 50 + 1];
            points[0] = (0, usage(), descriptors());
            let before = usage();
            for i in 1..=cycles {
                cycle(&mut files);
                if i % 50 == 0 {
                    points[i / 50] = (i, usage(), descriptors());
                }
            }
            let after = usage();
            assert!(points.iter().all(|p| p.2 == points[0].2));
            let points = points.into_iter().map(|(cycle,u,fds)| json!({"cycle":cycle,"rss":u.rss,"footprint":u.footprint,"peakRssBytes":u.peak_rss,"fds":fds})).collect::<Vec<_>>();
            println!(
                "CONTENT_LIFECYCLE {}",
                json!({"mode":mode,"size":size,"flavor":flavor,"warm":warm,"cycles":cycles,"cpuUs":after.cpu_us-before.cpu_us,"points":points,"sameProcess":true,"noForcedGc":true,"capabilityReuseRejected":true})
            );
        }
    }
}
