use super::WorkspaceFiles;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

#[test]
#[ignore = "manual owned bounded-read capacity witness"]
fn bounded_read_capacity_probe() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("owned.txt");
    let mut records = Vec::new();
    for size in [
        0, 1, 16, 31, 32, 33, 4096, 4097, 65536, 65537, 1_048_576, 1_048_577, 8_388_608,
    ] {
        std::fs::write(&path, vec![b'x'; size]).unwrap();
        let before = descriptors();
        let (bytes, metadata, identity) =
            super::read_bounded(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(bytes.len(), size);
        assert!(bytes.iter().all(|&byte| byte == b'x'));
        assert_eq!(metadata.len(), size as u64);
        assert!(!identity.is_empty());
        assert_eq!(descriptors(), before);
        records.push(json!({"size":size,"capacity":bytes.capacity(),"fds":before}));
    }
    let too_large = std::fs::File::create(&path).unwrap();
    too_large.set_len(super::MAX_FILE_BYTES + 1).unwrap();
    assert_eq!(
        super::read_bounded(std::fs::File::open(&path).unwrap()).unwrap_err(),
        "file-too-large"
    );
    println!(
        "READ_CAPACITY {}",
        json!({"records":records,"aboveCapRejected":true,"scope":"Actual returned Vec capacity, not peak allocation, physical RAM or timing"})
    );
}

