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
    AnyElement, App, Context, ElementId, Entity, FontWeight, Hsla, ImageSource, IntoElement,
    ParentElement as _, Render, SharedString, SharedUri, StyleRefinement, Styled as _,
    Subscription, Window, div, px,
};
use gpui_base::text::{TextView, TextViewStyle};
use yuzora_core::HostValue;

use crate::docs::{self, DocView};
use crate::highlight::{self, SyntaxPalette};

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
    let path = url.strip_prefix("file://").unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = percent_decode(path);
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
        }
    }
}

fn missing_image() -> ImageSource {
    ImageSource::from(PathBuf::new())
}

/// A regular file small enough to show: not a device, FIFO or folder.
fn loadable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= crate::image::MAX_IMAGE_BYTES)
}

/// A path's answer is looked at again after this long.
const RECHECK: Duration = Duration::from_secs(10);

/// Which of a preview's images may be shown. Drawing only reads it; the
/// files are looked at on a worker after the frame.
#[derive(Default)]
struct ImageChecks {
    /// Whether each path is [`loadable`], and when that was found.
    known: HashMap<PathBuf, (bool, Instant)>,
    /// Paths drawn without an answer, or with an old one.
    wanted: HashSet<PathBuf>,
    /// A check is queued or running.
    busy: bool,
}

impl ImageChecks {
    /// The answer for `path` so far (not shown while unknown); asks for a
    /// check when there is none or it is old.
    fn answer(&mut self, path: &Path) -> bool {
        match self.known.get(path) {
            Some(&(ok, at)) => {
                if at.elapsed() > RECHECK {
                    self.wanted.insert(path.to_owned());
                }
                ok
            }
            None => {
                self.wanted.insert(path.to_owned());
                false
            }
        }
    }

    /// Records answers; true when one changed what is shown.
    fn record(&mut self, answers: Vec<(PathBuf, bool)>) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for (path, ok) in answers {
            let before = self.known.insert(path, (ok, now)).map(|(was, _)| was);
            changed |= before != Some(ok);
        }
        changed
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
        let base = Self::base_of(&doc, cx);
        let remote = doc.read(cx).key.on.is_some();
        Self {
            doc,
            id,
            base,
            remote,
            colors,
            highlighter: highlight::code_block_highlighter(Arc::new(SyntaxPalette::one_dark())),
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
                let wanted: Vec<PathBuf> = {
                    let mut checks = images.lock().unwrap_or_else(|e| e.into_inner());
                    if checks.wanted.is_empty() {
                        checks.busy = false;
                        break;
                    }
                    checks.wanted.drain().collect()
                };
                let answers = docs::blocking(move || {
                    wanted
                        .into_iter()
                        .map(|path| {
                            let ok = loadable(&path);
                            (path, ok)
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
            if changed {
                let _ = this.update(cx, |_, cx| cx.notify());
            }
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
        self.check_images(cx);
        div().size_full().min_h_0().px(px(18.)).py(px(14.)).child(
            TextView::markdown(ElementId::Name(self.id.clone()), text)
                .style(self.style(cx))
                .shared_code_block_highlighter(self.highlighter.clone())
                .image_source(move |uri: &SharedUri| {
                    match image_path(uri.as_ref(), base.as_deref()) {
                        Some(path)
                            if !remote
                                && images
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .answer(&path) =>
                        {
                            ImageSource::from(path)
                        }
                        _ => missing_image(),
                    }
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
            v.images = Arc::default();
            cx.notify();
        }
        if v.colors != colors {
            v.colors = colors;
            cx.notify();
        }
    });
    view.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn only_regular_files_within_the_limit_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"not really a png").unwrap();
        assert!(loadable(&png));
        assert!(!loadable(dir.path()));
        assert!(!loadable(&dir.path().join("missing.png")));
        #[cfg(unix)]
        {
            assert!(!loadable(Path::new("/dev/zero")));
            let link = dir.path().join("zero.png");
            std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
            assert!(!loadable(&link));
        }
    }

    #[test]
    fn image_answers_wait_for_a_check_and_are_looked_at_again() {
        let mut checks = ImageChecks::default();
        let (a, b) = (Path::new("/r/a.png"), Path::new("/r/b.png"));
        assert!(!checks.answer(a));
        assert!(!checks.answer(b));
        assert_eq!(checks.wanted.len(), 2);
        checks.wanted.clear();
        assert!(checks.record(vec![(a.to_owned(), true), (b.to_owned(), false)]));
        assert!(checks.answer(a));
        assert!(!checks.answer(b));
        assert!(checks.wanted.is_empty());
        // The same answers again change nothing shown.
        assert!(!checks.record(vec![(a.to_owned(), true), (b.to_owned(), false)]));
        // An old answer is still used while it is checked again.
        checks
            .known
            .insert(a.to_owned(), (true, Instant::now() - RECHECK * 2));
        assert!(checks.answer(a));
        assert!(checks.wanted.contains(a));
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
