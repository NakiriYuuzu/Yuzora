//! The image viewer: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO and SVG, here or on
//! another machine. The bytes are read by us (not GPUI's path cache), so a
//! file that changed on disk shows its new content when `rev` changes.

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, ElementId, Hsla, Image, ImageFormat, ImageSource,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, div, img, px,
};
use yuzora_core::HostValue;

use crate::docs::blocking;
use crate::fsops::{failure, machine, run_sh};
use crate::highlight;

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

/// [`fits_in_memory`] for a file on this machine: only a GIF or WebP is read
/// whole (at most [`MAX_IMAGE_BYTES`]) to count its frames; others, as far
/// as their size.
pub fn file_fits_in_memory(path: &Path) -> bool {
    let reader = image::ImageReader::open(path).and_then(|r| r.with_guessed_format());
    let Ok(reader) = reader else { return false };
    match reader.format() {
        Some(image::ImageFormat::Gif | image::ImageFormat::WebP) => {
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .and_then(|f| f.take(MAX_IMAGE_BYTES).read_to_end(&mut bytes))
                .is_ok()
                && fits_in_memory(&bytes)
        }
        _ => reader
            .into_dimensions()
            .map_or(true, |(w, h)| within_budget(w, h, 1)),
    }
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

fn read(path: &str, on: Option<&str>) -> Result<Vec<u8>, String> {
    match machine(on) {
        None => {
            // Looked at before opening: a FIFO would wait, a device never ends.
            let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
            if !meta.is_file() {
                return Err("unsupported".into());
            }
            if meta.len() > MAX_IMAGE_BYTES {
                return Err("too_large".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(path)
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

enum Loaded {
    Loading,
    Ready(Arc<Image>),
    Failed(String),
}

struct ImageView {
    key: (String, Option<String>, u64),
    loaded: Loaded,
}

impl ImageView {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.loaded = Loaded::Loading;
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
            let bytes = blocking(move || {
                read(&path, on.as_deref()).and_then(|bytes| {
                    if fits_in_memory(&bytes) {
                        Ok(bytes)
                    } else {
                        Err("too_large_decoded".into())
                    }
                })
            })
            .await
            .unwrap_or_else(|| Err("worker".into()));
            let _ = this.update(cx, |v, cx| {
                if v.key != key {
                    return;
                }
                v.loaded = match bytes {
                    Ok(bytes) => Loaded::Ready(Arc::new(Image::from_bytes(format, bytes))),
                    Err(e) => Loaded::Failed(e),
                };
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
        muted: color("muted").unwrap_or_else(|| gpui::rgb(0xa1a1aa).into()),
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
            move |_, cx| {
                let mut v = ImageView {
                    key,
                    loaded: Loaded::Loading,
                };
                v.load(cx);
                v
            }
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
        Loaded::Failed(e) if e == "too_large" => status(s.t("image.too_large", &[]), look.muted),
        Loaded::Failed(e) if e == "too_large_decoded" => {
            status(s.t("image.too_large_decoded", &[]), look.muted)
        }
        Loaded::Failed(e) if e == "unsupported" => {
            status(s.t("image.unsupported", &[]), look.muted)
        }
        Loaded::Failed(e) => status(s.t("image.failed", &[("error", e.clone())]), look.muted),
        Loaded::Ready(image) => {
            let source = ImageSource::Image(image.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A GIF of a `size`² canvas holding `frames` 1×1 images.
    fn gif(size: u16, frames: usize) -> Vec<u8> {
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
        // A file is checked the same way.
        let dir = tempfile::tempdir().unwrap();
        let (small, big) = (dir.path().join("s.gif"), dir.path().join("b.gif"));
        std::fs::write(&small, gif(8, 3)).unwrap();
        std::fs::write(&big, gif(8192, 3)).unwrap();
        assert!(file_fits_in_memory(&small));
        assert!(!file_fits_in_memory(&big));
        assert!(!file_fits_in_memory(&dir.path().join("missing.gif")));
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

    #[test]
    fn what_has_no_raster_header_is_left_to_the_decoder() {
        assert!(fits_in_memory(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
        assert!(fits_in_memory(b""));
    }
}