struct Fixture {
    files: WorkspaceFiles,
    id: String,
    path: String,
    expected: Value,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new(count: usize, flavor: &str) -> Self {
        assert!(count <= 20_000);
        let root = tempfile::Builder::new()
            .prefix("yuzora-list-probe-042-")
            .tempdir()
            .unwrap();
        let path = if flavor == "long-path" {
            (0..8)
                .map(|i| format!("owned-directory-level-{i}"))
                .collect::<Vec<_>>()
                .join("/")
        } else {
            "owned".to_owned()
        };
        let directory = root.path().join(&path);
        std::fs::create_dir_all(&directory).unwrap();
        let target = root.path().join("owned-link-target");
        std::fs::write(&target, "owned").unwrap();
        for i in 0..count {
            let index = i * 37 % count;
            let prefix = ["alpha", "Beta", "gamma", "DELTA"][index % 4];
            let name = if flavor == "files" || flavor == "dirs" {
                format!("entry_{index:05}.txt")
            } else if flavor == "unicode" {
                format!("{prefix}_{index:05}_ΣΟΣ_İ_中文_😀")
            } else {
                format!("{prefix}_{index:05}_entry.txt")
            };
            let entry = directory.join(name);
            if flavor == "dirs" || flavor != "files" && index % 8 == 0 {
                std::fs::create_dir(entry).unwrap();
            } else if flavor != "files" && index % 16 == 7 {
                std::os::unix::fs::symlink(&target, entry).unwrap();
            } else {
                std::fs::File::create(entry).unwrap();
            }
        }
        let expected = expected_listing(&directory, &path);
        assert_eq!(expected.as_array().unwrap().len(), count);
        let mut files = WorkspaceFiles::default();
        let opened = files.open(root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap().to_owned();
        Self {
            files,
            id,
            path,
            expected,
            root,
        }
    }

    fn list(&self) -> Value {
        self.files.list(&self.id, &self.path).unwrap()
    }

    fn cycle(&mut self) {
        let opened = self.files.open(self.root.path().to_str().unwrap()).unwrap();
        let id = opened["capabilityId"].as_str().unwrap();
        let result = self.files.list(id, &self.path).unwrap();
        assert_eq!(
            result.as_array().unwrap().len(),
            self.expected.as_array().unwrap().len()
        );
        std::hint::black_box(&result);
        drop(result);
        self.files.close(id);
        assert!(self.files.roots.is_empty());
    }
}

// Independent filesystem walk supplies the expected schema and stable ordering.
fn expected_listing(directory: &Path, relative: &str) -> Value {
    let mut entries = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let file_type = entry.file_type().unwrap();
            let kind = if file_type.is_symlink() {
                "symlink"
            } else if file_type.is_dir() {
                "directory"
            } else if file_type.is_file() {
                "file"
            } else {
                "other"
            };
            (name, kind)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.sort_by(|a, b| {
        (b.1 == "directory")
            .cmp(&(a.1 == "directory"))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    Value::Array(entries.into_iter().map(|(name, kind)| {
        json!({ "name": name, "path": format!("{relative}/{name}"), "isDir": kind == "directory", "kind": kind })
    }).collect())
}

#[derive(Clone, Copy, Default)]
struct Usage {
    cpu_us: f64,
    rss: u64,
    footprint: u64,
    peak_rss: i64,
}

fn usage() -> Usage {
    // SAFETY: libc receives correctly sized writable records; read only after success.
    let (cpu, task) = unsafe {
        let mut cpu = std::mem::MaybeUninit::<libc::rusage>::uninit();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, cpu.as_mut_ptr()), 0);
        let mut task = std::mem::MaybeUninit::<libc::rusage_info_v0>::uninit();
        assert_eq!(
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V0,
                task.as_mut_ptr().cast(),
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

#[test]
fn listing_preserves_owned_entry_metadata_and_order() {
    for (count, flavor) in [
        (0, "ascii"),
        (1, "ascii"),
        (2, "ascii"),
        (3, "ascii"),
        (64, "ascii"),
        (64, "files"),
        (64, "dirs"),
        (64, "unicode"),
        (64, "long-path"),
    ] {
        let fixture = Fixture::new(count, flavor);
        assert_eq!(fixture.list(), fixture.expected);
        assert!(fixture.files.list(&fixture.id, "../escape").is_err());
    }
}

#[test]
fn listing_close_releases_owned_capability_and_rejects_reuse() {
    let mut fixture = Fixture::new(16, "unicode");
    fixture.files.close(&fixture.id);
    let before = descriptors();
    for _ in 0..100 {
        fixture.cycle();
    }
    // `/dev/fd` counts the whole test process, so tests running alongside shift
    // it by a few descriptors; a leak in the cycle would add at least 100.
    let after = descriptors();
    assert!(
        after <= before + 50,
        "descriptors grew from {before} to {after}"
    );
    assert_eq!(
        fixture.files.list(&fixture.id, &fixture.path).unwrap_err(),
        "workspace-capability-missing"
    );
}

#[test]
#[ignore = "manual release-profile owned native directory listing measurement"]
fn directory_listing_probe() {
    assert!(!cfg!(debug_assertions), "use --release for this probe");
    let mode = std::env::var("YUZORA_LIST_PROBE_MODE").unwrap_or_else(|_| "timing".into());
    let count = std::env::var("YUZORA_LIST_PROBE_COUNT")
        .unwrap_or_else(|_| "1024".into())
        .parse::<usize>()
        .unwrap();
    let flavor = std::env::var("YUZORA_LIST_PROBE_FLAVOR").unwrap_or_else(|_| "ascii".into());
    let mut fixture = Fixture::new(count, &flavor);
    let first_before = usage();
    let first_start = Instant::now();
    let result = fixture.list();
    let first_wall_us = first_start.elapsed().as_secs_f64() * 1_000_000.0;
    let first_cpu_us = usage().cpu_us - first_before.cpu_us;
    assert_eq!(result, fixture.expected);
    drop(result);
    if mode == "timing" || mode == "preflight" {
        let warm = if mode == "preflight" { 5 } else { 20 };
        let samples = if mode == "preflight" { 10 } else { 100 };
        for _ in 0..warm {
            std::hint::black_box(fixture.list());
        }
        let mut timings = vec![0.0; samples];
        let before = usage();
        for sample in &mut timings {
            let start = Instant::now();
            let result = fixture.list();
            *sample = start.elapsed().as_secs_f64() * 1_000_000.0;
            std::hint::black_box(&result);
            drop(result);
        }
        let after = usage();
        let active_fds = descriptors();
        fixture.files.close(&fixture.id);
        let closed_fds = descriptors();
        assert!(fixture.files.roots.is_empty());
        println!(
            "LIST_TIMING {}",
            json!({"mode":mode,"count":count,"flavor":flavor,"warm":warm,"samples":samples,"firstWallUs":first_wall_us,"firstCpuUs":first_cpu_us,"cpuUs":after.cpu_us-before.cpu_us,"samplesUs":timings,"rssBefore":before.rss,"rssAfter":after.rss,"footprintBefore":before.footprint,"footprintAfter":after.footprint,"peakRssBytes":after.peak_rss,"activeFds":active_fds,"closedFds":closed_fds,"registryAfterClose":fixture.files.roots.len(),"scope":"first invocation after fixture creation, not cold filesystem/App startup; peak RSS is whole owned process"})
        );
    } else if mode == "lifecycle" {
        fixture.files.close(&fixture.id);
        let warm = 100;
        let cycles = 1000;
        for _ in 0..warm {
            fixture.cycle();
        }
        let mut points = vec![(0, Usage::default(), 0, 0); 21];
        points[0] = (0, usage(), descriptors(), fixture.files.roots.len());
        let before = usage();
        for i in 1..=cycles {
            fixture.cycle();
            if i % 50 == 0 {
                points[i / 50] = (i, usage(), descriptors(), fixture.files.roots.len());
            }
        }
        let after = usage();
        let points = points.into_iter().map(|(cycle, u, fds, registry)| json!({"cycle":cycle,"rss":u.rss,"footprint":u.footprint,"peakRssBytes":u.peak_rss,"fds":fds,"registry":registry})).collect::<Vec<_>>();
        assert!(points
            .iter()
            .all(|p| p["fds"] == points[0]["fds"] && p["registry"] == 0));
        println!(
            "LIST_LIFECYCLE {}",
            json!({"count":count,"flavor":flavor,"warm":warm,"cycles":cycles,"cpuUs":after.cpu_us-before.cpu_us,"points":points,"noForcedGc":true,"sameProcess":true})
        );
    } else {
        panic!("unknown mode");
    }
}
