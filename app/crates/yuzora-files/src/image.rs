//! The image viewer: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO and SVG, here or on
//! another machine. The bytes are read by us (not GPUI's path cache), so a
//! file that changed on disk shows its new content when `rev` changes.

use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, ElementId, Hsla, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};
use yuzora_core::HostValue;

use crate::docs::blocking;
use crate::fsops::{failure, machine, run_sh};
use crate::highlight;
use crate::svg::Cancel;

/// Images above this are not shown.
pub const MAX_IMAGE_BYTES: u64 = 32 << 20;

/// Nor images that take more than this once decoded: GPUI keeps every frame
/// of a GIF or animated WebP, 4 bytes a pixel of the whole canvas.
pub const MAX_DECODED_BYTES: u64 = 512 << 20;

/// Whether an image (all its bytes) decodes within [`MAX_DECODED_BYTES`],
/// by its header and, for GIF and WebP, its frames counted without
/// decoding. Not a raster (SVG) or a header that doesn't parse: left to the
/// decoder.
pub fn fits_in_memory(bytes: &[u8]) -> bool {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format();
    let Ok(reader) = reader else { return true };
    let frames = match reader.format() {
        Some(image::ImageFormat::Gif) => gif_frames(bytes),
        Some(image::ImageFormat::WebP) => webp_frames(bytes),
        _ => 1,
    };
    reader
        .into_dimensions()
        .map_or(true, |(w, h)| within_budget(w, h, frames))
}

/// Whether `frames` of a `width`×`height` canvas, 4 bytes a pixel, fit.
fn within_budget(width: u32, height: u32, frames: u64) -> bool {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        .saturating_mul(frames.max(1))
        <= MAX_DECODED_BYTES
}

/// The images (frames) a GIF holds, from its blocks; as many as are there
/// when it is cut short.
fn gif_frames(b: &[u8]) -> u64 {
    // A color table of 2^(n+1) entries, 3 bytes each.
    let table = |flags: u8| {
        if flags & 0x80 != 0 {
            3usize << ((flags & 7) + 1)
        } else {
            0
        }
    };
    // Data sub-blocks: a length byte and that many bytes, until a 0.
    let blocks = |mut at: usize| -> Option<usize> {
        loop {
            let n = usize::from(*b.get(at)?);
            at += 1 + n;
            if n == 0 {
                return Some(at);
            }
        }
    };
    let Some(&flags) = b.get(10) else { return 0 };
    let mut at = 13 + table(flags);
    let mut frames = 0;
    loop {
        let next = match b.get(at) {
            // An extension: its label, then its sub-blocks.
            Some(0x21) => blocks(at + 2),
            // An image: its descriptor, color table, code size and data.
            Some(0x2C) => {
                frames += 1;
                b.get(at + 9).and_then(|&f| blocks(at + 10 + table(f) + 1))
            }
            _ => None,
        };
        match next {
            Some(n) => at = n,
            None => return frames,
        }
    }
}

/// The frames of a WebP: its `ANMF` chunks (1 when still).
fn webp_frames(b: &[u8]) -> u64 {
    let mut at = 12;
    let mut frames = 0;
    while let Some(head) = b.get(at..at + 8) {
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        frames += u64::from(&head[..4] == b"ANMF");
        at = at
            .saturating_add(8)
            .saturating_add(size)
            .saturating_add(size & 1);
    }
    frames.max(1)
}

/// Reads `$1` on another machine, up to `$2` bytes: a regular file only, as
/// here (a FIFO would wait, a device never ends).
const READ_SCRIPT: &str = r#"[ -e "$1" ] || { echo "no such file: $1" >&2; exit 4; }
[ -f "$1" ] || exit 5
exec head -c "$2" -- "$1""#;

/// The format a file's extension names, if it is an image the viewer shows.
/// The format of an image by its bytes, for a name without a known
/// extension: a raster format the header names, else SVG (as GPUI guesses).
pub(crate) fn format_in(bytes: &[u8]) -> ImageFormat {
    use image::ImageFormat as F;
    match image::guess_format(bytes) {
        Ok(F::Png) => ImageFormat::Png,
        Ok(F::Jpeg) => ImageFormat::Jpeg,
        Ok(F::Gif) => ImageFormat::Gif,
        Ok(F::WebP) => ImageFormat::Webp,
        Ok(F::Bmp) => ImageFormat::Bmp,
        Ok(F::Tiff) => ImageFormat::Tiff,
        Ok(F::Ico) => ImageFormat::Ico,
        _ => ImageFormat::Svg,
    }
}

