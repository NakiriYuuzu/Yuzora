//! The Markdown preview: a document's text rendered as GFM by gpui-base's
//! `TextView`, code blocks highlighted by [`crate::highlight`]. It watches
//! the document, so it follows every keystroke in whichever editor has it.
//!
//! Images: relative paths load from the file's folder (local files only),
//! and only regular files up to the image pane's size limit, checked on a
//! worker (an image shows once its check is back). Network, UNC
//! (`//host/share`), inline (`data:`) images and those of a file on another
//! machine are not fetched. Links: only http,
//! https and mailto open, in the system's browser or mail app.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, ElementId, Entity, FontWeight, Hsla, ImageFormat, ImageSource,
    IntoElement, ParentElement as _, Render, RenderImage, SharedString, SharedUri, StyleRefinement,
    Styled as _, Subscription, Window, div, px,
};
use gpui_base::text::{TextView, TextViewStyle};
use yuzora_core::HostValue;

use crate::docs::{self, DocView};
use crate::fsops::{self, Stamp};
use crate::highlight::{self, SyntaxPalette};
use crate::image::Picture;
use crate::svg::Cancel;

/// Where a document image comes from, or `None` when it is not loaded.
pub fn image_path(url: &str, base: Option<&Path>) -> Option<PathBuf> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if url.is_empty()
        || lower.starts_with("data:")
        || lower.contains("://") && !lower.starts_with("file://")
    {
        return None;
    }
    let uri = url.strip_prefix("file://");
    let path = uri.unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let mut path = percent_decode(path);
    if uri.is_some() {
        path = drive_of_uri(&path, cfg!(windows)).to_owned();
    }
    // `//host/share/x.png` is a network path on Windows, whichever slashes.
    let slashed = path.replace('\\', "/");
    if slashed.starts_with("//") {
        return None;
    }
    let path = Path::new(&path);
    // Drive letters, `\\?\`, `\\.\` and UNC prefixes (Windows only).
    if path.components().any(|c| matches!(c, std::path::Component::Prefix(p) if !matches!(p.kind(), std::path::Prefix::Disk(_)))) {
        return None;
    }
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    Some(base?.join(path))
}

/// A `file:` URI's path names a drive as `/C:/…`; on Windows that is `C:/…`
/// (with the slash, it would be a folder under the document's).
fn drive_of_uri(path: &str, windows: bool) -> &str {
    let b = path.as_bytes();
    let drive = b.len() >= 3
        && b[0] == b'/'
        && b[1].is_ascii_alphabetic()
        && b[2] == b':'
        && b.get(3).is_none_or(|&c| c == b'/' || c == b'\\');
    if windows && drive { &path[1..] } else { path }
}

