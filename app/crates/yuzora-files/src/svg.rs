//! A repo's SVGs, for the image pane and the Markdown preview: rasterized in
//! a child process, never parsed here. The child is this same executable
//! started with [`CHILD_ARG`] (the app's `main` hands it to [`child_main`]):
//! it reads the SVG from stdin, draws it with GPUI's own `SvgRenderer` (usvg
//! and resvg, GPUI's fonts, and an `<image>` that loads nothing: vendored
//! gpui-pre) and writes the pixels to stdout. Whatever a file makes usvg and
//! resvg do (nest without end, refer in loops, ask for gigabytes of layers)
//! happens in the child, under limits set from here:
//!
//! - [`TIMEOUT`] of wall-clock time;
//! - [`MEMORY_LIMIT`]: macOS polls the child's `phys_footprint` and kills it
//!   over the limit, Linux sets `RLIMIT_AS` before it starts, Windows puts it
//!   in a Job Object with a process memory limit;
//! - no more read from it than the header and the `width × height × 4` bytes
//!   it names, at most [`MAX_EDGE`] a side;
//! - [`CONCURRENT`] children at once, the rest wait their turn; a request
//!   cancelled (its pane closed) gives up its turn or kills its child.
//!
//! A child that runs out of time or memory, crashes or answers with anything
//! else is killed with what it started (its process group, or its Job
//! Object), and the pane shows a placeholder and why. Results are kept by the
//! SVG's content: the same SVG is drawn once, and one that failed isn't tried
//! again until it changes.
//!
//! This process only weighs the bytes ([`MAX_SVG_BYTES`]) and refuses gzip
//! (SVGZ): usvg would inflate it whole first.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::hash::BuildHasher as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use gpui::RenderImage;

mod child;
mod process;
mod wire;

pub use child::child_main;

/// The argument that starts this executable as the child.
pub const CHILD_ARG: &str = "--yuzora-rasterize-svg";
/// Largest SVG drawn (the image pane's limit).
pub const MAX_SVG_BYTES: u64 = crate::image::MAX_IMAGE_BYTES;
/// Longest side of the pixels the child sends, at GPUI's twice-the-size.
pub const MAX_EDGE: u32 = 4096;
/// Wall-clock time a child has, from its start.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Memory a child may take.
pub const MEMORY_LIMIT: u64 = 512 << 20;
/// Children drawing at once.
pub const CONCURRENT: usize = 2;
/// Drawn SVGs kept for the next ask (bytes of pixels), and results in all.
const KEEP_BYTES: u64 = 128 << 20;
const KEEP_RESULTS: usize = 512;
/// How often a request waiting its turn looks at its cancel.
const TICK: Duration = Duration::from_millis(10);

/// A request cancelled: not shown, not kept.
pub const CANCELLED: &str = "cancelled";
/// Why an SVG isn't shown, each with its own `image.*` text.
pub const REFUSALS: [&str; 7] = [
    "svgz",
    "svg_invalid",
    "svg_timeout",
    "svg_memory",
    "svg_crashed",
    "svg_output",
    "svg_spawn",
];
const SVGZ: &str = REFUSALS[0];
const INVALID: &str = REFUSALS[1];
const TIMED_OUT: &str = REFUSALS[2];
const OUT_OF_MEMORY: &str = REFUSALS[3];
const CRASHED: &str = REFUSALS[4];
const BAD_OUTPUT: &str = REFUSALS[5];
const NOT_STARTED: &str = REFUSALS[6];
const TOO_LARGE: &str = "too_large";

/// Rasterizes a repo's SVG (`path` names the file, for its extension) in a
/// child. Blocks until done, cancelled or failed. Err: [`CANCELLED`], or a key
/// under `image.*` the panes show.
pub fn rasterize(path: &str, svg: &[u8], cancel: &Cancel) -> Result<Arc<RenderImage>, String> {
    let ext = path.rsplit_once('.').map(|(_, ext)| ext);
    if ext.is_some_and(|ext| ext.eq_ignore_ascii_case("svgz")) {
        return Err(SVGZ.into());
    }
    broker().draw(svg, cancel)
}

/// Stops a request: one waiting its turn gives up, a child drawing it is
/// killed. A pane cancels its requests when it closes or asks for another.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a child is held to.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub memory: u64,
    pub max_edge: u32,
    pub concurrent: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: TIMEOUT,
            memory: MEMORY_LIMIT,
            max_edge: MAX_EDGE,
            concurrent: CONCURRENT,
        }
    }
}