pub fn format_of(path: &str) -> Option<ImageFormat> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "svg" => ImageFormat::Svg,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        _ => return None,
    })
}

pub(crate) fn read(path: &str, on: Option<&str>) -> Result<Vec<u8>, String> {
    match machine(on) {
        None => {
            // Looked at first for the answer; `open_regular` guards the open itself.
            let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
            if !meta.is_file() {
                return Err("unsupported".into());
            }
            if meta.len() > MAX_IMAGE_BYTES {
                return Err("too_large".into());
            }
            let mut bytes = Vec::new();
            crate::fsops::open_regular(std::path::Path::new(path))
                .and_then(|f| f.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes))
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                return Err("too_large".into());
            }
            Ok(bytes)
        }
        Some(m) => read_sh(path, Some(m)),
    }
}

fn read_sh(path: &str, on: Option<&str>) -> Result<Vec<u8>, String> {
    let upto = (MAX_IMAGE_BYTES + 1).to_string();
    let out = run_sh(
        on,
        READ_SCRIPT,
        &[path, &upto],
        Duration::from_secs(30),
        MAX_IMAGE_BYTES + 1,
    )?;
    match out.code {
        0 => {}
        5 => return Err("unsupported".into()),
        _ => return Err(failure(&out, "reading the image")),
    }
    if out.stdout.len() as u64 > MAX_IMAGE_BYTES {
        return Err("too_large".into());
    }
    Ok(out.stdout)
}

/// An image ready for GPUI: a raster's bytes for it to decode (checked by
/// [`fits_in_memory`]), or an SVG a child rasterized ([`crate::svg`]).
#[derive(Clone)]
pub(crate) enum Picture {
    Bytes(Arc<Image>),
    Drawn(Arc<RenderImage>),
}

impl Picture {
    /// What a repo's image bytes (of the file at `path`) show as: an SVG is
    /// rasterized now, in a child, so this blocks (a worker's call) until it
    /// is drawn, fails or `cancel` stops it.
    pub(crate) fn of(
        format: ImageFormat,
        path: &str,
        bytes: Vec<u8>,
        cancel: &Cancel,
    ) -> Result<Self, String> {
        match format {
            ImageFormat::Svg => crate::svg::rasterize(path, &bytes, cancel).map(Picture::Drawn),
            _ => Ok(Picture::Bytes(Arc::new(Image::from_bytes(format, bytes)))),
        }
    }

    /// The texture GPUI keeps for it once drawn, which only
    /// [`App::drop_image`] lets go of: an SVG's.
    pub(crate) fn drawn(self) -> Option<Arc<RenderImage>> {
        match self {
            Picture::Bytes(_) => None,
            Picture::Drawn(image) => Some(image),
        }
    }

    pub(crate) fn source(&self) -> ImageSource {
        match self {
            Picture::Bytes(image) => ImageSource::Image(image.clone()),
            Picture::Drawn(image) => ImageSource::Render(image.clone()),
        }
    }
}

/// Why an image isn't shown, each with its own `image.*` text (an SVG's are
/// [`crate::svg::REFUSALS`]).
const REFUSALS: [&str; 3] = ["too_large", "too_large_decoded", "unsupported"];

enum Loaded {
    Loading,
    Ready(Picture),
    Failed(String),
}

impl Loaded {
    fn drawn(self) -> Option<Arc<RenderImage>> {
        match self {
            Loaded::Ready(picture) => picture.drawn(),
            _ => None,
        }
    }
}

struct ImageView {
    key: (String, Option<String>, u64),
    loaded: Loaded,
    /// The load under way: cancelled by the next one, or when the pane closes
    /// (an SVG's child is killed).
    cancel: Cancel,
}

