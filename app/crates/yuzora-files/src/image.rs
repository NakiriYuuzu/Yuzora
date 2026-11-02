//! The image viewer: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO and SVG, here or on
//! another machine. The bytes are read by us (not GPUI's path cache), so a
//! file that changed on disk shows its new content when `rev` changes.

use std::io::Read as _;
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
            let bytes = blocking(move || read(&path, on.as_deref()))
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
}