/// Starts the children, takes turns, and keeps what they drew.
pub struct Broker {
    program: PathBuf,
    args: Vec<OsString>,
    limits: Limits,
    hasher: std::hash::RandomState,
    /// Turns free, and a wake for those waiting on one.
    free: Mutex<usize>,
    freed: Condvar,
    results: Mutex<Results>,
    /// A result settled (one being drawn by another request may be done).
    settled: Condvar,
    spawned: AtomicUsize,
    running: AtomicUsize,
    peak: AtomicUsize,
}

/// How many children a broker started, and most at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub spawned: usize,
    pub peak: usize,
}

enum Slot {
    /// A request is drawing it; the others with the same SVG wait.
    Drawing,
    Drawn(Arc<RenderImage>),
    Failed(&'static str),
}

#[derive(Default)]
struct Results {
    slots: HashMap<u64, Slot>,
    /// Settled slots, oldest first: the first to go.
    order: VecDeque<u64>,
    bytes: u64,
}

fn image_bytes(image: &RenderImage) -> u64 {
    image.as_bytes(0).map_or(0, |b| b.len() as u64)
}

impl Results {
    fn keep(&mut self, key: u64, slot: Slot) {
        if let Slot::Drawn(image) = &slot {
            self.bytes += image_bytes(image);
        }
        self.slots.insert(key, slot);
        self.order.push_back(key);
        while self.bytes > KEEP_BYTES || self.order.len() > KEEP_RESULTS {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(Slot::Drawn(image)) = self.slots.remove(&old) {
                self.bytes -= image_bytes(&image);
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Broker {
    /// Children run as `program args…`, answering as [`child_main`] does.
    pub fn new(program: PathBuf, args: Vec<OsString>, limits: Limits) -> Self {
        Self {
            program,
            args,
            limits,
            hasher: std::hash::RandomState::new(),
            free: Mutex::new(limits.concurrent.max(1)),
            freed: Condvar::new(),
            results: Mutex::default(),
            settled: Condvar::new(),
            spawned: AtomicUsize::new(0),
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            spawned: self.spawned.load(Ordering::SeqCst),
            peak: self.peak.load(Ordering::SeqCst),
        }
    }

    /// `svg` drawn by a child, or kept from before. Blocks.
    pub fn draw(&self, svg: &[u8], cancel: &Cancel) -> Result<Arc<RenderImage>, String> {
        if svg.len() as u64 > MAX_SVG_BYTES {
            return Err(TOO_LARGE.into());
        }
        if svg.starts_with(&[0x1f, 0x8b]) {
            return Err(SVGZ.into());
        }
        // Keyed per process (the hasher's keys are random): no file can be
        // made to stand for another.
        let key = self.hasher.hash_one(svg);
        let mut claim = loop {
            let mut results = lock(&self.results);
            match results.slots.get(&key) {
                Some(Slot::Drawn(image)) => return Ok(image.clone()),
                Some(Slot::Failed(why)) => return Err((*why).into()),
                Some(Slot::Drawing) => {
                    if cancel.is_cancelled() {
                        return Err(CANCELLED.into());
                    }
                    drop(self.settled.wait_timeout(results, TICK));
                }
                None => {
                    results.slots.insert(key, Slot::Drawing);
                    break Claim {
                        broker: self,
                        key,
                        done: false,
                    };
                }
            }
        };
        let drawn = self.run(svg, cancel);
        claim.settle(&drawn);
        drawn.map_err(String::from)
    }

    fn run(&self, svg: &[u8], cancel: &Cancel) -> Result<Arc<RenderImage>, &'static str> {
        let _turn = self.turn(cancel)?;
        self.spawned.fetch_add(1, Ordering::SeqCst);
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        let raster = process::run(&self.program, &self.args, &self.limits, svg, cancel);
        self.running.fetch_sub(1, Ordering::SeqCst);
        raster.map(wire::Raster::into_image)
    }

    /// Waits for one of the [`Limits::concurrent`] turns.
    fn turn(&self, cancel: &Cancel) -> Result<Turn<'_>, &'static str> {
        let mut free = lock(&self.free);
        loop {
            if cancel.is_cancelled() {
                return Err(CANCELLED);
            }
            if *free > 0 {
                *free -= 1;
                return Ok(Turn(self));
            }
            free = self
                .freed
                .wait_timeout(free, TICK)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

struct Turn<'a>(&'a Broker);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *lock(&self.0.free) += 1;
        self.0.freed.notify_one();
    }
}

/// The request drawing an SVG: the others wait for what it settles. Dropped
/// unsettled (a panic), the next one draws it.
struct Claim<'a> {
    broker: &'a Broker,
    key: u64,
    done: bool,
}

impl Claim<'_> {
    fn settle(&mut self, drawn: &Result<Arc<RenderImage>, &'static str>) {
        let mut results = lock(&self.broker.results);
        match drawn {
            Ok(image) => results.keep(self.key, Slot::Drawn(image.clone())),
            // Nothing about the SVG: the next ask tries.
            Err(why) if *why == CANCELLED || *why == NOT_STARTED => {
                results.slots.remove(&self.key);
            }
            Err(why) => results.keep(self.key, Slot::Failed(why)),
        }
        self.done = true;
        self.broker.settled.notify_all();
    }
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if !self.done {
            lock(&self.broker.results).slots.remove(&self.key);
            self.broker.settled.notify_all();
        }
    }
}

/// The app's children: this executable, as [`child_main`].
fn broker() -> &'static Broker {
    static BROKER: OnceLock<Broker> = OnceLock::new();
    BROKER.get_or_init(|| {
        let (program, args) = child_command();
        Broker::new(program, args, Limits::default())
    })
}

