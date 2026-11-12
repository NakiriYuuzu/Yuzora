//! A line of a diff as one native element: `DiffLine.new(id, {text, diff, side, line, look, marks, sel})`.
//!
//! The line is shaped once, as one text in the font the rows set, and painted with the syntax its diff
//! session parsed ([`crate::syntax`]), the marks of what changed in it and the selection over it. Each
//! part is a range of the line as the file has it (tabs as tabs), and is drawn where the shaped text
//! has that place: what is drawn, what a mark covers and where a press lands are the same text.
//! A press is told back as a place in the line ([`hit`]) by the shaped text the last frame drew.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::{
    App, BorderStyle, Bounds, Element, ElementId, GlobalElementId, HighlightStyle, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, ShapedLine, SharedString, Style, TextRun, TextStyle, UnderlineStyle, Window, fill, point, px, quad, relative, transparent_black, underline_y_offset,
};
use gpui_base::input::HighlightStyleResolver;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;
use yuzora_core::host::from_host;
use yuzora_core::{HostModule, HostValue};
use yuzora_files::highlight::{SyntaxPalette, parse_hex};

use crate::diffview::{self, Side};
use crate::syntax::Span;

/// What a tab is drawn as: four spaces, as the diff has always shown it.
const TAB: &str = "    ";

/// The colours a diff's lines are drawn in: the editor's palette under the theme's colours, the marks of
/// removed and added text, and the selection.
pub struct Look {
    palette: SyntaxPalette,
    /// The palette's style of each name asked for: it looks a name up by each of its prefixes, and a line asks for each
    /// of its spans every frame.
    styles: Mutex<HashMap<&'static str, Option<HighlightStyle>>>,
    removed: Hsla,
    added: Hsla,
    selection: Hsla,
}

impl Look {
    fn of(v: &Value) -> Look {
        let color = |k: &str, or: u32| v.get(k).and_then(Value::as_str).and_then(parse_hex).unwrap_or_else(|| gpui::rgba(or).into());
        let overrides = v.get("syntax").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, c)| Some((k.clone(), parse_hex(c.as_str()?)?))).collect::<Vec<_>>()).unwrap_or_default();
        Look {
            palette: SyntaxPalette::one_dark().with_overrides(overrides),
            styles: Mutex::default(),
            removed: color("removed", 0xf08a8a55),
            added: color("added", 0x86d17a55),
            selection: color("selection", 0x4080ff44),
        }
    }

    fn style(&self, name: &'static str) -> Option<HighlightStyle> {
        *self.styles.lock().unwrap().entry(name).or_insert_with(|| self.palette.style(name))
    }
}

/// The looks asked for so far, each once: a script names one by its number in every line it draws.
fn looks() -> &'static Mutex<Vec<(String, Arc<Look>)>> {
    static LOOKS: OnceLock<Mutex<Vec<(String, Arc<Look>)>>> = OnceLock::new();
    LOOKS.get_or_init(Default::default)
}

/// The number of a look, `{syntax: {name: "#hex"}, removed, added, selection}`: the same for the same look.
pub fn look(v: &Value) -> usize {
    let key = v.to_string();
    let mut all = looks().lock().unwrap();
    if let Some(at) = all.iter().position(|(k, _)| *k == key) {
        return at;
    }
    all.push((key, Arc::new(Look::of(v))));
    all.len() - 1
}

fn look_of(at: Option<usize>) -> Arc<Look> {
    static PLAIN: OnceLock<Arc<Look>> = OnceLock::new();
    at.and_then(|at| looks().lock().unwrap().get(at).map(|(_, l)| l.clone())).unwrap_or_else(|| PLAIN.get_or_init(|| Arc::new(Look::of(&Value::Null))).clone())
}

