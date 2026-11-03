//! The SVG child, run for real. This binary is its own child, as the app's
//! executable is ([`CHILD_ARG`]), and plays children that misbehave
//! (`--misbehave <how>`): the parent has to kill each and say why, holding
//! no more than the limits allow. No libtest harness: it would write to
//! stdout before a child could answer there.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use yuzora_files::svg::{
    Broker, CANCELLED, CHILD_ARG, CONCURRENT, Cancel, Limits, MAX_EDGE, MEMORY_LIMIT, Stats,
    TIMEOUT,
};

const MISBEHAVE: &str = "--misbehave";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some(CHILD_ARG) => std::process::exit(yuzora_files::svg::child_main()),
        Some(MISBEHAVE) => misbehave(&args[2..]),
        _ => run(&args[1..]),
    }
}

// Children that misbehave. What they answer is the wire format, written out
// here: `YZSVGPX1`, status, width, height (u32, little-endian), RGBA.

fn answer_head(status: u32, width: u32, height: u32) -> Vec<u8> {
    let mut head = b"YZSVGPX1".to_vec();
    for n in [status, width, height] {
        head.extend(n.to_le_bytes());
    }
    head
}

fn write_pid(path: Option<&String>, pid: u32) {
    if let Some(path) = path {
        std::fs::write(path, pid.to_string()).unwrap();
    }
}

fn misbehave(args: &[String]) -> ! {
    let mut out = std::io::stdout().lock();
    let rest = || std::thread::sleep(Duration::from_secs(30));
    match args.first().map(String::as_str).unwrap_or_default() {
        // Never answers.
        "sleep" => {
            write_pid(args.get(1), std::process::id());
            rest();
        }
        // Answers one pixel, late.
        "slow" => {
            let ms = args.get(1).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(ms));
            out.write_all(&answer_head(0, 1, 1)).unwrap();
            out.write_all(&[0, 0, 255, 255]).unwrap();
        }
        // Takes memory, 16 MiB at a time and touched, up to 1.5 GiB.
        "alloc" => {
            let mut held = Vec::new();
            for _ in 0..96 {
                held.push(vec![1u8; 16 << 20]);
            }
            std::hint::black_box(&held);
            rest();
        }
        "crash" => std::process::abort(),
        // One 64×64 image, then on and on (up to 256 MiB).
        "flood" => {
            let _ = out.write_all(&answer_head(0, 64, 64));
            let _ = out.write_all(&[0; 64 * 64 * 4]);
            let more = vec![0u8; 1 << 16];
            for _ in 0..(256 << 20) / more.len() {
                if out.write_all(&more).is_err() {
                    break;
                }
            }
            rest();
        }
        // An image larger than any allowed, then nothing.
        "huge" => {
            let _ = out.write_all(&answer_head(0, 100_000, 100_000));
            let _ = out.flush();
            rest();
        }
        "garbage" => out.write_all(b"not an answer, just text").unwrap(),
        // Half the pixels it names.
        "short" => {
            out.write_all(&answer_head(0, 2, 2)).unwrap();
            out.write_all(&[0; 8]).unwrap();
        }
        // Starts a process of its own, then sleeps or crashes.
        "tree" => {
            // Never waited for: the parent kills it with this one.
            #[expect(clippy::zombie_processes)]
            let grandchild = std::process::Command::new(std::env::current_exe().unwrap())
                .args([MISBEHAVE, "sleep"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            write_pid(args.get(1), grandchild.id());
            if args.get(2).is_some_and(|end| end == "crash") {
                std::process::abort();
            }
            rest();
        }
        how => panic!("misbehave how? {how}"),
    }
    let _ = out.flush();
    std::process::exit(0);
}

// The parent's side.

fn broker(args: &[&str], limits: Limits) -> Broker {
    let exe = std::env::current_exe().unwrap();
    Broker::new(exe, args.iter().map(OsString::from).collect(), limits)
}

/// The real child.
fn child(limits: Limits) -> Broker {
    broker(&[CHILD_ARG], limits)
}

fn limits(timeout: u64) -> Limits {
    Limits {
        timeout: Duration::from_secs(timeout),
        ..Limits::default()
    }
}

fn draw(broker: &Broker, svg: &str) -> Result<Arc<gpui::RenderImage>, String> {
    broker.draw(svg.as_bytes(), &Cancel::default())
}

fn svg(width: u32, height: u32, inner: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">{inner}</svg>"#
    )
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("yuzora-svg-child-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn pid_in(path: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid in {}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether `pid` runs (a zombie doesn't).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        let state = stat.rsplit(')').next().unwrap_or_default().trim_start();
        return !state.starts_with('Z');
    }
    true
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

fn gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "{pid} still runs");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The most memory this process has held so far.
#[cfg(target_os = "macos")]
fn peak_memory() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let ok = unsafe {
        libc::proc_pid_rusage(
            std::process::id() as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    (ok == 0).then(|| unsafe { info.assume_init() }.ri_lifetime_max_phys_footprint)
}

#[cfg(target_os = "linux")]
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb << 10)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peak_memory() -> Option<u64> {
    None
}