#[cfg(not(test))]
fn child_command() -> (PathBuf, Vec<OsString>) {
    // None: nothing starts, and every SVG says so.
    let program = std::env::current_exe().unwrap_or_default();
    (program, vec![CHILD_ARG.into()])
}

/// What a pane shows in place of an SVG that isn't drawn: a frame, crossed
/// out, made here (not from an SVG) at GPUI's SVG scale.
pub fn placeholder() -> Arc<RenderImage> {
    static PLACEHOLDER: OnceLock<Arc<RenderImage>> = OnceLock::new();
    PLACEHOLDER
        .get_or_init(|| {
            let (w, h) = (96i64, 72i64);
            // Within a pixel of a diagonal: |x·h − y·w| over its length.
            let reach = (w * w + h * h).isqrt();
            let mut pixels = ::image::RgbaImage::new(w as u32, h as u32);
            for (x, y, px) in pixels.enumerate_pixels_mut() {
                let (x, y) = (i64::from(x), i64::from(y));
                let frame = x < 2 || y < 2 || x >= w - 2 || y >= h - 2;
                let cross = (x * h - y * w).abs() <= reach || ((w - x) * h - y * w).abs() <= reach;
                if frame || cross {
                    *px = ::image::Rgba([128, 128, 128, 160]);
                }
            }
            Arc::new(
                RenderImage::new(vec![::image::Frame::new(pixels)])
                    .with_scale_factor(gpui::SMOOTH_SVG_SCALE_FACTOR),
            )
        })
        .clone()
}

/// How many SVGs this process drew itself: none, but in the child.
#[doc(hidden)]
pub fn drawn_here() -> usize {
    child::drawn_here()
}

/// A unit test binary can't be the child (its harness writes to stdout
/// first): `sh` answers every SVG with one green pixel, unread. The real
/// child is tested in `tests/svg_child.rs`.
#[cfg(test)]
fn child_command() -> (PathBuf, Vec<OsString>) {
    let answer =
        r"printf 'YZSVGPX1\000\000\000\000\001\000\000\000\001\000\000\000\000\377\000\377'";
    ("/bin/sh".into(), vec!["-c".into(), answer.into()])
}

/// The pixel the unit tests' child answers with (BGRA, as GPUI holds it).
#[cfg(test)]
pub(crate) const TEST_CHILD_PIXEL: [u8; 4] = [0, 255, 0, 255];

/// SVGs for tests, the image pane's too.
#[cfg(test)]
pub(crate) mod samples {
    /// A 4×4 SVG holding `inner`.
    pub fn svg_with(inner: &str) -> String {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">{inner}</svg>"#)
    }