/// A diff's id as a key, whatever a script wrote it as.
pub fn diff_key(v: Option<&HostValue>) -> String {
    match v {
        Some(HostValue::Number(n)) => n.to_string(),
        Some(HostValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Where a place of a line (a byte offset, tabs as tabs) is in the text drawn for it (tabs as [`TAB`]).
fn shown(tabs: &[usize], at: usize) -> usize {
    at + (TAB.len() - 1) * tabs.partition_point(|&t| t < at)
}

/// The place of `text` (a line as the file has it) nearest `x`, measured from where the line is drawn: the
/// boundary of a character as one reads it (a grapheme: a letter with its combining marks, an emoji with its
/// joiners), so a press never lands inside one. `glyphs` are where the drawn text's glyphs start, (index in the
/// drawn text, x) in order, and `width` where it ends. A place inside a ligature is drawn where the ligature
/// ends; of places drawn at the same x the later is taken, which keeps what a selection covers what it tints.
pub fn nearest(text: &str, glyphs: &[(usize, f32)], width: f32, x: f32) -> usize {
    let (mut best, mut gap) = (0, f32::INFINITY);
    let (mut g, mut tabs) = (0, 0);
    let ends = text.grapheme_indices(true).map(|(at, s)| (at, s == "\t")).chain(std::iter::once((text.len(), false)));
    for (at, tab) in ends {
        let drawn = at + (TAB.len() - 1) * tabs;
        while g < glyphs.len() && glyphs[g].0 < drawn {
            g += 1;
        }
        let gx = glyphs.get(g).map_or(width, |p| p.1);
        let d = (gx - x).abs();
        if d <= gap {
            (best, gap) = (at, d);
        } else if gx > x {
            break;
        }
        tabs += usize::from(tab);
    }
    best
}

/// A line the last frames drew: where it starts in the window, its shaped text, and the line as the file has it.
#[derive(Clone)]
struct Drawn {
    left: Pixels,
    shaped: ShapedLine,
    text: Arc<str>,
    tabs: Arc<[usize]>,
}

impl Drawn {
    fn glyphs(&self) -> Vec<(usize, f32)> {
        self.shaped.runs.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.index, g.position.x.as_f32()))).collect()
    }
}

type Key = (String, bool, usize);

/// The lines drawn lately, by diff, side and line. A frame draws the lines in view again, so two generations
/// of a few thousand keep every line on screen and forget the rest.
#[derive(Default)]
struct Hits {
    now: HashMap<Key, Drawn>,
    then: HashMap<Key, Drawn>,
}

const KEPT: usize = 4096;

thread_local! {
    // Windows are drawn, and their scripts run, on the thread that drew them: a press is answered there. Tests run
    // windows side by side, each on its own thread, with the same diff ids.
    static HITS: RefCell<Hits> = RefCell::default();
}

fn remember(key: Key, drawn: Drawn) {
    HITS.with_borrow_mut(|h| {
        if h.now.len() >= KEPT {
            h.then = std::mem::take(&mut h.now);
        }
        h.now.insert(key, drawn);
    });
}

fn drawn(diff: &str, right: bool, line: usize) -> Option<Drawn> {
    let key = (diff.to_owned(), right, line);
    HITS.with_borrow(|h| h.now.get(&key).or_else(|| h.then.get(&key)).cloned())
}

/// The place of line `line` (0-based) of a side of a diff that a press at `x` (window pixels) lands on, by the
/// text the line was last drawn in. None for a line not drawn lately.
pub fn hit(diff: &str, right: bool, line: usize, x: f32) -> Option<usize> {
    let d = drawn(diff, right, line)?;
    Some(nearest(&d.text, &d.glyphs(), d.shaped.width.as_f32(), x - d.left.as_f32()))
}

/// Where place `at` of a drawn line is in the window, across: where a press lands on it.
pub fn x_at(diff: &str, right: bool, line: usize, at: usize) -> Option<f32> {
    let d = drawn(diff, right, line)?;
    Some((d.left + d.shaped.x_for_index(shown(&d.tabs, at.min(d.text.len())))).as_f32())
}

/// `DiffLine`, `diffLook` and `diffHit` (see [`crate::DECLARATIONS`]) in a module.
pub fn register(m: HostModule) -> HostModule {
    m.component("DiffLine", |args, _window, _cx| element(args.props()).into_any_element())
        .function("diffLook", |a| Ok(HostValue::Number(look(&from_host(a.value(0)?)) as f64)))
        .function("diffHit", |a| {
            let at = hit(&diff_key(a.get(0)), a.string(1)? == "r", a.integer(2)?.max(0) as usize, a.number(3)? as f32);
            Ok(at.map_or(HostValue::Null, |at| HostValue::Number(at as f64)))
        })
}

/// One line's element.
pub struct DiffLine {
    text: Arc<str>,
    tabs: Arc<[usize]>,
    key: String,
    right: bool,
    line: usize,
    spans: Vec<Span>,
    look: Arc<Look>,
    marks: Vec<Range<usize>>,
    /// The selection over the line: from a place (none: from before its start) to a place (none: past its end).
    sel: Option<(Option<usize>, Option<usize>)>,
}

