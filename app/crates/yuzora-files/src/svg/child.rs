//! The child: one SVG from stdin, its pixels to stdout, then it ends. The
//! only place in the app that parses a repo's SVG.

use std::io::{Read as _, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::{RenderImage, SMOOTH_SVG_SCALE_FACTOR, SvgRenderer, SvgSize};

use super::wire;

/// The drawing thread's stack: usvg and resvg recurse a level at a time.
const STACK: usize = 16 << 20;

static DRAWN: AtomicUsize = AtomicUsize::new(0);

pub(super) fn drawn_here() -> usize {
    DRAWN.load(Ordering::SeqCst)
}

/// Runs the child ([`super::CHILD_ARG`]) and returns its exit code: 0 once it
/// answered, 2 for a request that isn't one, 3 when the answer can't be
/// written. A panic, a stack overflow or memory running out ends it as a
/// crash.
pub fn child_main() -> i32 {
    // Its parent kills it in time; one whose parent is gone ends itself.
    #[cfg(unix)]
    unsafe {
        libc::alarm(2 * super::TIMEOUT.as_secs() as u32);
    }
    let mut stdin = std::io::stdin().lock();
    let mut head = [0; wire::REQUEST_HEAD];
    if stdin.read_exact(&mut head).is_err() {
        return 2;
    }
    let Some((max_edge, len)) = wire::read_request_head(&head) else {
        return 2;
    };
    let mut svg = vec![0; len];
    if stdin.read_exact(&mut svg).is_err() {
        return 2;
    }
    drop(stdin);
    let drawing = std::thread::Builder::new()
        .name("yuzora-svg".into())
        .stack_size(STACK)
        .spawn(move || rasterize_here(&svg, max_edge));
    let drawn = match drawing.map(std::thread::JoinHandle::join) {
        Ok(Ok(drawn)) => drawn,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => return 3,
    };
    let mut out = std::io::stdout().lock();
    let written = match drawn {
        Some(image) => write_image(&mut out, &image),
        None => out.write_all(&wire::answer_head(wire::UNREADABLE, 0, 0)),
    };
    if written.and_then(|()| out.flush()).is_err() {
        return 3;
    }
    0
}

/// `svg` as GPUI draws it (at twice its size), its longest side at most
/// `max_edge`; `None` when usvg can't read or draw it.
fn rasterize_here(svg: &[u8], max_edge: u32) -> Option<Arc<RenderImage>> {
    DRAWN.fetch_add(1, Ordering::SeqCst);
    // No asset source: GPUI's own (the app's folder) holds no fonts it looks
    // for, so the system's are used either way.
    let renderer = SvgRenderer::new(Arc::new(()));
    let parsed = renderer.parse_svg(svg).ok()?;
    let size = parsed.tree().size();
    let longest = size.width().max(size.height());
    let scale = (max_edge as f32 / (SMOOTH_SVG_SCALE_FACTOR * longest)).min(1.);
    renderer
        .render_parsed(&parsed, SvgSize::ScaleFactor(scale))
        .ok()
}

/// The answer for a drawn image: its BGRA turned back into RGBA, a piece at a
/// time.
fn write_image(out: &mut impl std::io::Write, image: &RenderImage) -> std::io::Result<()> {
    let size = image.size(0);
    let (width, height) = (i32::from(size.width) as u32, i32::from(size.height) as u32);
    out.write_all(&wire::answer_head(wire::DRAWN, width, height))?;
    let mut piece = vec![0; 1 << 16];
    for bgra in image.as_bytes(0).unwrap_or_default().chunks(1 << 16) {
        let piece = &mut piece[..bgra.len()];
        piece.copy_from_slice(bgra);
        for px in piece.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        out.write_all(piece)?;
    }
    Ok(())
}