    /// A 4×4 SVG showing the image at `href`.
    pub fn image_at(href: &str) -> String {
        svg_with(&format!(r#"<image href="{href}" width="4" height="4"/>"#))
    }

    /// An SVG in a data URI, percent-encoded (base64 would grow a third with
    /// each level).
    pub fn svg_data(svg: &str) -> String {
        let svg = svg
            .replace('%', "%25")
            .replace('<', "%3C")
            .replace('"', "%22")
            .replace('#', "%23");
        format!("data:image/svg+xml,{svg}")
    }

    /// `svg` in `<image>`s `levels` deep, each inside the next.
    pub fn nested_in_images(svg: String, levels: usize) -> String {
        (0..levels).fold(svg, |inner, _| image_at(&svg_data(&inner)))
    }

    /// `bytes` gzipped.
    pub fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(bytes).unwrap();
        gz.finish().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::samples::{gzip, svg_with};
    use super::*;

    const RED_RECT: &str = r#"<rect width="4" height="4" fill="red"/>"#;

    #[test]
    fn gzip_and_svgz_are_refused_unread() {
        let gz = gzip(svg_with(RED_RECT).as_bytes());
        let cancel = Cancel::default();
        assert_eq!(rasterize("a.svg", &gz, &cancel).unwrap_err(), "svgz");
        // By its name, whatever it holds.
        let plain = svg_with(RED_RECT);
        assert_eq!(
            rasterize("a.SVGZ", plain.as_bytes(), &cancel).unwrap_err(),
            "svgz"
        );
        let big = vec![b' '; MAX_SVG_BYTES as usize + 1];
        assert_eq!(rasterize("a.svg", &big, &cancel).unwrap_err(), "too_large");
    }

    #[cfg(unix)]
    #[test]
    fn a_repo_svg_is_drawn_by_the_child() {
        // Red here; the test child answers green.
        let svg = svg_with(r#"<rect width="4" height="4" fill="red"/><!-- by the child -->"#);
        let image = rasterize("a.svg", svg.as_bytes(), &Cancel::default()).unwrap();
        assert_eq!(image.as_bytes(0).unwrap(), TEST_CHILD_PIXEL);
        assert_eq!(drawn_here(), 0);
    }

    #[test]
    fn a_cancelled_request_starts_nothing() {
        let broker = Broker::new("/nonexistent".into(), Vec::new(), Limits::default());
        let cancel = Cancel::default();
        cancel.cancel();
        assert_eq!(broker.draw(b"<svg/>", &cancel).unwrap_err(), CANCELLED);
        assert_eq!(broker.stats().spawned, 0);
    }

    #[test]
    fn a_child_that_cannot_start_is_tried_again() {
        let broker = Broker::new("/nonexistent/child".into(), Vec::new(), Limits::default());
        let cancel = Cancel::default();
        assert_eq!(broker.draw(b"<svg/>", &cancel).unwrap_err(), "svg_spawn");
        assert_eq!(broker.draw(b"<svg/>", &cancel).unwrap_err(), "svg_spawn");
        assert_eq!(broker.stats().spawned, 2);
    }

    #[test]
    fn kept_results_stay_within_their_bytes_and_count() {
        let image = |side: u32| {
            Arc::new(RenderImage::new(vec![::image::Frame::new(
                ::image::RgbaImage::new(side, side),
            )]))
        };
        let mut results = Results::default();
        // 48 MiB each: the third pushes the first out.
        for key in 0..3 {
            results.keep(key, Slot::Drawn(image(3584)));
        }
        assert!(results.bytes <= KEEP_BYTES);
        assert!(!results.slots.contains_key(&0) && results.slots.contains_key(&2));
        for key in 10..10 + KEEP_RESULTS as u64 {
            results.keep(key, Slot::Failed(CRASHED));
        }
        assert_eq!(results.order.len(), KEEP_RESULTS);
        assert_eq!(results.slots.len(), KEEP_RESULTS);
    }

    #[test]
    fn the_placeholder_is_drawn_at_svg_scale() {
        let image = placeholder();
        assert_eq!(
            image.size(0),
            gpui::size(gpui::DevicePixels(96), gpui::DevicePixels(72))
        );
        assert!(Arc::ptr_eq(&image, &placeholder()));
    }

    /// The code that runs in this process (each image file's code before its
    /// tests, comments left out), for what it names.
    fn parent_code() -> Vec<(&'static str, String)> {
        let files = [
            ("lib.rs", include_str!("lib.rs")),
            ("image.rs", include_str!("image.rs")),
            ("markdown.rs", include_str!("markdown.rs")),
            ("svg.rs", include_str!("svg.rs")),
            ("svg/process.rs", include_str!("svg/process.rs")),
            ("svg/wire.rs", include_str!("svg/wire.rs")),
        ];
        files
            .into_iter()
            .map(|(name, text)| {
                let code = text.split("#[cfg(test)]").next().unwrap_or_default();
                let code: Vec<&str> = code
                    .lines()
                    .map(|l| l.split("//").next().unwrap_or_default())
                    .collect();
                (name, code.join("\n"))
            })
            .collect()
    }

    #[test]
    fn only_the_child_parses_an_svg() {
        for (name, code) in parent_code() {
            assert!(code.len() > 500, "{name}: nothing read");
            for word in [
                "usvg",
                "resvg",
                "roxmltree",
                "simplecss",
                "SvgRenderer",
                "svg_renderer",
                "parse_svg",
                "render_parsed",
                "render_single_frame",
                "rasterize_here",
            ] {
                assert!(!code.contains(word), "{name} names {word}");
            }
        }
        // Nor can it: no SVG parser among this crate's dependencies.
        let manifest = include_str!("../Cargo.toml");
        let deps = manifest.split("[dev-dependencies]").next().unwrap();
        for parser in ["usvg", "resvg", "roxmltree", "simplecss"] {
            assert!(
                !deps.lines().any(|l| l.trim_start().starts_with(parser)),
                "{parser} is a dependency"
            );
        }
    }
}