/// A place a script gave: within the line, on a character's boundary; none when it is not a number.
fn place(text: &str, v: Option<&HostValue>) -> Option<usize> {
    let n = v?.as_number()?;
    (n >= 0.0).then(|| text.floor_char_boundary((n as usize).min(text.len())))
}

/// The element for props `{text, diff, side: "l" | "r", line, look, marks: [[from, to]], sel: [from, to]}`. The text
/// is the line as the file has it; `diff` and `side` and `line` name it in its diff session, whose syntax colours it;
/// `marks` are ranges of it; `sel` runs from a place (below 0: from before the line) to a place (past the line: past its end).
pub fn element(props: &HostValue) -> DiffLine {
    let text: Arc<str> = props.get("text").and_then(HostValue::as_str).unwrap_or_default().into();
    let tabs: Arc<[usize]> = text.match_indices('\t').map(|(at, _)| at).collect();
    let right = props.get("side").and_then(HostValue::as_str) == Some("r");
    let line = props.get("line").and_then(HostValue::as_number).filter(|n| *n >= 0.0).unwrap_or(0.0) as usize;
    let diff = props.get("diff");
    let spans = match diff {
        Some(HostValue::Number(id)) => diffview::get(*id as u64).map(|s| s.line_spans(if right { Side::Right } else { Side::Left }, line)).unwrap_or_default(),
        _ => Vec::new(),
    };
    let marks = props
        .get("marks")
        .and_then(HostValue::as_array)
        .map(|m| {
            m.iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    let (from, to) = (place(&text, r.first())?, place(&text, r.get(1))?);
                    (from < to).then_some(from..to)
                })
                .collect()
        })
        .unwrap_or_default();
    let sel = props.get("sel").and_then(HostValue::as_array).filter(|s| s.len() == 2).map(|s| {
        let to = s[1].as_number().filter(|n| *n <= text.len() as f64).and_then(|_| place(&text, s.get(1)));
        (place(&text, s.first()), to)
    });
    let look = look_of(props.get("look").and_then(HostValue::as_number).map(|n| n as usize));
    DiffLine { key: diff_key(diff), text, tabs, right, line, spans, look, marks, sel }
}

/// A styled part of a line as it is drawn: where (in the drawn text), its colour, and its underline if it has one.
#[derive(Clone, Debug, PartialEq)]
struct Paint {
    at: Range<usize>,
    color: Hsla,
    underline: Option<UnderlineStyle>,
}

impl DiffLine {
    /// The text as it is drawn, the runs it is shaped in and the styled parts it is painted with (in order, apart). It
    /// is shaped as one run of the rows' style, cut only where a span's style changes the font (a comment's slant): a
    /// run for each colour would cut the line into as many pieces for the platform to shape, and each such line would
    /// be shaped again whenever it comes into view. The colours are given to the glyphs as they are painted.
    fn runs(&self, base: &TextStyle) -> (SharedString, Vec<TextRun>, Vec<Paint>) {
        let drawn: SharedString = if self.tabs.is_empty() { self.text.to_string().into() } else { self.text.replace('\t', TAB).into() };
        let len = self.text.len();
        let (mut runs, mut paints) = (Vec::<TextRun>::new(), Vec::new());
        let mut push = |from: usize, to: usize, style: Option<HighlightStyle>| {
            let at = shown(&self.tabs, from)..shown(&self.tabs, to);
            let slant = style.filter(|s| s.font_weight.is_some() || s.font_style.is_some());
            let font = slant.map_or_else(|| base.font(), |s| base.clone().highlight(HighlightStyle { font_weight: s.font_weight, font_style: s.font_style, ..Default::default() }).font());
            match runs.last_mut() {
                Some(run) if run.font == font => run.len += at.len(),
                _ => runs.push(TextRun { font, ..base.to_run(at.len()) }),
            }
            if let Some(s) = style.filter(|s| s.color.is_some() || s.underline.is_some()) {
                paints.push(Paint { at, color: s.color.unwrap_or(base.color), underline: s.underline });
            }
        };
        let mut at = 0;
        for (r, name) in &self.spans {
            let (from, to) = (r.start.min(len), r.end.min(len));
            if from < at || from >= to || !self.text.is_char_boundary(from) || !self.text.is_char_boundary(to) {
                continue;
            }
            if from > at {
                push(at, from, None);
            }
            push(from, to, self.look.style(name));
            at = to;
        }
        if at < len {
            push(at, len, None);
        }
        (drawn, runs, paints)
    }