impl ImageView {
    fn new(key: (String, Option<String>, u64), cx: &mut Context<Self>) -> Self {
        cx.on_release(|v: &mut ImageView, cx| {
            v.cancel.cancel();
            let loaded = std::mem::replace(&mut v.loaded, Loaded::Loading);
            if let Some(image) = loaded.drawn() {
                cx.drop_image(image, None);
            }
        })
        .detach();
        let mut v = ImageView {
            key,
            loaded: Loaded::Loading,
            cancel: Cancel::default(),
        };
        v.load(cx);
        v
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loaded = Loaded::Loading;
        self.cancel.cancel();
        self.cancel = Cancel::default();
        let cancel = self.cancel.clone();
        let (path, on, _) = self.key.clone();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let Some(format) = format_of(&path) else {
                let _ = this.update(cx, |v, cx| {
                    v.loaded = Loaded::Failed("unsupported".into());
                    cx.notify();
                });
                return;
            };
            let picture = blocking(move || {
                read(&path, on.as_deref()).and_then(|bytes| {
                    if fits_in_memory(&bytes) {
                        Picture::of(format, &path, bytes, &cancel)
                    } else {
                        Err("too_large_decoded".into())
                    }
                })
            })
            .await
            .unwrap_or_else(|| Err("worker".into()));
            let _ = this.update(cx, |v, cx| {
                if v.key != key || picture.as_ref().is_err_and(|e| e == crate::svg::CANCELLED) {
                    return;
                }
                let was = std::mem::replace(
                    &mut v.loaded,
                    match picture {
                        Ok(picture) => Loaded::Ready(picture),
                        Err(e) => Loaded::Failed(e),
                    },
                );
                // An SVG drawn before: its texture goes with it.
                if let Some(image) = was.drawn() {
                    cx.drop_image(image, None);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Contain,
    Actual,
}

struct Look {
    fit: Fit,
    muted: Hsla,
    checker: Option<Hsla>,
}

/// An SVG that isn't drawn: a placeholder, and why.
fn not_drawn(text: String, muted: Hsla) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .size_full()
        .items_center()
        .justify_center()
        .px(px(16.))
        .text_size(px(12.))
        .text_color(muted)
        .child(
            img(ImageSource::Render(crate::svg::placeholder()))
                .w(px(48.))
                .h(px(36.)),
        )
        .child(text)
        .into_any_element()
}

fn status(text: String, muted: Hsla) -> AnyElement {
    div()
        .flex()
        .size_full()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .text_color(muted)
        .child(text)
        .into_any_element()
}

/// The `ImageView` component: `ImageView.new(id, {path, on?, rev?, fit?: "contain" | "actual", colors?})`.
pub fn image_component(
    args: gpui_shell::ComponentArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let props = args.props();
    let path = props
        .get("path")
        .and_then(HostValue::as_str)
        .unwrap_or_default()
        .to_owned();
    let on = props
        .get("on")
        .and_then(HostValue::as_str)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    let rev = props
        .get("rev")
        .and_then(HostValue::as_number)
        .unwrap_or(0.0) as u64;
    let color = |k: &str| {
        props
            .get("colors")
            .and_then(|c| c.get(k))
            .and_then(HostValue::as_str)
            .and_then(highlight::parse_hex)
    };
    let look = Look {
        fit: match props.get("fit").and_then(HostValue::as_str) {
            Some("actual") => Fit::Actual,
            _ => Fit::Contain,
        },
        muted: color("muted").unwrap_or_else(crate::editor::fallback_muted),
        checker: color("checker"),
    };
    let s = crate::strings();
    if path.is_empty() {
        return status(s.t("image.none", &[]), look.muted);
    }
    let key = (path, on, rev);
    let view = window.use_keyed_state(
        ElementId::Name(SharedString::from(format!("yuzora-image-{}", args.id()))),
        cx,
        {
            let key = key.clone();
            move |_, cx| ImageView::new(key, cx)
        },
    );
    view.update(cx, |v, cx| {
        if v.key != key {
            v.key = key;
            v.load(cx);
        }
    });
    let v = view.read(cx);
    match &v.loaded {
        Loaded::Loading => status(s.t("image.loading", &[]), look.muted),
        Loaded::Failed(e) if crate::svg::REFUSALS.contains(&e.as_str()) => {
            not_drawn(s.t(&format!("image.{e}"), &[]), look.muted)
        }
        Loaded::Failed(e) if REFUSALS.contains(&e.as_str()) => {
            status(s.t(&format!("image.{e}"), &[]), look.muted)
        }
        Loaded::Failed(e) => status(s.t("image.failed", &[("error", e.clone())]), look.muted),
        Loaded::Ready(image) => {
            let source = image.source();
            let muted = look.muted;
            let picture = img(source).with_fallback(move || {
                status(
                    crate::strings().t("image.failed", &[("error", String::new())]),
                    muted,
                )
            });
            match look.fit {
                Fit::Contain => div()
                    .flex()
                    .size_full()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .p(px(12.))
                    .when_some(look.checker, |el, c| el.bg(c))
                    .child(
                        picture
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::ScaleDown),
                    )
                    .into_any_element(),
                // Its own size, centered while it fits, scrolled both ways when not.
                Fit::Actual => div()
                    .id(SharedString::from(format!(
                        "yuzora-image-scroll-{}",
                        args.id()
                    )))
                    .flex()
                    .items_start()
                    .size_full()
                    .min_h_0()
                    .overflow_scroll()
                    .when_some(look.checker, |el, c| el.bg(c))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .min_w_full()
                            .min_h_full()
                            .items_center()
                            .justify_center()
                            .p(px(12.))
                            .child(picture.flex_none()),
                    )
                    .into_any_element(),
            }
        }
    }
}

use gpui::prelude::FluentBuilder as _;

/// A GIF of a `size`² canvas holding `frames` 1×1 images.
#[cfg(test)]
pub(crate) fn test_gif(size: u16, frames: usize) -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend(size.to_le_bytes());
    b.extend(size.to_le_bytes());
    // A global color table of 2 entries.
    b.extend([0x80, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff]);
    // Loop forever (an extension), so animated.
    b.extend([0x21, 0xff, 0x0b]);
    b.extend(b"NETSCAPE2.0");
    b.extend([0x03, 0x01, 0, 0, 0]);
    for _ in 0..frames {
        b.extend([0x21, 0xf9, 0x04, 0, 0x0a, 0, 0, 0]);
        b.extend([0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0x02, 0x02, 0x44, 0x01, 0]);
    }
    b.push(0x3b);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn formats_follow_the_extension() {
        assert_eq!(format_of("a/b.PNG"), Some(ImageFormat::Png));
        assert_eq!(format_of("x.svg"), Some(ImageFormat::Svg));
        assert_eq!(format_of("x.jpeg"), Some(ImageFormat::Jpeg));
        assert_eq!(format_of("x.txt"), None);
        assert_eq!(format_of("noext"), None);
    }