/// Whether a clicked link may open outside the app.
pub fn opens_outside(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

fn percent_decode(s: &str) -> String {
    // Bytes, not string slices: a `%` before a multibyte character is not an
    // escape, and slicing into that character would panic.
    let hex = |b: u8| char::from(b).to_digit(16);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

#[derive(Clone, PartialEq)]
struct Colors {
    foreground: Option<Hsla>,
    body: Option<Hsla>,
    muted: Option<Hsla>,
    link: Option<Hsla>,
    code: Option<Hsla>,
    border: Option<Hsla>,
    /// Code blocks' colors by semantic name, as the editor's (`syntax`).
    syntax: Vec<(String, Hsla)>,
}

impl Colors {
    fn from_props(props: &HostValue) -> Self {
        let c = |k: &str| {
            props
                .get("colors")
                .and_then(|c| c.get(k))
                .and_then(HostValue::as_str)
                .and_then(highlight::parse_hex)
        };
        Self {
            foreground: c("foreground"),
            body: c("body"),
            muted: c("muted"),
            link: c("link"),
            code: c("code"),
            border: c("border"),
            syntax: crate::editor::syntax_overrides(props),
        }
    }

    /// Highlights code blocks in these colors.
    fn highlighter(&self) -> highlight::CodeBlockHighlighter {
        let palette = SyntaxPalette::one_dark().with_overrides(self.syntax.clone());
        highlight::code_block_highlighter(Arc::new(palette))
    }
}

fn missing_image() -> ImageSource {
    ImageSource::from(PathBuf::new())
}

/// What a look at an image file found.
enum Found {
    /// The stamp of what is shown: not read again.
    Same,
    /// What to show, checked (an SVG drawn by a child, or a placeholder when
    /// it isn't), and the stamp from before it was read (a change after that
    /// is seen the next time, and only then is an SVG drawn again).
    Read { stamp: Stamp, picture: Picture },
    /// Not a regular file (a device, FIFO or folder), too big, too big once
    /// decoded, or the preview closed while its SVG was drawn.
    Nothing,
}

/// Reads an image for the preview, unless its stamp is still `shown`. GPUI
/// gets these bytes, not the path: what it decodes is what was checked
/// ([`crate::image::read`] reads no more than the limit). Blocks: an SVG is
/// drawn by a child, until `cancel`.
fn read_image(path: &Path, shown: Option<Stamp>, cancel: &Cancel) -> Found {
    let stamp = std::fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| fsops::stamp_of(&m));
    let (Some(stamp), Some(name)) = (stamp, path.to_str()) else {
        return Found::Nothing;
    };
    if shown == Some(stamp) {
        return Found::Same;
    }
    let picture = crate::image::read(name, None)
        .ok()
        .filter(|bytes| crate::image::fits_in_memory(bytes))
        .and_then(|bytes| {
            let format =
                crate::image::format_of(name).unwrap_or_else(|| crate::image::format_in(&bytes));
            match Picture::of(format, name, bytes, cancel) {
                Ok(picture) => Some(picture),
                Err(e) if format == ImageFormat::Svg && e != crate::svg::CANCELLED => {
                    Some(Picture::Drawn(crate::svg::placeholder()))
                }
                Err(_) => None,
            }
        });
    match picture {
        Some(picture) => Found::Read { stamp, picture },
        None => Found::Nothing,
    }
}

/// A path's answer is looked at again after this long.
const RECHECK: Duration = Duration::from_secs(10);
/// An image the last frame didn't draw, and none has for this long, is let
/// go (with its bytes). Off screen it is read again when drawn.
const FORGET: Duration = Duration::from_secs(60);

/// What a preview shows for one image path.
struct Known {
    /// The image and the stamp it was read at; `None`: nothing to show.
    shown: Option<(Stamp, Picture)>,
    /// When it was looked at.
    at: Instant,
    /// When a frame last drew it, and which.
    asked: Instant,
    frame: u64,
}

/// Which of a preview's images may be shown. Drawing only reads it; the
/// files are looked at on a worker after the frame.
#[derive(Default)]
struct ImageChecks {
    /// What each path shows.
    known: HashMap<PathBuf, Known>,
    /// Paths drawn without an answer, or with an old one.
    wanted: HashSet<PathBuf>,
    /// A check is queued or running.
    busy: bool,
    /// A frame drew an image it couldn't show (yet).
    missing: bool,
    /// A redraw to look at missing images again is waiting.
    timer: bool,
    /// The frame drawn last (counted as the preview renders).
    frame: u64,
    /// A pass to let go of images no longer drawn is waiting.
    forgetting: bool,
    /// SVGs let go of whose textures are still to be dropped (that takes the
    /// app, which drawing and the checks don't have).
    released: Vec<Arc<RenderImage>>,
    /// The checks' SVG children: killed when the preview closes.
    cancel: Cancel,
}

impl ImageChecks {
    /// The image for `path` so far (none while unknown); asks for a check
    /// when there is no answer or it is old.
    fn answer(&mut self, path: &Path) -> Option<Picture> {
        match self.known.get_mut(path) {
            Some(known) => {
                known.asked = Instant::now();
                known.frame = self.frame;
                if known.at.elapsed() > RECHECK {
                    self.wanted.insert(path.to_owned());
                }
                self.missing |= known.shown.is_none();
                known.shown.as_ref().map(|(_, image)| image.clone())
            }
            None => {
                self.wanted.insert(path.to_owned());
                self.missing = true;
                None
            }
        }
    }

    /// The paths to look at, each with the stamp of what it shows.
    fn take_wanted(&mut self) -> Vec<(PathBuf, Option<Stamp>)> {
        let wanted: Vec<PathBuf> = self.wanted.drain().collect();
        wanted
            .into_iter()
            .map(|path| {
                let shown = self.known.get(&path).and_then(|k| k.shown.as_ref());
                let stamp = shown.map(|(stamp, _)| *stamp);
                (path, stamp)
            })
            .collect()
    }

    /// Records answers; true when one changed what is shown (an image
    /// replaced at the same path too). Lets go of images not drawn lately.
    fn record(&mut self, answers: Vec<(PathBuf, Found)>) -> bool {
        let now = Instant::now();
        self.forget();
        let mut changed = false;
        for (path, found) in answers {
            let known = self.known.entry(path).or_insert(Known {
                shown: None,
                at: now,
                asked: now,
                frame: self.frame,
            });
            known.at = now;
            let was = match found {
                Found::Same => None,
                Found::Read { stamp, picture } => {
                    changed = true;
                    known.shown.replace((stamp, picture))
                }
                Found::Nothing => known.shown.take(),
            };
            changed |= was.is_some();
            self.released.extend(was.and_then(|(_, p)| p.drawn()));
        }
        changed
    }

    /// Lets go of the images the last frame didn't draw and none has for
    /// [`FORGET`]. True when some it didn't draw are left, for later.
    fn forget(&mut self) -> bool {
        let (now, frame) = (Instant::now(), self.frame);
        let released = &mut self.released;
        self.known.retain(|_, k| {
            let keep = k.frame == frame || now.duration_since(k.asked) < FORGET;
            if !keep {
                released.extend(k.shown.take().and_then(|(_, p)| p.drawn()));
            }
            keep
        });
        self.known.values().any(|k| k.frame != frame)
    }

    /// Everything shown, let go of, and the checks under way stopped (the
    /// preview is closed).
    fn clear(&mut self) {
        self.cancel.cancel();
        let shown = self.known.drain().filter_map(|(_, k)| k.shown);
        self.released.extend(shown.filter_map(|(_, p)| p.drawn()));
    }
}

/// Drops the textures of the SVGs `images` let go of.
fn drop_released(images: &Mutex<ImageChecks>, cx: &mut App) {
    let released = std::mem::take(&mut images.lock().unwrap_or_else(|e| e.into_inner()).released);
    for image in released {
        cx.drop_image(image, None);
    }
}

/// One preview pane's view.
pub struct MarkdownView {
    doc: Entity<DocView>,
    id: SharedString,
    base: Option<PathBuf>,
    /// The file is on another machine: no image is read from this one.
    remote: bool,
    colors: Colors,
    highlighter: highlight::CodeBlockHighlighter,
    /// The text shown and the document revision it is from.
    text: SharedString,
    rev: u64,
    images: Arc<Mutex<ImageChecks>>,
    _watch: Subscription,
}

impl MarkdownView {
    fn new(doc: Entity<DocView>, id: SharedString, colors: Colors, cx: &mut Context<Self>) -> Self {
        let watch = Self::watch(&doc, cx);
        cx.on_release(|this: &mut Self, cx| {
            this.images
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            drop_released(&this.images, cx);
        })
        .detach();
        let base = Self::base_of(&doc, cx);
        let remote = doc.read(cx).key.on.is_some();
        Self {
            doc,
            id,
            base,
            remote,
            highlighter: colors.highlighter(),
            colors,
            text: SharedString::default(),
            rev: u64::MAX,
            images: Arc::default(),
            _watch: watch,
        }
    }

    /// After this frame (whose layout asks for the images), looks at the
    /// files it had no answer for, then redraws if one may now be shown.
    fn check_images(&self, cx: &mut Context<Self>) {
        let images = self.images.clone();
        {
            let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
            if self.remote || checks.busy {
                return;
            }
            checks.busy = true;
        }
        cx.spawn(async move |this, cx| {
            let mut changed = false;
            // Frames drawn meanwhile may ask for more.
            loop {
                let (wanted, cancel) = {
                    let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
                    if checks.wanted.is_empty() {
                        checks.busy = false;
                        break;
                    }
                    (checks.take_wanted(), checks.cancel.clone())
                };
                let answers = docs::blocking(move || {
                    wanted
                        .into_iter()
                        .map(|(path, shown)| {
                            let found = read_image(&path, shown, &cancel);
                            (path, found)
                        })
                        .collect()
                })
                .await
                .unwrap_or_default();
                changed |= images
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .record(answers);
            }
            let _ = this.update(cx, |_, cx| {
                drop_released(&images, cx);
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Lets go later of images the frames stopped drawing (an edit took them
    /// out, or they are off screen): no frame may come to do it.
    fn forget_later(&self, cx: &mut Context<Self>) {
        let images = self.images.clone();
        {
            let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
            if checks.forgetting || checks.known.is_empty() {
                return;
            }
            checks.forgetting = true;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FORGET).await;
                let more = {
                    let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
                    let more = this.upgrade().is_some() && checks.forget();
                    checks.forgetting = more;
                    more
                };
                let _ = this.update(cx, |_, cx| drop_released(&images, cx));
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// While a frame shows a missing image, draws again once its answer is
    /// old enough to be looked at again: the file may have been made since.
    fn recheck_later(&self, cx: &mut Context<Self>) {
        let images = self.images.clone();
        {
            let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
            if self.remote || checks.timer || !checks.missing {
                return;
            }
            checks.missing = false;
            checks.timer = true;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECHECK).await;
            images.lock().unwrap_or_else(|e| e.into_inner()).timer = false;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// Redraws when the text changed, not on every cursor blink of the
    /// editor, or when a rename moved the folder images load from.
    fn watch(doc: &Entity<DocView>, cx: &mut Context<Self>) -> Subscription {
        cx.observe(doc, |this: &mut Self, doc, cx| {
            let base = Self::base_of(&doc, cx);
            if base != this.base {
                this.base = base;
                cx.notify();
            } else if doc.read(cx).rev() != this.rev {
                cx.notify();
            }
        })
    }

    /// The script's colors. Code blocks are colored again only when their
    /// colors change: they cache their highlighting by the highlighter.
    fn set_colors(&mut self, colors: Colors, cx: &mut Context<Self>) {
        if self.colors == colors {
            return;
        }
        if self.colors.syntax != colors.syntax {
            self.highlighter = colors.highlighter();
        }
        self.colors = colors;
        cx.notify();
    }

    fn base_of(doc: &Entity<DocView>, cx: &App) -> Option<PathBuf> {
        let key = &doc.read(cx).key;
        if key.on.is_some() {
            return None;
        }
        Path::new(&key.path).parent().map(Path::to_path_buf)
    }

    fn style(&self, cx: &App) -> TextViewStyle {
        let mut style = TextViewStyle::from_theme(&gpui_base::Theme::global(cx));
        if let Some(c) = self.colors.body.or(self.colors.foreground) {
            style = style.with_foreground(c);
        }
        if let Some(c) = self.colors.muted {
            style = style.with_muted_foreground(c);
        }
        if let Some(c) = self.colors.link {
            style = style.with_link(c);
        }
        if let Some(c) = self.colors.code {
            style = style
                .with_code_background(c)
                .with_inline_code(gpui::HighlightStyle {
                    background_color: Some(c),
                    ..Default::default()
                });
        }
        if let Some(c) = self.colors.border {
            style = style.with_border(c);
        }
        let heading_color = self.colors.foreground;
        style
            .with_heading(move |level| {
                let (size, weight) = match level {
                    1 => (22., FontWeight::BOLD),
                    2 => (17., FontWeight::BOLD),
                    3 => (14., FontWeight::SEMIBOLD),
                    _ => (13., FontWeight::SEMIBOLD),
                };
                let mut s = StyleRefinement::default()
                    .text_size(px(size))
                    .font_weight(weight);
                if let Some(c) = heading_color {
                    s = s.text_color(c);
                }
                s
            })
            .with_code_block(StyleRefinement::default().rounded(px(6.)).p(px(10.)))
    }
}

impl Render for MarkdownView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rev = self.doc.read(cx).rev();
        if rev != self.rev {
            self.text = self.doc.read(cx).text(cx);
            self.rev = rev;
        }
        let text = self.text.clone();
        let base = self.base.clone();
        let remote = self.remote;
        let images = self.images.clone();
        images.lock().unwrap_or_else(|e| e.into_inner()).frame += 1;
        self.check_images(cx);
        self.recheck_later(cx);
        self.forget_later(cx);
        div().size_full().min_h_0().px(px(18.)).py(px(14.)).child(
            TextView::markdown(ElementId::Name(self.id.clone()), text)
                .style(self.style(cx))
                .shared_code_block_highlighter(self.highlighter.clone())
                .image_source(move |uri: &SharedUri| {
                    let path = image_path(uri.as_ref(), base.as_deref()).filter(|_| !remote);
                    let image = path.and_then(|path| {
                        images
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .answer(&path)
                    });
                    image.map_or_else(missing_image, |image| image.source())
                })
                .on_link_click(|url, _, _, cx| {
                    if opens_outside(url) {
                        cx.open_url(url);
                    }
                })
                .selectable(true)
                .scrollable(true),
        )
    }
}

/// The `Markdown` component: `Markdown.new(id, {doc, colors?})`.
pub fn markdown_component(
    args: gpui_shell::ComponentArgs<'_>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let props = args.props();
    let Some(doc) = props
        .get("doc")
        .and_then(HostValue::as_number)
        .and_then(|id| docs::get(id as u64, cx))
    else {
        return div().size_full().into_any_element();
    };
    let id = SharedString::from(format!("yuzora-md-{}", args.id()));
    let colors = Colors::from_props(props);
    let key = ElementId::Name(id.clone());
    let init_doc = doc.clone();
    let init_colors = colors.clone();
    let init_id = id.clone();
    let view = window.use_keyed_state(key, cx, move |_, cx| {
        MarkdownView::new(init_doc, init_id, init_colors, cx)
    });
    view.update(cx, |v, cx| {
        if v.doc != doc {
            v._watch = MarkdownView::watch(&doc, cx);
            v.base = MarkdownView::base_of(&doc, cx);
            v.remote = doc.read(cx).key.on.is_some();
            v.doc = doc;
            v.rev = u64::MAX;
            v.images
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cancel
                .cancel();
            v.images = Arc::default();
            cx.notify();
        }
        v.set_colors(colors, cx);
    });
    view.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::samples::{gzip, svg_with};
    use gpui::Image;

    const RED_RECT: &str = r#"<rect width="4" height="4" fill="red"/>"#;

    /// A PNG's bytes to show, `len` long.
    fn png(len: usize) -> Picture {
        Picture::Bytes(Arc::new(Image::from_bytes(ImageFormat::Png, vec![0; len])))
    }

    /// An SVG drawn (pixels as a child sends them).
    fn drawn() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![::image::Frame::new(
            ::image::RgbaImage::new(1, 1),
        )]))
    }

    fn bytes_of(picture: Option<Picture>) -> Arc<Image> {
        match picture {
            Some(Picture::Bytes(image)) => image,
            _ => panic!("no bytes shown"),
        }
    }

    #[gpui::test]
    fn code_blocks_take_the_script_colors(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "text\n").unwrap();
        let key = docs::DocKey {
            on: None,
            path: dir.path().join("a.md").to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| docs::open(key, cx));
        cx.run_until_parked();
        let doc = cx.update(|cx| docs::get(id, cx).unwrap());
        let obj = |kv: Vec<(&str, HostValue)>| {
            HostValue::Object(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
        };
        let colors = |keyword: &str, link: &str| {
            Colors::from_props(&obj(vec![
                ("colors", obj(vec![("link", HostValue::Str(link.into()))])),
                (
                    "syntax",
                    obj(vec![("keyword", HostValue::Str(keyword.into()))]),
                ),
            ]))
        };
        let block = gpui_base::text::CodeBlock::from_code("fn main() {}", Some("rust"));
        // `fn`, a keyword.
        let keyword = |v: &MarkdownView| {
            (v.highlighter)(&block)
                .into_iter()
                .find(|(range, _)| *range == (0..2))
                .and_then(|(_, style)| style.color)
        };
        let (red, blue) = ("#ff0000", "#0000ff");
        let view = cx.new(|cx| MarkdownView::new(doc, "md".into(), colors(red, red), cx));
        view.update(cx, |v, cx| {
            assert_eq!(keyword(v), highlight::parse_hex(red));
            let first = v.highlighter.clone();
            v.set_colors(colors(red, blue), cx);
            assert!(
                Arc::ptr_eq(&first, &v.highlighter),
                "same code colors, same highlighting"
            );
            v.set_colors(colors(blue, blue), cx);
            assert_eq!(keyword(v), highlight::parse_hex(blue));
        });
    }

    #[gpui::test]
    fn images_follow_a_rename_of_the_folder(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        let (docs_dir, notes_dir) = (dir.path().join("docs"), dir.path().join("notes"));
        std::fs::create_dir(&docs_dir).unwrap();
        std::fs::write(docs_dir.join("a.md"), "![x](x.png)\n").unwrap();
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let key = docs::DocKey {
            on: None,
            path: path(&docs_dir.join("a.md")),
        };
        let id = cx.update(|cx| docs::open(key, cx));
        cx.run_until_parked();
        let doc = cx.update(|cx| docs::get(id, cx).unwrap());
        let colors = Colors::from_props(&HostValue::Null);
        let view = cx.new(|cx| MarkdownView::new(doc, "md".into(), colors, cx));
        cx.update(|cx| assert_eq!(view.read(cx).base.as_deref(), Some(docs_dir.as_path())));
        let rx = cx.update(|cx| docs::rename(path(&docs_dir), path(&notes_dir), None, cx));
        cx.run_until_parked();
        assert_eq!(futures::executor::block_on(rx).unwrap(), Ok(()));
        cx.update(|cx| assert_eq!(view.read(cx).base.as_deref(), Some(notes_dir.as_path())));
    }

    #[test]
    fn images_load_from_the_file_folder_and_never_from_the_network() {
        let base = Path::new("/repo/docs");
        assert_eq!(
            image_path("img/a.png", Some(base)),
            Some(PathBuf::from("/repo/docs/img/a.png"))
        );
        assert_eq!(
            image_path("/abs/b.svg", Some(base)),
            Some(PathBuf::from("/abs/b.svg"))
        );
        assert_eq!(
            image_path("a%20b.png?raw=1", Some(base)),
            Some(PathBuf::from("/repo/docs/a b.png"))
        );
        assert_eq!(image_path("https://example.com/x.png", Some(base)), None);
        assert_eq!(image_path("data:image/png;base64,AAAA", Some(base)), None);
        assert_eq!(image_path("rel.png", None), None);
        // Network paths on Windows.
        assert_eq!(image_path("//host/share/a.png", Some(base)), None);
        assert_eq!(image_path("file:////host/share/a.png", Some(base)), None);
        assert_eq!(image_path("\\\\host\\share\\a.png", Some(base)), None);
        assert_eq!(image_path("/\\host/share/a.png", Some(base)), None);
        assert_eq!(image_path("\\/host/share/a.png", Some(base)), None);
        assert_eq!(image_path("%5C%5Chost%5Cshare%5Ca.png", Some(base)), None);
        // A drive in a file URI, on Windows.
        assert_eq!(drive_of_uri("/C:/repo/a.png", true), "C:/repo/a.png");
        assert_eq!(drive_of_uri("/c:\\repo\\a.png", true), "c:\\repo\\a.png");
        assert_eq!(drive_of_uri("/C:", true), "C:");
        assert_eq!(drive_of_uri("/C:/repo/a.png", false), "/C:/repo/a.png");
        assert_eq!(drive_of_uri("/repo/a.png", true), "/repo/a.png");
        assert_eq!(drive_of_uri("/CD:/a.png", true), "/CD:/a.png");
        assert_eq!(drive_of_uri("/C:a.png", true), "/C:a.png");
        #[cfg(windows)]
        assert_eq!(
            image_path("file:///C:/repo/a.png", Some(base)),
            Some(PathBuf::from("C:/repo/a.png"))
        );
        #[cfg(not(windows))]
        assert_eq!(
            image_path("file:///C:/repo/a.png", Some(base)),
            Some(PathBuf::from("/C:/repo/a.png"))
        );
        // A stray `%` before a multibyte character is kept as it is.
        assert_eq!(
            image_path("%\u{1f9e9}.png", Some(base)),
            Some(PathBuf::from("/repo/docs/%\u{1f9e9}.png"))
        );
        assert_eq!(
            image_path("a%\u{00e9}%41.png", Some(base)),
            Some(PathBuf::from("/repo/docs/a%\u{00e9}A.png"))
        );
    }

    #[test]
    fn only_regular_files_within_the_limits_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"not really a png").unwrap();
        let cancel = Cancel::default();
        let Found::Read {
            stamp,
            picture: Picture::Bytes(image),
        } = read_image(&png, None, &cancel)
        else {
            panic!("not read");
        };
        assert_eq!(
            (image.format(), image.bytes()),
            (ImageFormat::Png, &b"not really a png"[..])
        );
        // Unchanged since it was read: not read again.
        assert!(matches!(
            read_image(&png, Some(stamp), &cancel),
            Found::Same
        ));
        // No extension: the format from its bytes.
        let bare = dir.path().join("picture");
        std::fs::write(&bare, crate::image::test_gif(1, 1)).unwrap();
        assert!(matches!(
            read_image(&bare, None, &cancel),
            Found::Read {
                picture: Picture::Bytes(image),
                ..
            } if image.format() == ImageFormat::Gif
        ));
        // Too big once decoded.
        let big = dir.path().join("big.gif");
        std::fs::write(&big, crate::image::test_gif(8192, 3)).unwrap();
        assert!(matches!(read_image(&big, None, &cancel), Found::Nothing));
        assert!(matches!(
            read_image(dir.path(), None, &cancel),
            Found::Nothing
        ));
        assert!(matches!(
            read_image(&dir.path().join("missing.png"), None, &cancel),
            Found::Nothing
        ));
        // An SVG is drawn by a child (the test one answers green, whatever the
        // SVG), never here.
        #[cfg(unix)]
        {
            let drawn = dir.path().join("a.svg");
            std::fs::write(
                &drawn,
                svg_with(r#"<rect width="4" height="4" fill="red"/><!-- md -->"#),
            )
            .unwrap();
            let Found::Read {
                picture: Picture::Drawn(image),
                ..
            } = read_image(&drawn, None, &cancel)
            else {
                panic!("not drawn");
            };
            assert_eq!(image.as_bytes(0).unwrap(), crate::svg::TEST_CHILD_PIXEL);
            assert_eq!(crate::svg::drawn_here(), 0);
        }
        // One that isn't drawn shows the placeholder (and isn't read again
        // while unchanged).
        let svgz = dir.path().join("b.svg");
        std::fs::write(&svgz, gzip(svg_with(RED_RECT).as_bytes())).unwrap();
        let Found::Read {
            stamp,
            picture: Picture::Drawn(image),
        } = read_image(&svgz, None, &cancel)
        else {
            panic!("no placeholder");
        };
        assert!(Arc::ptr_eq(&image, &crate::svg::placeholder()));
        assert!(matches!(
            read_image(&svgz, Some(stamp), &cancel),
            Found::Same
        ));
        // The preview closed meanwhile: nothing.
        let closed = Cancel::default();
        closed.cancel();
        let other = dir.path().join("c.svg");
        std::fs::write(&other, svg_with("<!-- closed -->")).unwrap();
        assert!(matches!(read_image(&other, None, &closed), Found::Nothing));
        #[cfg(unix)]
        {
            assert!(matches!(
                read_image(Path::new("/dev/zero"), None, &cancel),
                Found::Nothing
            ));
            let link = dir.path().join("zero.png");
            std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
            assert!(matches!(read_image(&link, None, &cancel), Found::Nothing));
        }
    }

    #[test]
    fn image_answers_wait_for_a_check_and_are_looked_at_again() {
        let stamp = |len| Stamp { len, mtime_ms: 1 };
        let read = |len| Found::Read {
            stamp: stamp(len),
            picture: png(len as usize),
        };
        let mut checks = ImageChecks::default();
        let (a, b) = (Path::new("/r/a.png"), Path::new("/r/b.png"));
        assert!(checks.answer(a).is_none());
        assert!(checks.answer(b).is_none());
        assert_eq!(checks.wanted.len(), 2);
        checks.wanted.clear();
        assert!(checks.record(vec![
            (a.to_owned(), read(1)),
            (b.to_owned(), Found::Nothing)
        ]));
        let shown = bytes_of(checks.answer(a));
        assert!(checks.answer(b).is_none());
        assert!(checks.wanted.is_empty());
        // Looked at again with the stamp it shows; unchanged, nothing changes.
        checks.wanted.insert(a.to_owned());
        assert_eq!(checks.take_wanted(), [(a.to_owned(), Some(stamp(1)))]);
        assert!(!checks.record(vec![
            (a.to_owned(), Found::Same),
            (b.to_owned(), Found::Nothing)
        ]));
        assert!(Arc::ptr_eq(&bytes_of(checks.answer(a)), &shown));
        // Replaced at the same path: the new image is shown.
        assert!(checks.record(vec![(a.to_owned(), read(2))]));
        assert!(!Arc::ptr_eq(&bytes_of(checks.answer(a)), &shown));
        // Gone: nothing shown.
        assert!(checks.record(vec![(a.to_owned(), Found::Nothing)]));
        assert!(checks.answer(a).is_none());
        // An old answer is still used while it is checked again.
        checks.record(vec![(a.to_owned(), read(3))]);
        checks.known.get_mut(a).unwrap().at = Instant::now() - RECHECK * 2;
        assert!(checks.answer(a).is_some());
        assert!(checks.wanted.contains(a));
    }

    #[test]
    fn images_no_longer_drawn_are_let_go() {
        let read = || Found::Read {
            stamp: Stamp {
                len: 1,
                mtime_ms: 1,
            },
            picture: png(1),
        };
        let (a, b) = (Path::new("/r/a.png"), Path::new("/r/b.png"));
        let mut checks = ImageChecks {
            frame: 1,
            ..Default::default()
        };
        checks.answer(a);
        checks.answer(b);
        checks.record(vec![(a.to_owned(), read()), (b.to_owned(), read())]);
        // The next frame draws only `a` (an edit took `b` out): `b` is kept
        // for now, to be looked at later.
        checks.frame = 2;
        checks.answer(a);
        assert!(checks.forget());
        assert!(checks.known.contains_key(b));
        // A while later, with no frame since: `b` goes; `a`, still on screen, stays.
        for k in checks.known.values_mut() {
            k.asked = Instant::now() - FORGET * 2;
        }
        assert!(!checks.forget());
        assert!(checks.known.contains_key(a) && !checks.known.contains_key(b));
        // Bytes: GPUI's to let go of.
        assert!(checks.released.is_empty());
    }

    #[test]
    fn svgs_let_go_of_have_their_textures_released() {
        let stamp = |len| Stamp { len, mtime_ms: 1 };
        let svg = |len, image: &Arc<RenderImage>| Found::Read {
            stamp: stamp(len),
            picture: Picture::Drawn(image.clone()),
        };
        let (one, two, three) = (drawn(), drawn(), drawn());
        let (a, b) = (Path::new("/r/a.svg"), Path::new("/r/b.svg"));
        let mut checks = ImageChecks {
            frame: 1,
            ..Default::default()
        };
        checks.record(vec![
            (a.to_owned(), svg(1, &one)),
            (b.to_owned(), svg(1, &two)),
        ]);
        assert!(checks.released.is_empty());
        // Replaced, and gone.
        checks.record(vec![
            (a.to_owned(), svg(2, &three)),
            (b.to_owned(), Found::Nothing),
        ]);
        let released = |checks: &mut ImageChecks| -> Vec<_> {
            checks.released.drain(..).map(|i| i.id).collect()
        };
        assert_eq!(released(&mut checks), [one.id, two.id]);
        // No longer drawn, a while ago.
        checks.frame = 2;
        checks.known.get_mut(a).unwrap().asked = Instant::now() - FORGET * 2;
        checks.forget();
        assert_eq!(released(&mut checks), [three.id]);
        // Closed: whatever is shown.
        checks.record(vec![(a.to_owned(), svg(3, &one))]);
        checks.clear();
        assert_eq!(released(&mut checks), [one.id]);
    }

    #[gpui::test]
    fn images_taken_out_are_let_go_without_a_frame(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "text\n").unwrap();
        let key = docs::DocKey {
            on: None,
            path: dir.path().join("a.md").to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| docs::open(key, cx));
        cx.run_until_parked();
        let doc = cx.update(|cx| docs::get(id, cx).unwrap());
        let colors = Colors::from_props(&HostValue::Null);
        let view = cx.new(|cx| MarkdownView::new(doc, "md".into(), colors, cx));
        let image = dir.path().join("x.png");
        view.update(cx, |v, cx| {
            let mut checks = v.images.lock().unwrap();
            checks.frame = 1;
            checks.answer(&image);
            checks.record(vec![(
                image.clone(),
                Found::Read {
                    stamp: Stamp {
                        len: 1,
                        mtime_ms: 1,
                    },
                    picture: Picture::Drawn(drawn()),
                },
            )]);
            // The last frame no longer drew it, a while ago.
            checks.frame = 2;
            checks.known.get_mut(&image).unwrap().asked = Instant::now() - FORGET * 2;
            drop(checks);
            v.forget_later(cx);
        });
        cx.executor().advance_clock(FORGET);
        cx.run_until_parked();
        view.update(cx, |v, _| {
            let checks = v.images.lock().unwrap();
            assert!(checks.known.is_empty() && !checks.forgetting);
            // Its texture dropped.
            assert!(checks.released.is_empty());
        });
    }

    #[gpui::test]
    fn a_closed_preview_releases_its_svgs(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "text\n").unwrap();
        let key = docs::DocKey {
            on: None,
            path: dir.path().join("a.md").to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| docs::open(key, cx));
        cx.run_until_parked();
        let doc = cx.update(|cx| docs::get(id, cx).unwrap());
        let colors = Colors::from_props(&HostValue::Null);
        let view = cx.new(|cx| MarkdownView::new(doc, "md".into(), colors, cx));
        let image = drawn();
        let images = view.update(cx, |v, _| {
            v.images.lock().unwrap().record(vec![(
                dir.path().join("x.svg"),
                Found::Read {
                    stamp: Stamp {
                        len: 1,
                        mtime_ms: 1,
                    },
                    picture: Picture::Drawn(image.clone()),
                },
            )]);
            v.images.clone()
        });
        drop(view);
        // Released once effects are flushed.
        cx.update(|_| {});
        let checks = images.lock().unwrap();
        assert!(checks.known.is_empty() && checks.released.is_empty());
        assert_eq!(Arc::strong_count(&image), 1);
        // Its SVG children killed.
        assert!(checks.cancel.is_cancelled());
    }

    #[gpui::test]
    fn a_missing_image_is_looked_for_again_unasked(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.md"), "![x](x.png)\n").unwrap();
        let key = docs::DocKey {
            on: None,
            path: dir.path().join("a.md").to_string_lossy().into_owned(),
        };
        let id = cx.update(|cx| docs::open(key, cx));
        cx.run_until_parked();
        let doc = cx.update(|cx| docs::get(id, cx).unwrap());
        let colors = Colors::from_props(&HostValue::Null);
        let view = cx.new(|cx| MarkdownView::new(doc, "md".into(), colors, cx));
        let redraws = std::rc::Rc::new(std::cell::Cell::new(0));
        let count = redraws.clone();
        cx.update(|cx| {
            cx.observe(&view, move |_, _| count.set(count.get() + 1))
                .detach()
        });
        // A frame drew the image before the file was there.
        let image = dir.path().join("x.png");
        view.update(cx, |v, cx| {
            v.images.lock().unwrap().answer(&image);
            v.recheck_later(cx);
        });
        cx.run_until_parked();
        assert_eq!(redraws.get(), 0);
        cx.executor().advance_clock(RECHECK);
        cx.run_until_parked();
        assert_eq!(redraws.get(), 1, "no redraw to look for it again");
        // No frame showed a missing image since: no more redraws.
        view.update(cx, |v, cx| v.recheck_later(cx));
        cx.executor().advance_clock(RECHECK * 3);
        cx.run_until_parked();
        assert_eq!(redraws.get(), 1);
    }

    #[test]
    fn only_web_and_mail_links_leave_the_app() {
        assert!(opens_outside("https://github.com"));
        assert!(opens_outside("mailto:a@b.c"));
        assert!(!opens_outside("file:///Applications/Calculator.app"));
        assert!(!opens_outside("./other.md"));
        assert!(!opens_outside("javascript:alert(1)"));
    }
}