    fn x(&self, shaped: &ShapedLine, left: Pixels, at: usize) -> Pixels {
        left + shaped.x_for_index(shown(&self.tabs, at))
    }
}

/// What a line's layout leaves for its paint: the shaped text, the height of a line of it, the rows' colour and the
/// styled parts.
pub struct Shaped {
    shaped: ShapedLine,
    line_height: Pixels,
    color: Hsla,
    paints: Vec<Paint>,
}

/// Paints the glyphs of `shaped` with the line's top left at `origin`, as gpui paints a line: each glyph in the colour
/// of the part it is in (`color` elsewhere), then the underlines; a glyph outside what is shown is not painted.
fn paint_text(shaped: &ShapedLine, origin: Point<Pixels>, line_height: Pixels, color: Hsla, paints: &[Paint], window: &mut Window, cx: &App) {
    let (ascent, descent, font_size) = (shaped.ascent, shaped.descent, shaped.font_size);
    let baseline = (line_height - ascent - descent) / 2. + ascent;
    let clip = window.content_mask().bounds;
    window.paint_layer(Bounds { origin, size: gpui::size(shaped.width, line_height) }, |window| {
        for run in &shaped.runs {
            let glyph = cx.text_system().bounding_box(run.font_id, font_size).size;
            for g in &run.glyphs {
                let at = point(origin.x + g.position.x, origin.y);
                if !(Bounds { origin: at, size: glyph }).intersects(&clip) {
                    continue;
                }
                let at = at + point(px(0.), baseline + g.position.y);
                let color = paint_at(paints, g.index).map_or(color, |p| p.color);
                let _ = if g.is_emoji { window.paint_emoji_with_color(at, run.font_id, g.id, font_size, color) } else { window.paint_glyph(at, run.font_id, g.id, font_size, color) };
            }
        }
        for p in paints {
            if let Some(u) = p.underline {
                let (x0, x1) = (shaped.x_for_index(p.at.start), shaped.x_for_index(p.at.end));
                window.paint_underline(point(origin.x + x0, origin.y + underline_y_offset(line_height, ascent, descent)), x1 - x0, &UnderlineStyle { color: Some(u.color.unwrap_or(p.color)), ..u });
            }
        }
    });
}

/// The styled part a place of the drawn text is in.
fn paint_at(paints: &[Paint], at: usize) -> Option<&Paint> {
    paints.get(paints.partition_point(|p| p.at.end <= at)).filter(|p| p.at.start <= at)
}