    #[test]
    fn local_reads_refuse_huge_files_and_report_missing_ones() {
        assert!(read("/definitely/not/here.png", None).is_err());
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.png");
        std::fs::write(&p, [1u8, 2, 3]).unwrap();
        assert_eq!(read(p.to_str().unwrap(), None).unwrap(), vec![1, 2, 3]);
        // A folder, and (here) a device that never ends: not read at all.
        assert_eq!(
            read(dir.path().to_str().unwrap(), None).unwrap_err(),
            "unsupported"
        );
        #[cfg(unix)]
        assert_eq!(read("/dev/zero", None).unwrap_err(), "unsupported");
    }

    #[cfg(unix)]
    #[test]
    fn remote_reads_take_regular_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a b.png");
        std::fs::write(&p, [1u8, 2, 3]).unwrap();
        // The script run here, as it runs on the other machine.
        assert_eq!(read_sh(p.to_str().unwrap(), None).unwrap(), vec![1, 2, 3]);
        assert_eq!(
            read_sh(dir.path().to_str().unwrap(), None).unwrap_err(),
            "unsupported"
        );
        assert_eq!(read_sh("/dev/zero", None).unwrap_err(), "unsupported");
        let missing = dir.path().join("missing.png");
        assert!(
            read_sh(missing.to_str().unwrap(), None)
                .unwrap_err()
                .starts_with("no such file")
        );
    }

    use super::test_gif as gif;

    #[test]
    fn formats_are_guessed_from_bytes_without_an_extension() {
        assert_eq!(format_in(&gif(1, 1)), ImageFormat::Gif);
        assert_eq!(format_in(b"\x89PNG\r\n\x1a\n"), ImageFormat::Png);
        assert_eq!(format_in(b"<svg/>"), ImageFormat::Svg);
    }

    #[test]
    fn a_gif_is_shown_only_when_all_its_frames_fit_in_memory() {
        assert_eq!(gif_frames(&gif(8, 3)), 3);
        // Cut short: the frames up to there.
        let cut = gif(8, 3);
        assert_eq!(gif_frames(&cut[..cut.len() - 20]), 2);
        assert!(fits_in_memory(&gif(8, 3)));
        // 8192² is 256 MB a frame: two fill the budget, three are over it.
        assert!(fits_in_memory(&gif(8192, 2)));
        assert!(!fits_in_memory(&gif(8192, 3)));
    }

    #[test]
    fn webp_frames_are_its_anmf_chunks() {
        let chunk = |name: &[u8; 4], body: &[u8]| {
            let mut c = name.to_vec();
            c.extend((body.len() as u32).to_le_bytes());
            c.extend(body);
            if body.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        let mut b = b"RIFF\0\0\0\0WEBP".to_vec();
        b.extend(chunk(b"VP8X", &[0x02; 10]));
        b.extend(chunk(b"ANIM", &[0; 6]));
        b.extend(chunk(b"ANMF", &[0; 17]));
        b.extend(chunk(b"ANMF", &[0; 16]));
        assert_eq!(webp_frames(&b), 2);
        assert_eq!(webp_frames(b"RIFF\0\0\0\0WEBP"), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_pane_draws_a_repo_svg_in_a_child() {
        // Red here; the test child answers green.
        let svg = svg_with(r#"<rect width="4" height="4" fill="red"/><!-- pane -->"#);
        let cancel = Cancel::default();
        let Ok(Picture::Drawn(image)) =
            Picture::of(ImageFormat::Svg, "a.svg", svg.into_bytes(), &cancel)
        else {
            panic!("not drawn");
        };
        assert_eq!(image.as_bytes(0).unwrap(), crate::svg::TEST_CHILD_PIXEL);
        assert_eq!(crate::svg::drawn_here(), 0);
    }

    #[gpui::test]
    fn a_pane_closed_or_moved_on_cancels_its_load(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let view = cx.new(|cx| ImageView::new(("/none/a.svg".into(), None, 0), cx));
        let first = view.update(cx, |v, _| v.cancel.clone());
        assert!(!first.is_cancelled());
        // Another file: the first load stops.
        let second = view.update(cx, |v, cx| {
            v.key = ("/none/b.svg".into(), None, 0);
            v.load(cx);
            v.cancel.clone()
        });
        assert!(first.is_cancelled() && !second.is_cancelled());
        drop(view);
        cx.update(|_| {});
        assert!(second.is_cancelled());
    }

    #[test]
    fn every_refusal_has_its_text() {
        let en: serde_json::Value = serde_json::from_str(include_str!("../i18n/en.json")).unwrap();
        for key in REFUSALS.iter().chain(&crate::svg::REFUSALS) {
            assert!(en["image"][key].is_string(), "image.{key}");
        }
    }

    #[test]
    fn what_has_no_raster_header_is_left_to_the_decoder() {
        assert!(fits_in_memory(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(fits_in_memory(b""));
    }

    // GPUI's SVG renderer, the one the image pane, Markdown and every mod's
    // `svg()`/`img()` use: an SVG's `<image>` reads no file and decodes no
    // raster it carries.

    /// A 4×4 red PNG.
    const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGP4z8DwHxkzkC4AADxAH+HggXe0AAAAAElFTkSuQmCC";
    /// A PNG whose header says 60000×60000 (14 GB decoded), 74 bytes long.
    const HUGE_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAA6mAAAOpgCAYAAACA0nVCAAAAEUlEQVR4nGNgGAWjYBQMdwAAA+gAAbOm00YAAAAASUVORK5CYII=";

    use crate::svg::samples::{gzip, image_at, nested_in_images, svg_data, svg_with};

    /// The stack of the dispatch queue threads GPUI decodes images on
    /// (macOS), where a mod's SVG is drawn.
    const DECODER_STACK: usize = 536_576;

    /// The top-left pixel (BGRA) of `svg` as GPUI's own decoder renders it, on
    /// a stack that size, within ten seconds.
    fn first_pixel(svg: String) -> [u8; 4] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .stack_size(DECODER_STACK)
            .spawn(move || {
                let image = gpui::SvgRenderer::new(std::sync::Arc::new(()))
                    .render_single_frame(svg.as_bytes(), 1.0)
                    .unwrap();
                let _ = tx.send(<[u8; 4]>::try_from(&image.as_bytes(0).unwrap()[..4]).unwrap());
            })
            .unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("the SVG didn't render within ten seconds")
    }

    const RED: [u8; 4] = [0, 0, 255, 255];
    const NOTHING: [u8; 4] = [0, 0, 0, 0];

    #[test]
    fn an_svg_image_reads_no_file() {
        assert_eq!(
            first_pixel(svg_with(r#"<rect width="4" height="4" fill="red"/>"#)),
            RED
        );
        let dir = tempfile::tempdir().unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(RED_PNG)
            .unwrap();
        let red = dir.path().join("red.png");
        std::fs::write(&red, &png).unwrap();
        // A big file is not read whole either: 64 MiB, red to a PNG decoder
        // (it stops at the image's end).
        let big = dir.path().join("big.png");
        std::fs::write(&big, &png).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&big)
            .unwrap()
            .set_len(64 << 20)
            .unwrap();
        let red = red.to_string_lossy().into_owned();
        #[allow(unused_mut)]
        let mut hrefs = vec![red.clone(), big.to_string_lossy().into_owned()];
        // The same file by its long name.
        #[cfg(windows)]
        hrefs.push(format!(r"\\?\{red}"));
        for href in hrefs {
            assert_eq!(first_pixel(image_at(&href)), NOTHING, "{href}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_svg_image_never_waits_on_a_fifo() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.png");
        let name = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let svg = image_at(&fifo.to_string_lossy());
        std::thread::spawn(move || {
            let _ = gpui::SvgRenderer::new(std::sync::Arc::new(()))
                .render_single_frame(svg.as_bytes(), 1.0);
            let _ = tx.send(());
        });
        if rx.recv_timeout(std::time::Duration::from_secs(10)).is_err() {
            // Let the stuck reader go before failing.
            let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
            panic!("the SVG waited on a FIFO");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_svg_image_never_reads_a_device() {
        // Read, /dev/zero never ends: the app would grow until it aborts.
        assert_eq!(first_pixel(image_at("/dev/zero")), NOTHING);
    }

    #[test]
    fn an_svg_decodes_no_raster_it_carries() {
        for mime in ["image/png", "text/plain", ""] {
            let svg = image_at(&format!("data:{mime};base64,{RED_PNG}"));
            assert_eq!(first_pixel(svg), NOTHING, "{mime}");
        }
        // Whatever size its header claims.
        assert_eq!(
            first_pixel(image_at(&format!("data:image/png;base64,{HUGE_PNG}"))),
            NOTHING
        );
        // Nor an SVG: each would be parsed inside the last.
        let rect = svg_with(r#"<rect width="4" height="4" fill="red"/>"#);
        let base64 = base64::engine::general_purpose::STANDARD.encode(&rect);
        for href in [
            format!("data:image/svg+xml;base64,{base64}"),
            svg_data(&rect),
        ] {
            assert_eq!(first_pixel(image_at(&href)), NOTHING, "{href}");
        }
    }

    #[test]
    fn svgs_nested_in_data_never_run_the_stack_out() {
        // 75 deep, drawn on a decoder thread: the process used to abort.
        let rect = svg_with(r#"<rect width="4" height="4" fill="red"/>"#);
        assert_eq!(first_pixel(nested_in_images(rect, 75)), NOTHING);
    }

    #[test]
    fn an_svg_inflates_no_gzip_it_carries() {
        // usvg inflates an SVGZ whole, however far it expands.
        let rect = svg_with(r#"<rect width="4" height="4" fill="red"/>"#);
        let base64 = base64::engine::general_purpose::STANDARD.encode(gzip(rect.as_bytes()));
        let svg = image_at(&format!("data:image/svg+xml;base64,{base64}"));
        assert_eq!(first_pixel(svg), NOTHING);
    }
}