// The tests.

/// First: the peak is this process's so far.
fn what_a_child_sends_past_its_header_is_not_held() {
    let before = peak_memory();
    let small = Limits {
        max_edge: 64,
        ..limits(10)
    };
    // 16 KiB of pixels, then up to 256 MiB more.
    let start = Instant::now();
    let flood = broker(&[MISBEHAVE, "flood"], small);
    assert_eq!(draw(&flood, "<svg/>").unwrap_err(), "svg_output");
    // Refused at the first byte past it, not at the deadline.
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
    // A header naming 40 GB: refused before anything is held.
    let huge = broker(&[MISBEHAVE, "huge"], small);
    assert_eq!(draw(&huge, "<svg/>").unwrap_err(), "svg_output");
    if let (Some(before), Some(after)) = (before, peak_memory()) {
        let grew = after.saturating_sub(before);
        assert!(grew < 32 << 20, "the parent grew {} MiB", grew >> 20);
    }
}

fn a_plain_svg_draws_as_gpui_draws_it() {
    let here = gpui::SvgRenderer::new(Arc::new(()));
    let broker = child(limits(60));
    let svgs = [
        svg(4, 4, r#"<rect width="4" height="4" fill="red"/>"#),
        svg(
            40,
            30,
            r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#0af"/><stop offset="1" stop-color="#f0a" stop-opacity=".5"/></linearGradient></defs><circle cx="20" cy="15" r="12" fill="url(#g)" stroke="#333" stroke-width="2"/><path d="M2 28 L38 2" stroke="green" stroke-dasharray="3 2"/>"##,
        ),
        svg(
            120,
            32,
            r#"<text x="4" y="22" font-family="sans-serif" font-size="18">Yuzora 檔案</text>"#,
        ),
        svg(
            16,
            16,
            r#"<g opacity=".5"><rect width="16" height="16" fill="blue"/></g>"#,
        ),
    ];
    for svg in &svgs {
        let drawn = draw(&broker, svg).unwrap();
        let expected = here.render_single_frame(svg.as_bytes(), 1.0).unwrap();
        assert_eq!(drawn.size(0), expected.size(0), "{svg}");
        assert!(
            drawn.as_bytes(0) == expected.as_bytes(0),
            "{svg}: other pixels"
        );
    }
    // Larger than the longest side allowed: drawn smaller, as before.
    let small = Limits {
        max_edge: 256,
        ..limits(60)
    };
    let wide = svg(
        200,
        100,
        r#"<rect width="200" height="100" fill="red"/><circle cx="50" cy="50" r="40"/>"#,
    );
    let drawn = draw(&child(small), &wide).unwrap();
    let parsed = here.parse_svg(wide.as_bytes()).unwrap();
    let expected = here
        .render_parsed(&parsed, gpui::SvgSize::ScaleFactor(256. / 400.))
        .unwrap();
    assert_eq!(
        drawn.size(0),
        gpui::size(gpui::DevicePixels(256), gpui::DevicePixels(128))
    );
    assert!(drawn.as_bytes(0) == expected.as_bytes(0), "other pixels");
}

fn what_usvg_cannot_read_is_refused() {
    let broker = child(limits(30));
    for svg in ["not xml at all", "<svg", "<html/>"] {
        assert_eq!(draw(&broker, svg).unwrap_err(), "svg_invalid", "{svg}");
    }
}

fn a_child_past_its_time_is_killed() {
    let pid = temp("sleep.pid");
    let broker = broker(&[MISBEHAVE, "sleep", pid.to_str().unwrap()], limits(2));
    let start = Instant::now();
    assert_eq!(draw(&broker, "<svg/>").unwrap_err(), "svg_timeout");
    let took = start.elapsed();
    assert!(
        (Duration::from_secs(2)..Duration::from_secs(4)).contains(&took),
        "{took:?}"
    );
    gone(pid_in(&pid));
}

fn a_child_past_its_memory_is_killed() {
    let broker = broker(&[MISBEHAVE, "alloc"], limits(15));
    let start = Instant::now();
    assert_eq!(draw(&broker, "<svg/>").unwrap_err(), "svg_memory");
    // Stopped at the limit, not at the deadline.
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}

fn a_child_that_crashes_says_so() {
    let broker = broker(&[MISBEHAVE, "crash"], limits(10));
    assert_eq!(draw(&broker, "<svg/>").unwrap_err(), "svg_crashed");
}

fn answers_that_are_not_answers_are_refused() {
    for how in ["garbage", "short"] {
        let broker = broker(&[MISBEHAVE, how], limits(10));
        assert_eq!(draw(&broker, "<svg/>").unwrap_err(), "svg_output", "{how}");
    }
}

fn what_a_child_starts_goes_with_it() {
    for (end, why) in [("sleep", "svg_timeout"), ("crash", "svg_crashed")] {
        let pid = temp(&format!("tree-{end}.pid"));
        let broker = broker(&[MISBEHAVE, "tree", pid.to_str().unwrap(), end], limits(2));
        assert_eq!(draw(&broker, "<svg/>").unwrap_err(), why);
        gone(pid_in(&pid));
    }
}

fn no_more_children_run_at_once_than_allowed() {
    let broker = broker(&[MISBEHAVE, "slow", "400"], limits(10));
    let broker = Arc::new(broker);
    let start = Instant::now();
    let draws: Vec<_> = (0..6)
        .map(|i| {
            let broker = broker.clone();
            std::thread::spawn(move || draw(&broker, &format!("<svg id='{i}'/>")).map(|_| ()))
        })
        .collect();
    for d in draws {
        d.join().unwrap().unwrap();
    }
    // Two at a time, 400 ms each: three rounds at least.
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(1200), "{took:?}");
    assert_eq!(
        broker.stats(),
        Stats {
            spawned: 6,
            peak: CONCURRENT
        }
    );
}