impl IntoElement for DiffLine {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for DiffLine {
    type RequestLayoutState = Shaped;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    /// As wide as the shaped text (rounded up to a pixel, as a text element is) and as tall as what holds it: the
    /// selection covers the row, the text is in its middle.
    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, Shaped) {
        let base = window.text_style();
        let font_size = base.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(base.line_height.to_pixels(font_size.into(), window.rem_size()));
        let (drawn, runs, paints) = self.runs(&base);
        let shaped = window.text_system().shape_line(drawn, font_size, &runs, None);
        let mut style = Style::default();
        style.size.width = shaped.width.ceil().into();
        style.size.height = relative(1.).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), Shaped { shaped, line_height, color: base.color, paints })
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, _: Bounds<Pixels>, _: &mut Shaped, _: &mut Window, _: &mut App) {}

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, state: &mut Shaped, _: &mut (), window: &mut Window, cx: &mut App) {
        let Shaped { shaped, line_height, color: text, paints } = state;
        let clip = window.content_mask().bounds;
        let left = bounds.left();
        let top = bounds.top() + ((bounds.size.height - *line_height) * 0.5).max(px(0.));
        if let Some((from, to)) = self.sel {
            let x0 = from.map_or(clip.left().min(left), |at| self.x(shaped, left, at));
            let x1 = to.map_or(clip.right().max(bounds.right()), |at| self.x(shaped, left, at));
            if x0 < x1 {
                window.paint_quad(fill(Bounds::from_corners(point(x0, bounds.top()), point(x1, bounds.bottom())), self.look.selection));
            }
        }
        let color = if self.right { self.look.added } else { self.look.removed };
        let radius = shaped.font_size * (1. / 6.);
        for m in &self.marks {
            let (x0, x1) = (self.x(shaped, left, m.start), self.x(shaped, left, m.end));
            window.paint_quad(quad(Bounds::from_corners(point(x0, top), point(x1, top + *line_height)), radius, color, px(0.), transparent_black(), BorderStyle::default()));
        }
        paint_text(shaped, point(left, top), *line_height, *text, paints, window, cx);
        if bounds.intersects(&clip) {
            remember((self.key.clone(), self.right, self.line), Drawn { left, shaped: shaped.clone(), text: self.text.clone(), tabs: self.tabs.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Glyphs `w` apart, one for each character of the drawn text (tabs as four spaces), with the advance each takes.
    fn glyphs(drawn: &str, advance: impl Fn(char) -> f32) -> (Vec<(usize, f32)>, f32) {
        let mut x = 0.0;
        let mut out = Vec::new();
        for (at, c) in drawn.char_indices() {
            out.push((at, x));
            x += advance(c);
        }
        (out, x)
    }

    fn press(text: &str, advance: impl Fn(char) -> f32, x: f32) -> usize {
        let (g, w) = glyphs(&text.replace('\t', TAB), advance);
        nearest(text, &g, w, x)
    }

    #[test]
    fn a_press_lands_on_the_nearest_place_between_ascii_characters() {
        let cell = |_| 7.0;
        assert_eq!(press("let a", cell, -20.0), 0, "left of the line: its start");
        assert_eq!(press("let a", cell, 3.0), 0);
        assert_eq!(press("let a", cell, 4.0), 1, "past half a character: after it");
        assert_eq!(press("let a", cell, 20.0), 3);
        assert_eq!(press("let a", cell, 500.0), 5, "right of the line: its end");
        assert_eq!(press("", cell, 30.0), 0);
    }

    #[test]
    fn a_tab_is_one_place_drawn_four_cells_wide() {
        let cell = |_| 7.0;
        // "\tx": the tab takes 0..28, `x` 28..35.
        assert_eq!(press("\tx", cell, 13.0), 0, "the tab's first half");
        assert_eq!(press("\tx", cell, 15.0), 1, "its second half: after the tab, one byte in");
        assert_eq!(press("\tx", cell, 33.0), 2);
        assert_eq!(press("a\t\tb", cell, 7.0 + 28.0 + 15.0), 3, "past the second tab's middle");
    }

    #[test]
    fn a_wide_character_or_an_emoji_is_never_split() {
        let advance = |c: char| if c.is_ascii() { 7.0 } else { 12.0 };
        // `漢` is three bytes, one character: a press near its middle is before or after it.
        assert_eq!(press("a漢b", advance, 7.0 + 5.0), 1);
        assert_eq!(press("a漢b", advance, 7.0 + 7.0), 4);
        // An emoji made of a person, a joiner and a laptop is one grapheme of eleven bytes, three glyphs here.
        let coder = "\u{1f9d1}\u{200d}\u{1f4bb}";
        let line = format!("x{coder}y");
        let past = press(&line, advance, 7.0 + 12.0 * 2.0);
        assert_eq!(past, 1 + coder.len(), "after the whole emoji, never between its parts");
        assert_eq!(press(&line, advance, 7.0 + 1.0), 1);
    }

    #[test]
    fn a_combining_mark_stays_with_its_letter() {
        // `e` and a combining acute (two bytes), the mark drawn with no advance.
        let advance = |c: char| if c == '\u{301}' { 0.0 } else { 7.0 };
        assert_eq!(press("e\u{301}x", advance, 6.0), 3, "after the letter and its mark");
        assert_eq!(press("e\u{301}x", advance, 2.0), 0);
    }

    #[test]
    fn a_place_inside_a_ligature_is_where_the_ligature_ends() {
        // `->` drawn as one glyph two cells wide: no glyph starts at the `>`.
        let g = vec![(0, 0.0), (1, 7.0), (3, 21.0)];
        assert_eq!(nearest("a->b", &g, 28.0, 21.0), 3, "the ligature's end is the later of the places drawn there");
        assert_eq!(nearest("a->b", &g, 28.0, 8.0), 1);
    }

    #[test]
    fn a_place_given_is_on_a_characters_boundary_within_the_line() {
        assert_eq!(place("a漢", Some(&HostValue::Number(2.0))), Some(1));
        assert_eq!(place("a漢", Some(&HostValue::Number(99.0))), Some(4));
        assert_eq!(place("a漢", Some(&HostValue::Number(-1.0))), None);
        assert_eq!(place("a漢", None), None);
        assert_eq!(shown(&[1, 3], 4), 4 + 6, "two tabs before: six cells more");
    }

    /// The diff colours code by the theme's colours as the editor does: its table of them and the function that reads it
    /// are the editor's own.
    #[test]
    fn the_diff_mod_names_the_themes_colours_as_the_editor_mod_does() {
        let read = |mod_file: &str| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults/mods").join(mod_file)).unwrap();
        let (editor, diff) = (read("yuzora-editor/doc-pane.js"), read("yuzora-git-diff/main.js"));
        let part = |src: &str, from: &str, to: &str| -> String {
            let at = src.find(from).unwrap_or_else(|| panic!("no {from:?}"));
            let end = src[at..].find(to).unwrap_or_else(|| panic!("no end of {from:?}"));
            src[at..at + end].to_owned()
        };
        for (from, to) in [("\nconst SYNTAX = {", "\n};\n"), ("\nfunction syntax(", "\n}\n")] {
            assert_eq!(part(&diff, from, to), part(&editor, from, to));
        }
    }

    #[test]
    fn a_look_is_numbered_once() {
        let a = serde_json::json!({"syntax": {"keyword": "#ff0000"}, "removed": "#ff000055", "added": "#00ff0055", "selection": "#0000ff44"});
        let b = serde_json::json!({"syntax": {"keyword": "#00ff00"}});
        assert_eq!(look(&a), look(&a.clone()));
        assert_ne!(look(&a), look(&b));
        let red = look_of(Some(look(&a)));
        assert_eq!(red.palette.style("keyword").and_then(|s| s.color), parse_hex("#ff0000"));
        assert_eq!(red.removed, parse_hex("#ff000055").unwrap());
    }

    /// A line is drawn as one text: its tabs four spaces, each span in the look's colour for its name, the rest in the rows'.
    #[test]
    fn a_line_is_one_text_its_spans_in_the_looks_colours() {
        let red = serde_json::json!({"syntax": {"keyword": "#ff0000", "number": "#00ff00"}});
        let text: Arc<str> = "\tlet a = 1; ".into();
        let line = DiffLine {
            tabs: text.match_indices('\t').map(|(at, _)| at).collect(),
            text,
            key: String::new(),
            right: false,
            line: 0,
            spans: vec![(1..4, "keyword"), (9..10, "number"), (11..40, "keyword")],
            look: look_of(Some(look(&red))),
            marks: Vec::new(),
            sel: None,
        };
        let base = TextStyle::default();
        let (drawn, runs, paints) = line.runs(&base);
        assert_eq!(&*drawn, "    let a = 1; ");
        assert_eq!(runs.iter().map(|r| (r.len, r.color)).collect::<Vec<_>>(), [(15, base.color)], "shaped as one run, in one colour");
        let (k, n) = (parse_hex("#ff0000").unwrap(), parse_hex("#00ff00").unwrap());
        let parts: Vec<_> = paints.iter().map(|p| (p.at.clone(), p.color)).collect();
        assert_eq!(parts, [(4..7, k), (12..13, n), (14..15, k)], "`let` after the tab drawn four wide, `1`, and the span past the line's end clipped to it");
        assert_eq!((paint_at(&paints, 3), paint_at(&paints, 6).map(|p| p.color), paint_at(&paints, 7)), (None, Some(k), None));
    }

    /// A comment is slanted, as in the editor: the line is shaped in two fonts, and only there is it cut.
    #[test]
    fn a_line_is_shaped_in_runs_only_where_its_font_changes() {
        let text: Arc<str> = "let a = 1; // one".into();
        let line = DiffLine {
            tabs: Arc::from([]),
            text,
            key: String::new(),
            right: false,
            line: 0,
            spans: vec![(0..3, "keyword"), (8..9, "number"), (11..17, "comment")],
            look: look_of(None),
            marks: Vec::new(),
            sel: None,
        };
        let (_, runs, paints) = line.runs(&TextStyle::default());
        assert_eq!(runs.iter().map(|r| (r.len, r.font.style)).collect::<Vec<_>>(), [(11, gpui::FontStyle::Normal), (6, gpui::FontStyle::Italic)]);
        assert_eq!(paints.len(), 3);
    }
}