fn a_cancelled_draw_kills_its_child_or_starts_none() {
    let pid = temp("cancelled.pid");
    let one_at_a_time = Limits {
        concurrent: 1,
        ..limits(30)
    };
    let broker = Arc::new(broker(
        &[MISBEHAVE, "sleep", pid.to_str().unwrap()],
        one_at_a_time,
    ));
    let first = Cancel::default();
    let drawing = {
        let (broker, cancel) = (broker.clone(), first.clone());
        std::thread::spawn(move || broker.draw(b"<svg id='1'/>", &cancel))
    };
    let child = pid_in(&pid);
    // The next waits its turn; cancelled, it gives up without one.
    let second = Cancel::default();
    let waiting = {
        let (broker, cancel) = (broker.clone(), second.clone());
        std::thread::spawn(move || broker.draw(b"<svg id='2'/>", &cancel))
    };
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    second.cancel();
    assert_eq!(waiting.join().unwrap().unwrap_err(), CANCELLED);
    first.cancel();
    assert_eq!(drawing.join().unwrap().unwrap_err(), CANCELLED);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    gone(child);
    assert_eq!(broker.stats().spawned, 1);
}

fn the_same_svg_is_drawn_once() {
    let rect = svg(4, 4, r#"<rect width="4" height="4" fill="teal"/>"#);
    let broker = Arc::new(child(limits(30)));
    let a = draw(&broker, &rect).unwrap();
    let b = draw(&broker, &rect).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(broker.stats().spawned, 1);
    // Asked twice at once: one child.
    let other = svg(4, 4, r#"<rect width="4" height="4" fill="navy"/>"#);
    let both: Vec<_> = (0..2)
        .map(|_| {
            let (broker, other) = (broker.clone(), other.clone());
            std::thread::spawn(move || draw(&broker, &other).unwrap())
        })
        .collect();
    let both: Vec<_> = both.into_iter().map(|t| t.join().unwrap()).collect();
    assert!(Arc::ptr_eq(&both[0], &both[1]));
    assert_eq!(broker.stats().spawned, 2);
    // One that failed isn't tried again.
    let crash = broker_crashing();
    assert_eq!(draw(&crash, "<svg/>").unwrap_err(), "svg_crashed");
    assert_eq!(draw(&crash, "<svg/>").unwrap_err(), "svg_crashed");
    assert_eq!(crash.stats().spawned, 1);
    // Nor gzip, which is refused unstarted.
    let gz = [0x1f, 0x8b, 8, 0];
    assert_eq!(broker.draw(&gz, &Cancel::default()).unwrap_err(), "svgz");
    assert_eq!(broker.stats().spawned, 2);
}

fn broker_crashing() -> Broker {
    broker(&[MISBEHAVE, "crash"], limits(10))
}

fn the_limits_are_the_documented_ones() {
    let l = Limits::default();
    assert_eq!(
        (l.timeout, l.memory, l.max_edge, l.concurrent),
        (TIMEOUT, MEMORY_LIMIT, MAX_EDGE, CONCURRENT)
    );
    assert_eq!(
        (TIMEOUT, MEMORY_LIMIT, MAX_EDGE, CONCURRENT),
        (Duration::from_secs(5), 512 << 20, 4096, 2)
    );
}

/// Last: everything above was drawn in children.
fn this_process_drew_no_svg() {
    assert_eq!(yuzora_files::svg::drawn_here(), 0);
}

const TESTS: &[(&str, fn())] = &[
    (
        "what_a_child_sends_past_its_header_is_not_held",
        what_a_child_sends_past_its_header_is_not_held,
    ),
    (
        "a_plain_svg_draws_as_gpui_draws_it",
        a_plain_svg_draws_as_gpui_draws_it,
    ),
    (
        "what_usvg_cannot_read_is_refused",
        what_usvg_cannot_read_is_refused,
    ),
    (
        "a_child_past_its_time_is_killed",
        a_child_past_its_time_is_killed,
    ),
    (
        "a_child_past_its_memory_is_killed",
        a_child_past_its_memory_is_killed,
    ),
    ("a_child_that_crashes_says_so", a_child_that_crashes_says_so),
    (
        "answers_that_are_not_answers_are_refused",
        answers_that_are_not_answers_are_refused,
    ),
    (
        "what_a_child_starts_goes_with_it",
        what_a_child_starts_goes_with_it,
    ),
    (
        "no_more_children_run_at_once_than_allowed",
        no_more_children_run_at_once_than_allowed,
    ),
    (
        "a_cancelled_draw_kills_its_child_or_starts_none",
        a_cancelled_draw_kills_its_child_or_starts_none,
    ),
    ("the_same_svg_is_drawn_once", the_same_svg_is_drawn_once),
    (
        "the_limits_are_the_documented_ones",
        the_limits_are_the_documented_ones,
    ),
    ("this_process_drew_no_svg", this_process_drew_no_svg),
];

/// libtest's flags (cargo passes some) are taken and ignored, but a bare
/// word filters by name and `--skip` skips, as there.
fn run(args: &[String]) {
    let (mut filters, mut skips) = (Vec::new(), Vec::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => {
                for (name, _) in TESTS {
                    println!("{name}: test");
                }
                return;
            }
            "--skip" => skips.extend(args.next()),
            "--test-threads" | "--color" | "--format" | "--logfile" | "-Z" | "--shuffle-seed" => {
                args.next();
            }
            a if a.starts_with('-') => {}
            a => filters.push(a.to_owned()),
        }
    }
    let chosen: Vec<_> = TESTS
        .iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .filter(|(name, _)| !skips.iter().any(|s| name.contains(s.as_str())))
        .collect();
    println!("\nrunning {} tests", chosen.len());
    let mut failed = Vec::new();
    for (name, test) in &chosen {
        let (tx, rx) = std::sync::mpsc::channel();
        let test = *test;
        std::thread::spawn(move || {
            let _ = tx.send(std::panic::catch_unwind(test).is_ok());
        });
        let ok = match rx.recv_timeout(Duration::from_secs(90)) {
            Ok(ok) => ok,
            Err(_) => {
                println!("test {name} ... FAILED (over 90 s)");
                std::process::exit(101);
            }
        };
        println!("test {name} ... {}", if ok { "ok" } else { "FAILED" });
        if !ok {
            failed.push(name);
        }
    }
    let _ = std::fs::remove_dir_all(temp("x").parent().unwrap());
    if failed.is_empty() {
        println!("\ntest result: ok. {} passed; 0 failed\n", chosen.len());
    } else {
        println!("\nfailures: {failed:?}");
        println!(
            "\ntest result: FAILED. {} passed; {} failed\n",
            chosen.len() - failed.len(),
            failed.len()
        );
        std::process::exit(101);
    }
}
