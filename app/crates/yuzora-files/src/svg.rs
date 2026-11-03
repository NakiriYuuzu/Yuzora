//! A repo's SVGs, for the image pane and the Markdown preview: checked and
//! rasterized here, on a thread of our own, not by GPUI's image decoder (a
//! small stack on a shared queue, and nothing bounding what a file asks of
//! it). The tree is GPUI's own parse (its fonts, and an `<image>` that shows
//! nothing: vendored gpui-pre), so it draws as GPUI would.
//!
//! What an SVG here can't do:
//! - come gzip-compressed (SVGZ): usvg inflates it whole before anything;
//! - nest elements deeper than [`MAX_DEPTH`], or declare entities that hold
//!   markup or references (expanded, they nest or grow without showing);
//! - refer in a loop (a clip, mask, pattern, marker, filter or `<use>` that
//!   leads back to itself), or so that usvg's conversion goes deeper than
//!   [`MAX_REACH`]: usvg recurses into each reference and breaks only loops
//!   of one or two;
//! - make more than [`MAX_NODES`] nodes: `<use>`s expanded, a marker's once
//!   per vertex it is drawn at, text a node a character, and what is drawn
//!   again for each use (a pattern, clip or mask) counted each time;
//! - have more than [`MAX_LAYERS`] layers inside one another;
//! - take more than [`BUDGET`] of pixels at once, or paint more than [`WORK`]:
//!   resvg gives each layer (a group with opacity, a filter, mask, clip or
//!   blend) a buffer of its box, up to 5×5 canvases; a clip or mask one more
//!   each, chained ones each; a filter one per primitive; a pattern a tile of
//!   its own size. The canvas is drawn smaller (never above [`MAX_EDGE`]) to
//!   fit, and not below [`MIN_EDGE`].

use std::cell::Cell;
use std::sync::Arc;

use gpui::{RenderImage, SMOOTH_SVG_SCALE_FACTOR, SvgRenderer, SvgSize};
use usvg::roxmltree;

/// Deepest element nesting in the markup.
pub const MAX_DEPTH: usize = 128;
/// Deepest usvg's conversion goes: nesting, and each reference followed.
pub const MAX_REACH: usize = 512;
/// Nodes made and drawn (see the module's notes).
pub const MAX_NODES: usize = 100_000;
/// Layers inside one another.
pub const MAX_LAYERS: usize = 16;
/// Longest side drawn, in pixels.
pub const MAX_EDGE: f32 = 4096.;
/// Shortest the longest side is drawn at to fit the budgets.
pub const MIN_EDGE: f32 = 16.;
/// Bytes of pixels held at once while drawing, the canvas's included.
pub const BUDGET: u64 = 256 << 20;
/// Pixels painted (each path's box, each layer cleared and composited, each
/// filter pass), about a second's work.
pub const WORK: f64 = (1u64 << 30) as f64;
/// Style sheets' text, and the steps matching their selectors may take (usvg
/// matches each rule against each element too).
const MAX_STYLE: usize = 1 << 20;
const CSS_STEPS: u64 = 1 << 24;
/// The thread's stack: usvg and resvg recurse per level, and [`MAX_REACH`]
/// levels fit in it many times over.
const STACK: usize = 64 << 20;

/// Rasterizes `bytes` as GPUI would at twice its size, within the limits
/// above. Blocks until done. Err: a key under `image.*` the panes show.
pub fn rasterize(renderer: &SvgRenderer, bytes: Vec<u8>) -> Result<Arc<RenderImage>, String> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err("svgz".into());
    }
    check_markup(&bytes)?;
    let renderer = renderer.clone();
    std::thread::Builder::new()
        .name("yuzora-svg".into())
        .stack_size(STACK)
        .spawn(move || {
            check_references(&bytes)?;
            let parsed = renderer.parse_svg(&bytes).map_err(|e| e.to_string())?;
            let scale = scale_within_budget(parsed.tree())?;
            renderer
                .render_parsed(&parsed, SvgSize::ScaleFactor(scale))
                .map_err(|e| e.to_string())
        })
        .map_err(|e| e.to_string())?
        .join()
        .unwrap_or_else(|_| Err("svg_failed".into()))
}

/// Properties whose `url(#…)` usvg follows, inherited ones (by index).
const INHERITED: [&str; 5] = ["fill", "stroke", "marker-start", "marker-mid", "marker-end"];
const NOT_INHERITED: [&str; 3] = ["clip-path", "mask", "filter"];
/// Elements whose `href="#…"` usvg follows.
const FOLLOWS_HREF: [&str; 8] = [
    "use",
    "feImage",
    "pattern",
    "linearGradient",
    "radialGradient",
    "filter",
    "textPath",
    "tref",
];
/// What is filled or stroked, and of those what markers are drawn on.
const GRAPHICS: [&str; 10] = [
    "path", "line", "polyline", "polygon", "rect", "circle", "ellipse", "text", "tspan", "textPath",
];
const SHAPES: [&str; 4] = ["path", "line", "polyline", "polygon"];
const TEXT: [&str; 4] = ["text", "tspan", "textPath", "tref"];

/// One element, as far as usvg's conversion goes.
#[derive(Default)]
struct Element {
    kids: Vec<usize>,
    /// What it refers to, each with how many nodes per node of it are made.
    refs: Vec<(usize, u64)>,
    /// A `<use>`'s element, and what its copy inherits from it.
    used: Option<usize>,
    passed: Vec<usize>,
    /// Itself and its text's characters.
    weight: u64,
    /// Where markers go on it, plus its ends: a shape's vertices, 0 for
    /// other graphics, none for the rest.
    points: Option<u64>,
}

/// What converting an element takes: how deep it goes, the nodes it makes,
/// and its graphics' points (for markers a `<use>` passes on).
#[derive(Clone, Copy)]
struct Reach {
    depth: usize,
    nodes: u64,
    points: u64,
}

enum Visit {
    New,
    Open,
    Done(Reach),
}

/// Refuses an SVG whose references loop, or that usvg would convert deeper
/// than [`MAX_REACH`] or into more than [`MAX_NODES`] nodes. Markup that
/// doesn't parse is left to usvg, which refuses it the same way.
fn check_references(xml: &[u8]) -> Result<(), String> {
    let Ok(text) = std::str::from_utf8(xml) else {
        return Ok(());
    };
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: 4 * MAX_NODES as u32,
        ..Default::default()
    };
    let doc = match roxmltree::Document::parse_with_options(text, options) {
        Ok(doc) => doc,
        Err(roxmltree::Error::NodesLimitReached) => return Err("svg_too_complex".into()),
        Err(_) => return Ok(()),
    };
    let elements = references(&doc)?;
    let mut visits: Vec<Visit> = (0..elements.len()).map(|_| Visit::New).collect();
    let root = doc.root_element().id().get_usize();
    let reach = visit(&elements, root, 0, &mut visits)?;
    if reach.depth > MAX_REACH {
        return Err("svg_too_deep".into());
    }
    Ok(())
}

fn visit(
    elements: &[Element],
    at: usize,
    depth: usize,
    visits: &mut [Visit],
) -> Result<Reach, String> {
    match visits[at] {
        Visit::Done(reach) => return Ok(reach),
        // A loop.
        Visit::Open => return Err("svg_too_deep".into()),
        Visit::New if depth > MAX_REACH => return Err("svg_too_deep".into()),
        Visit::New => visits[at] = Visit::Open,
    }
    let e = &elements[at];
    let mut reach = Reach {
        depth: 0,
        nodes: e.weight,
        points: e.points.map_or(0, |p| p + 2),
    };
    for &kid in &e.kids {
        let r = visit(elements, kid, depth + 1, visits)?;
        reach.depth = reach.depth.max(r.depth);
        reach.nodes = reach.nodes.saturating_add(r.nodes);
        reach.points = reach.points.saturating_add(r.points);
    }
    for &(to, times) in &e.refs {
        let r = visit(elements, to, depth + 1, visits)?;
        reach.depth = reach.depth.max(r.depth);
        reach.nodes = reach.nodes.saturating_add(times.saturating_mul(r.nodes));
    }
    if let Some(used) = e.used {
        let copy = visit(elements, used, depth + 1, visits)?;
        // The copy's graphics take what the `<use>` passes on, each of them
        // (a marker at each point).
        let (mut deeper, mut passed) = (0, 0u64);
        for &to in &e.passed {
            let r = visit(elements, to, depth + 1, visits)?;
            deeper = deeper.max(r.depth);
            passed = passed.saturating_add(r.nodes);
        }
        reach.depth = reach.depth.max(copy.depth + deeper);
        reach.nodes = reach
            .nodes
            .saturating_add(copy.nodes)
            .saturating_add(copy.points.saturating_mul(passed));
        reach.points = reach.points.saturating_add(copy.points);
    }
    if reach.nodes > MAX_NODES as u64 {
        return Err("svg_too_complex".into());
    }
    reach.depth += 1;
    visits[at] = Visit::Done(reach);
    Ok(reach)
}

/// Each element's children and references, as usvg resolves them: from its
/// attributes, its `style`, and the style sheets' rules that match it; fill,
/// stroke and markers inherited from its parent when it sets none.
fn references(doc: &roxmltree::Document) -> Result<Vec<Element>, String> {
    let mut ids = std::collections::HashMap::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = node.attribute("id") {
            ids.entry(id).or_insert(node.id().get_usize());
        }
    }
    let mut sheet = simplecss::StyleSheet::new();
    let mut style = 0;
    for node in doc.descendants().filter(|n| n.has_tag_name("style")) {
        if node.attribute("type").is_some_and(|t| t != "text/css") {
            continue;
        }
        let text = node.text().unwrap_or_default();
        style += text.len();
        if style > MAX_STYLE {
            return Err("svg_too_complex".into());
        }
        sheet.parse_more(text);
    }
    let steps = Cell::new(CSS_STEPS);
    let len = doc.descendants().count();
    let mut elements: Vec<Element> = (0..len).map(|_| Element::default()).collect();
    // What each element's fill, stroke and markers refer to, as it inherits
    // them (one `url` each, as usvg reads them).
    let mut inherited: Vec<[Option<usize>; 5]> = vec![[None; 5]; len];
    for node in doc.descendants().filter(|n| n.is_element()) {
        let at = node.id().get_usize();
        let tag = node.tag_name().name();
        // Each inherited property as set (inherit: `None`), and whether by an
        // `!important` declaration.
        let mut own: [Option<(Option<Option<usize>>, bool)>; 5] = [None; 5];
        let mut refs = Vec::new();
        let mut set = |name: &str, value: &str, important: bool| {
            let target = |id: &str| ids.get(id).copied();
            let names: &[&str] = if name == "marker" {
                &INHERITED[2..]
            } else {
                std::slice::from_ref(&name)
            };
            for name in names {
                if let Some(i) = INHERITED.iter().position(|p| p == name) {
                    // usvg's order: a later one wins unless the one before was important.
                    if own[i].is_some_and(|(_, kept)| kept) {
                        continue;
                    }
                    let value =
                        (value.trim() != "inherit").then(|| urls(value).next().and_then(target));
                    own[i] = Some((value, important));
                } else if *name == "filter" {
                    refs.extend(urls(value).filter_map(target));
                } else if NOT_INHERITED.contains(name) {
                    refs.extend(urls(value).next().and_then(target));
                }
            }
        };
        // Presentation attributes are the ones usvg reads: no other namespace.
        let svg_ns = |a: &roxmltree::Attribute| {
            a.namespace().is_none_or(|ns| {
                [
                    "http://www.w3.org/2000/svg",
                    "http://www.w3.org/1999/xlink",
                    "http://www.w3.org/XML/1998/namespace",
                ]
                .contains(&ns)
            })
        };
        for attr in node.attributes().filter(svg_ns) {
            set(attr.name(), attr.value(), false);
        }
        // simplecss sorts the rules by specificity, as usvg applies them.
        for rule in &sheet.rules {
            if rule.selector.matches(&Matched(node, &steps)) {
                for d in &rule.declarations {
                    set(d.name, d.value, d.important);
                }
            }
        }
        if steps.get() == 0 {
            return Err("svg_too_complex".into());
        }
        if let Some(style) = node.attribute("style") {
            for d in simplecss::DeclarationTokenizer::from(style) {
                set(d.name, d.value, d.important);
            }
        }
        let parent = node.parent_element().map(|p| p.id().get_usize());
        let mine: [Option<usize>; 5] = std::array::from_fn(|i| match own[i] {
            Some((Some(target), _)) => target,
            _ => parent.and_then(|p| inherited[p][i]),
        });
        let e = &mut elements[at];
        let graphics = GRAPHICS.contains(&tag);
        e.points = if SHAPES.contains(&tag) {
            Some(points(&node))
        } else {
            graphics.then_some(0)
        };
        e.refs = refs.into_iter().map(|to| (to, 1)).collect();
        for (i, target) in mine.iter().enumerate() {
            // Converted at each graphic that fills or strokes with it, and a
            // marker at each of its points; the edges stay for the depth.
            let times = match (i, e.points) {
                (0 | 1, _) if graphics => 1,
                (2.., Some(points)) => points + 2,
                _ => 0,
            };
            e.refs.extend(target.map(|to| (to, times)));
        }
        let href = node
            .attributes()
            .filter(|a| a.name() == "href")
            .find_map(|a| a.value().trim().strip_prefix('#'))
            .filter(|_| FOLLOWS_HREF.contains(&tag))
            .and_then(|id| ids.get(id).copied());
        match href {
            Some(to) if tag == "use" => {
                e.used = Some(to);
                e.passed = mine.iter().flatten().copied().collect();
            }
            Some(to) => e.refs.push((to, 1)),
            None => {}
        }
        let chars: usize = if TEXT.contains(&tag) {
            node.children()
                .filter(|c| c.is_text())
                .map(|c| c.text().unwrap_or_default().chars().count())
                .sum()
        } else {
            0
        };
        e.weight = 1 + chars as u64;
        e.kids = node
            .children()
            .filter(|c| c.is_element())
            .map(|c| c.id().get_usize())
            .collect();
        inherited[at] = mine;
    }
    Ok(elements)
}

/// The `#id`s of each `url(…)` in a property's value.
fn urls(value: &str) -> impl Iterator<Item = &str> {
    value.match_indices("url(").filter_map(move |(at, _)| {
        let rest = value[at + 4..].trim_start().trim_start_matches(['"', '\'']);
        let rest = rest.strip_prefix('#')?;
        let end = rest
            .find(|c: char| matches!(c, ')' | '"' | '\'') || c.is_whitespace())
            .unwrap_or(rest.len());
        Some(&rest[..end])
    })
}

/// At most how many vertices a shape has: its numbers' runs of digits halved,
/// and its commands.
fn points(node: &roxmltree::Node) -> u64 {
    let data = match node.tag_name().name() {
        "path" => node.attribute("d"),
        "polyline" | "polygon" => node.attribute("points"),
        _ => return 2,
    };
    let (mut numbers, mut commands, mut in_number) = (0u64, 0u64, false);
    for b in data.unwrap_or_default().bytes() {
        let digit = b.is_ascii_digit();
        numbers += u64::from(digit && !in_number);
        commands += u64::from(b.is_ascii_alphabetic());
        in_number = digit;
    }
    numbers / 2 + commands
}

/// An element for simplecss's matching (as usvg's), each step counted:
/// none left, nothing more matches.
struct Matched<'a, 'input>(roxmltree::Node<'a, 'input>, &'a Cell<u64>);

impl Matched<'_, '_> {
    fn step(&self) -> bool {
        let left = self.1.get();
        self.1.set(left.saturating_sub(1));
        left > 0
    }
}

impl simplecss::Element for Matched<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.step()
            .then(|| self.0.parent_element())
            .flatten()
            .map(|n| Matched(n, self.1))
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.step()
            .then(|| self.0.prev_sibling_element())
            .flatten()
            .map(|n| Matched(n, self.1))
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.step() && self.0.tag_name().name() == name
    }

    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator) -> bool {
        self.step() && self.0.attribute(name).is_some_and(|v| operator.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass) -> bool {
        self.step()
            && matches!(class, simplecss::PseudoClass::FirstChild)
            && self.0.prev_sibling_element().is_none()
    }
}

/// What drawing a tree takes, as resvg draws it: nodes and layers as counted
/// above, and the pixels painted.
#[derive(Default)]
struct Cost {
    nodes: usize,
    layers: usize,
    work: f64,
}

/// Pixels across and down.
type Px = (f64, f64);

/// A walk through a tree drawn on a canvas: no layer is larger than `limit`
/// (resvg's 5×5 canvases).
struct Walk {
    limit: Px,
    cost: Cost,
}

impl Walk {
    /// `rect`'s box drawn with `ts`, cut to `within`.
    fn cut(rect: Option<usvg::Rect>, within: Px) -> Px {
        rect.map_or((0., 0.), |r| {
            (
                (f64::from(r.width()) + 4.).min(within.0),
                (f64::from(r.height()) + 4.).min(within.1),
            )
        })
    }

    fn over(&self) -> bool {
        self.cost.nodes > MAX_NODES || self.cost.layers > MAX_LAYERS
    }

    fn deeper(&mut self, layers: usize) -> usize {
        self.cost.layers = self.cost.layers.max(layers + 1);
        layers + 1
    }

    /// Draws `group`'s children with `ts` on a target of `within` pixels;
    /// the pixels it holds meanwhile, at most.
    fn children(
        &mut self,
        group: &usvg::Group,
        ts: usvg::Transform,
        within: Px,
        layers: usize,
    ) -> f64 {
        let mut held = 0f64;
        for node in group.children() {
            self.cost.nodes += 1;
            if self.over() {
                break;
            }
            held = held.max(match node {
                usvg::Node::Group(g) => self.group(g, ts, within, layers),
                usvg::Node::Path(p) => self.path(p, ts, within, layers),
                usvg::Node::Text(t) => self.group(t.flattened(), ts, within, layers),
                // None comes from gpui-pre's resolver; a buffer of the target if one did.
                usvg::Node::Image(_) => {
                    self.cost.work += within.0 * within.1;
                    within.0 * within.1
                }
            });
        }
        held
    }

    fn group(&mut self, g: &usvg::Group, ts: usvg::Transform, within: Px, layers: usize) -> f64 {
        let ts = ts.pre_concat(g.transform());
        if !g.should_isolate() {
            return self.children(g, ts, within, layers);
        }
        let layers = self.deeper(layers);
        let layer = Self::cut(
            g.layer_bounding_box().transform(ts).map(|r| r.to_rect()),
            self.limit,
        );
        let px = layer.0 * layer.1;
        // Cleared, then drawn onto the target.
        self.cost.work += 2. * px;
        let mut held = self.children(g, ts, layer, layers);
        for filter in g.filters() {
            held = held.max(self.filter(filter, ts, layer, layers));
        }
        if let Some(clip) = g.clip_path() {
            held = held.max(self.clip(clip, ts, layer, layers));
        }
        if let Some(mask) = g.mask() {
            held = held.max(self.mask(mask, ts, layer, layers));
        }
        px + held
    }

    fn path(&mut self, p: &usvg::Path, ts: usvg::Transform, within: Px, layers: usize) -> f64 {
        let painted = Self::cut(p.stroke_bounding_box().transform(ts), within);
        if let Some(dashes) = p.stroke().and_then(|s| s.dasharray()) {
            // Cut into pieces (tiny-skia stops at a million) at any scale,
            // each worth some pixels' work; along its control points, never
            // shorter than the path.
            let period: f32 = dashes.iter().sum();
            let length: f32 = p
                .data()
                .points()
                .windows(2)
                .map(|w| w[0].distance(w[1]))
                .sum();
            let pieces = f64::from(length / period.max(f32::MIN_POSITIVE)).min(1e6);
            self.cost.work += 64. * pieces;
        }
        let paints = [p.fill().map(|f| f.paint()), p.stroke().map(|s| s.paint())];
        let mut held = 0f64;
        for paint in paints.into_iter().flatten() {
            self.cost.work += painted.0 * painted.1;
            let usvg::Paint::Pattern(pattern) = paint else {
                continue;
            };
            // A tile of its own size, whatever the canvas's.
            let (sx, sy) = ts.pre_concat(pattern.transform()).get_scale();
            let rect = pattern.rect();
            let tile = (
                f64::from((rect.width() * sx).round()),
                f64::from((rect.height() * sy).round()),
            );
            let px = tile.0 * tile.1;
            self.cost.work += px;
            let layers = self.deeper(layers);
            let inside = self.children(
                pattern.root(),
                usvg::Transform::from_scale(sx, sy),
                tile,
                layers,
            );
            held = held.max(px + inside);
        }
        held
    }

    /// A clip: a buffer of the layer and its mask (a byte a pixel), the clip
    /// of the clip applied while they're held.
    fn clip(
        &mut self,
        clip: &usvg::ClipPath,
        ts: usvg::Transform,
        layer: Px,
        layers: usize,
    ) -> f64 {
        let layers = self.deeper(layers);
        let px = layer.0 * layer.1;
        self.cost.work += 2. * px;
        let content =
            self.clip_children(clip.root(), ts.pre_concat(clip.transform()), layer, layers);
        let nested = clip
            .clip_path()
            .map_or(0., |c| self.clip(c, ts, layer, layers));
        1.25 * px + content.max(nested)
    }

    /// A clip's content: paths filled into its buffer; a group with a clip of
    /// its own on a buffer of its own.
    fn clip_children(
        &mut self,
        group: &usvg::Group,
        ts: usvg::Transform,
        layer: Px,
        layers: usize,
    ) -> f64 {
        let mut held = 0f64;
        for node in group.children() {
            self.cost.nodes += 1;
            if self.over() {
                break;
            }
            held = held.max(match node {
                usvg::Node::Path(p) => {
                    let painted = Self::cut(p.bounding_box().transform(ts), layer);
                    self.cost.work += painted.0 * painted.1;
                    0.
                }
                usvg::Node::Text(t) => self.clip_children(t.flattened(), ts, layer, layers),
                usvg::Node::Group(g) => {
                    let ts = ts.pre_concat(g.transform());
                    match g.clip_path() {
                        Some(clip) => {
                            let layers = self.deeper(layers);
                            let inside = self.clip_children(g, ts, layer, layers);
                            layer.0 * layer.1 + inside.max(self.clip(clip, ts, layer, layers))
                        }
                        None => self.clip_children(g, ts, layer, layers),
                    }
                }
                usvg::Node::Image(_) => 0.,
            });
        }
        held
    }

    /// A mask: a buffer of the layer, the region's mask and the mask made of
    /// it (a byte a pixel each), the mask of the mask applied while held.
    fn mask(&mut self, mask: &usvg::Mask, ts: usvg::Transform, layer: Px, layers: usize) -> f64 {
        if mask.root().children().is_empty() {
            return 0.;
        }
        let layers = self.deeper(layers);
        let px = layer.0 * layer.1;
        self.cost.work += 3. * px;
        let content = self.children(mask.root(), ts, layer, layers);
        let nested = mask.mask().map_or(0., |m| self.mask(m, ts, layer, layers));
        1.5 * px + content.max(nested)
    }

    /// A filter on a layer: its region copied, each primitive's result kept
    /// until the last, and a pass's inputs copied. resvg doesn't cut the
    /// region to the layer (a flood, merge or image is a buffer of it).
    fn filter(
        &mut self,
        filter: &usvg::filter::Filter,
        ts: usvg::Transform,
        layer: Px,
        layers: usize,
    ) -> f64 {
        use usvg::filter::Kind;
        let _ = layer;
        let region = Self::cut(
            filter.rect().transform(ts).map(|r| r.to_rect()),
            (f64::INFINITY, f64::INFINITY),
        );
        let px = region.0 * region.1;
        let (sx, sy) = ts.get_scale();
        let mut held = px;
        let mut peak = held;
        for primitive in filter.primitives() {
            let per_pixel = match primitive.kind() {
                Kind::GaussianBlur(_) | Kind::DropShadow(_) => 8.,
                Kind::Morphology(m) => {
                    let side = |r: f32, s: f32, max: f64| {
                        (2. * f64::from((r * s).ceil())).clamp(1., max.max(1.))
                    };
                    side(m.radius_x().get(), sx, region.0) * side(m.radius_y().get(), sy, region.1)
                }
                Kind::ConvolveMatrix(c) => {
                    f64::from(c.matrix().columns()) * f64::from(c.matrix().rows())
                }
                Kind::Turbulence(t) => 8. * f64::from(t.num_octaves().max(1)),
                Kind::DiffuseLighting(_) | Kind::SpecularLighting(_) => 16.,
                Kind::Image(image) => {
                    let layers = self.deeper(layers);
                    let inside = self.children(image.root(), ts, region, layers);
                    peak = peak.max(held + px + inside);
                    1.
                }
                _ => 2.,
            };
            self.cost.work += px * per_pixel;
            held += px;
            peak = peak.max(held + 2. * px);
        }
        peak
    }
}

/// What drawing `tree` takes with `f` pixels a unit: the bytes of pixels held
/// at once, the canvas's included, and the pixels painted.
fn cost_at(tree: &usvg::Tree, f: f64) -> Result<(f64, f64), String> {
    let size = tree.size();
    let canvas = (
        (f64::from(size.width()) * f).floor().max(1.),
        (f64::from(size.height()) * f).floor().max(1.),
    );
    let mut walk = Walk {
        limit: (5. * canvas.0, 5. * canvas.1),
        cost: Cost::default(),
    };
    let ts = usvg::Transform::from_scale(
        (canvas.0 / f64::from(size.width())) as f32,
        (canvas.1 / f64::from(size.height())) as f32,
    );
    let held = walk.children(tree.root(), ts, canvas, 0);
    if walk.cost.nodes > MAX_NODES {
        return Err("svg_too_complex".into());
    }
    if walk.cost.layers > MAX_LAYERS {
        return Err("svg_too_many_layers".into());
    }
    let px = canvas.0 * canvas.1;
    Ok((4. * (px + held), px + walk.cost.work))
}

/// The factor (of GPUI's twice-the-size drawing) that keeps the canvas within
/// [`MAX_EDGE`] and its drawing within [`BUDGET`] and [`WORK`].
fn scale_within_budget(tree: &usvg::Tree) -> Result<f32, String> {
    let size = tree.size();
    let longest = f64::from(size.width().max(size.height()));
    let smooth = f64::from(SMOOTH_SVG_SCALE_FACTOR);
    // Pixels a unit.
    let mut f = smooth * (f64::from(MAX_EDGE) / (smooth * longest)).min(1.);
    // Most of it grows with the square of the scale (a morphology's radius
    // faster): shrink until it fits.
    for _ in 0..8 {
        let (bytes, work) = cost_at(tree, f)?;
        let shrink = (BUDGET as f64 / bytes).min(WORK / work).sqrt();
        if shrink >= 1. {
            return Ok((f / smooth) as f32);
        }
        f *= shrink * 0.95;
        if longest * f < f64::from(MIN_EDGE) {
            break;
        }
    }
    Err("svg_too_complex".into())
}

/// Refuses markup nested deeper than [`MAX_DEPTH`], and entities whose text
/// holds markup or references (they'd expand into nesting or text the scan
/// can't see). Malformed markup is left to the parser, which refuses it.
fn check_markup(xml: &[u8]) -> Result<(), String> {
    let find = |from: usize, what: &[u8]| {
        xml[from..]
            .windows(what.len())
            .position(|w| w == what)
            .map(|at| from + at + what.len())
    };
    let mut depth = 0usize;
    let mut at = 0;
    while let Some(lt) = xml[at..].iter().position(|b| *b == b'<').map(|p| at + p) {
        let rest = &xml[lt..];
        let next = if rest.starts_with(b"<!--") {
            find(lt + 4, b"-->")
        } else if rest.starts_with(b"<![CDATA[") {
            find(lt + 9, b"]]>")
        } else if rest.starts_with(b"<?") {
            find(lt + 2, b"?>")
        } else if rest.starts_with(b"<!") {
            let end = declaration_end(xml, lt);
            if let Some(end) = end {
                check_entities(&xml[lt..end])?;
            }
            end
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            find(lt + 2, b">")
        } else {
            let end = tag_end(xml, lt);
            if end.is_some_and(|end| xml[end - 2] != b'/') {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err("svg_too_deep".into());
                }
            }
            end
        };
        let Some(next) = next else { return Ok(()) };
        at = next;
    }
    Ok(())
}

/// Just past the `>` that ends the tag starting at `lt`, quotes skipped.
fn tag_end(xml: &[u8], lt: usize) -> Option<usize> {
    let mut quote = None;
    for (i, b) in xml.iter().enumerate().skip(lt + 1) {
        match (quote, *b) {
            (None, b'"' | b'\'') => quote = Some(*b),
            (Some(q), b) if b == q => quote = None,
            (None, b'>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// Just past the `>` that ends the `<!…>` declaration at `lt` (a DOCTYPE's
/// internal subset, its quotes and comments included).
fn declaration_end(xml: &[u8], lt: usize) -> Option<usize> {
    let (mut quote, mut subset) = (None, false);
    let mut i = lt + 2;
    while i < xml.len() {
        let b = xml[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if xml[i..].starts_with(b"<!--") => {
                i = xml[i + 4..].windows(3).position(|w| w == b"-->")? + i + 4 + 3;
                continue;
            }
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'[' => subset = true,
                b']' => subset = false,
                b'>' if !subset => return Some(i + 1),
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Each `<!ENTITY name "text">` in a declaration: a parameter entity, or
/// text with `<`, `&` or `%`, is refused.
fn check_entities(declaration: &[u8]) -> Result<(), String> {
    let refuse = || Err("svg_entities".to_owned());
    let mut rest = declaration;
    while let Some(at) = rest.windows(8).position(|w| w == b"<!ENTITY") {
        rest = &rest[at + 8..];
        let after_space = rest
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(rest.len());
        if rest.get(after_space) == Some(&b'%') {
            return refuse();
        }
        // The name, then its text if quoted (external ones aren't loaded).
        let Some(open) = rest.iter().position(|b| matches!(b, b'"' | b'\'' | b'>')) else {
            return Ok(());
        };
        if rest[open] == b'>' {
            continue;
        }
        let quote = rest[open];
        let Some(len) = rest[open + 1..].iter().position(|b| *b == quote) else {
            return Ok(());
        };
        if rest[open + 1..open + 1 + len]
            .iter()
            .any(|b| matches!(b, b'<' | b'&' | b'%'))
        {
            return refuse();
        }
        rest = &rest[open + 1 + len..];
    }
    Ok(())
}

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
    use super::samples::{gzip, image_at, nested_in_images, svg_data, svg_with};
    use super::*;

    const RED_RECT: &str = r#"<rect width="4" height="4" fill="red"/>"#;

    fn renderer() -> SvgRenderer {
        SvgRenderer::new(Arc::new(()))
    }

    fn svg(inner: &str, side: u32) -> Vec<u8> {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}">{inner}</svg>"#)
            .into_bytes()
    }

    fn nested(open: &str, close: &str, n: usize, side: u32) -> Vec<u8> {
        let inner = format!(
            r#"{}<rect width="{side}" height="{side}" fill="red"/>{}"#,
            open.repeat(n),
            close.repeat(n)
        );
        svg(&inner, side)
    }

    #[test]
    fn a_plain_svg_draws_at_twice_its_size() {
        let image = rasterize(&renderer(), svg(RED_RECT, 4)).unwrap();
        assert_eq!(
            image.size(0),
            gpui::size(gpui::DevicePixels(8), gpui::DevicePixels(8))
        );
        assert_eq!(&image.as_bytes(0).unwrap()[..4], [0, 0, 255, 255]);
    }

    #[test]
    fn deep_nesting_is_refused_before_parsing() {
        // `<g>` a thousand deep: GPUI's decoder overflows its stack on it.
        let deep = nested("<g>", "</g>", 1000, 4);
        assert_eq!(rasterize(&renderer(), deep).unwrap_err(), "svg_too_deep");
        // Past the markup's limit, within the reach's.
        let past = nested("<g>", "</g>", MAX_DEPTH, 4);
        assert_eq!(rasterize(&renderer(), past).unwrap_err(), "svg_too_deep");
        // As deep as allowed: drawn on our stack.
        let ok = nested("<g>", "</g>", MAX_DEPTH - 2, 4);
        assert!(rasterize(&renderer(), ok).is_ok());
        // Comments, CDATA, quotes and self-closed tags don't count.
        let mixed = svg(
            r#"<!-- <g><g> --><![CDATA[<g>]]><g title="<g>"><rect width="4" height="4"/></g>"#,
            4,
        );
        assert!(check_markup(&mixed).is_ok());
    }

    #[test]
    fn use_chains_deeper_than_markup_stay_on_our_stack() {
        // Each `<use>` names the next: nested ~1000 deep once expanded, while
        // the markup is three deep.
        let chain = |n: usize| {
            let defs: String = (0..n)
                .map(|i| format!(r##"<g id="u{i}"><use href="#u{}"/></g>"##, i + 1))
                .collect();
            let end = format!(r#"<rect id="u{n}" width="4" height="4"/>"#);
            svg(
                &format!(r##"<defs>{defs}{end}</defs><use href="#u0"/>"##),
                4,
            )
        };
        assert_eq!(
            rasterize(&renderer(), chain(1000)).unwrap_err(),
            "svg_too_deep"
        );
        assert!(rasterize(&renderer(), chain(100)).is_ok());
    }

    #[test]
    fn gzip_is_refused_uninflated() {
        // usvg inflates an SVGZ whole, however far it expands.
        let gz = gzip(svg_with(RED_RECT).as_bytes());
        assert_eq!(rasterize(&renderer(), gz).unwrap_err(), "svgz");
    }

    #[test]
    fn entities_that_expand_into_markup_or_more_entities_are_refused() {
        let doc = |dtd: &str| {
            format!(
                r#"<?xml version="1.0"?><!DOCTYPE svg [{dtd}]><svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#
            )
        };
        let check_markup = |xml: &[u8]| rasterize(&renderer(), xml.to_vec()).map(|_| ());
        let laughs = doc(r#"<!ENTITY a "lol"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">"#);
        assert_eq!(check_markup(laughs.as_bytes()).unwrap_err(), "svg_entities");
        let markup = doc(r#"<!ENTITY g "<g><g><g></g></g></g>">"#);
        assert_eq!(check_markup(markup.as_bytes()).unwrap_err(), "svg_entities");
        assert_eq!(
            check_markup(doc(r#"<!ENTITY % p "x">"#).as_bytes()).unwrap_err(),
            "svg_entities"
        );
        // Illustrator's: names for its namespaces.
        let named = doc(
            r#"<!-- it's --><!ENTITY ns_svg "http://www.w3.org/2000/svg"><!ENTITY ns_x SYSTEM "x.dtd">"#,
        );
        assert!(check_markup(named.as_bytes()).is_ok());
    }

    #[test]
    fn layers_inside_one_another_shrink_the_canvas_to_the_budget() {
        // Eight groups with opacity on a 4096² canvas: 1.9 GB drawn as is,
        // each a layer of the canvas (and a pixel more around).
        let groups = nested(r#"<g opacity=".9">"#, "</g>", 8, 4096);
        let image = rasterize(&renderer(), groups).unwrap();
        let px = image.size(0);
        let (w, h) = (i32::from(px.width) as u64, i32::from(px.height) as u64);
        assert!(4 * (w * h + 8 * (w + 4) * (h + 4)) <= BUDGET, "{w}×{h}");
        assert!(w > 2048, "{w}×{h}");
        // Without layers, only the longest side is capped.
        let plain = rasterize(&renderer(), nested("<g>", "</g>", 8, 4096)).unwrap();
        assert_eq!(i32::from(plain.size(0).width), MAX_EDGE as i32);
        // Too many to draw at any useful size.
        let many = nested(r#"<g opacity=".9">"#, "</g>", MAX_LAYERS + 1, 4);
        assert_eq!(
            rasterize(&renderer(), many).unwrap_err(),
            "svg_too_many_layers"
        );
    }

    #[test]
    fn expanded_uses_count_towards_the_nodes() {
        // 400 uses of a group of 400 rects: 160,000 nodes from 400 + 400 tags.
        let rects = r#"<rect width="1" height="1"/>"#.repeat(400);
        let uses = r##"<use href="#a"/>"##.repeat(400);
        let fan = svg(&format!(r#"<defs><g id="a">{rects}</g></defs>{uses}"#), 4);
        assert_eq!(rasterize(&renderer(), fan).unwrap_err(), "svg_too_complex");
    }

    #[test]
    fn an_image_in_it_shows_nothing() {
        use base64::Engine as _;
        let first_pixel = |svg: String| {
            let image = rasterize(&renderer(), svg.into_bytes()).unwrap();
            <[u8; 4]>::try_from(&image.as_bytes(0).unwrap()[..4]).unwrap()
        };
        // SVGs in data URIs 75 deep: each parsed inside the last (GPUI's
        // decoder ran out of stack on them).
        assert_eq!(
            first_pixel(nested_in_images(svg_with(RED_RECT), 75)),
            [0; 4]
        );
        // One gzipped: inflated whole.
        let gz =
            base64::engine::general_purpose::STANDARD.encode(gzip(svg_with(RED_RECT).as_bytes()));
        assert_eq!(
            first_pixel(image_at(&format!("data:image/svg+xml;base64,{gz}"))),
            [0; 4]
        );
        // Fifty uses of one: each drawn on a layer of the whole canvas.
        let fan = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><defs><image id="a" width="4" height="4" href="{}"/></defs>{}</svg>"##,
            svg_data(&svg_with(RED_RECT)),
            r##"<use href="#a"/>"##.repeat(50)
        );
        assert_eq!(first_pixel(fan), [0; 4]);
    }
    /// The first pixel of `svg` drawn, or why it isn't.
    fn drawn(svg: String) -> Result<(i32, [u8; 4]), String> {
        let image = rasterize(&renderer(), svg.into_bytes())?;
        let px = <[u8; 4]>::try_from(&image.as_bytes(0).unwrap()[..4]).unwrap();
        Ok((i32::from(image.size(0).width), px))
    }

    fn defs(defs: &str, body: &str, side: u32) -> String {
        String::from_utf8(svg(&format!("<defs>{defs}</defs>{body}"), side)).unwrap()
    }

    #[test]
    fn references_that_loop_are_refused() {
        // usvg breaks loops of one or two; longer ones it follows until its
        // stack runs out.
        let clips: String = ["a", "b", "c"]
            .iter()
            .zip(["b", "c", "a"])
            .map(|(id, next)| {
                format!(r#"<clipPath id="{id}" clip-path="url(#{next})"><rect width="4" height="4"/></clipPath>"#)
            })
            .collect();
        let looped = [
            defs(&clips, r#"<rect width="4" height="4" clip-path="url(#a)"/>"#, 4),
            // Inherited: the pattern's content fills with the pattern.
            String::from_utf8(svg(
                r#"<g fill="url(#p)"><pattern id="p" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4"/></pattern><rect width="4" height="4"/></g>"#,
                4,
            ))
            .unwrap(),
            // And a marker's content marked with the marker.
            String::from_utf8(svg(
                r#"<g marker-start="url(#m)"><marker id="m"><path d="M0 0L4 4" stroke="red"/></marker><path d="M0 0L4 4" stroke="red"/></g>"#,
                4,
            ))
            .unwrap(),
            // From a style sheet.
            defs(
                r#"<style>.x{mask:url(#n)}</style><mask id="n"><rect class="x" width="4" height="4" fill="white"/></mask>"#,
                r#"<rect class="x" width="4" height="4"/>"#,
                4,
            ),
        ];
        // Three patterns, each filling with the next only by an `!important`
        // rule a later `style` doesn't override, and the pattern's content
        // inheriting it past an attribute in another namespace (not a
        // presentation attribute to usvg).
        let rules: String = ["a", "b", "c"]
            .iter()
            .zip(["b", "c", "a"])
            .map(|(id, next)| format!(".t{id}{{fill:url(#p{next}) !important}}"))
            .collect();
        let patterns: String = ["a", "b", "c"]
            .iter()
            .map(|id| {
                format!(r#"<pattern id="p{id}" width="4" height="4" patternUnits="userSpaceOnUse"><g class="t{id}" style="fill:red"><rect xmlns:x="urn:x" x:fill="red" width="4" height="4"/></g></pattern>"#)
            })
            .collect();
        let precedence = defs(
            &format!("<style>{rules}</style>{patterns}"),
            r#"<rect width="4" height="4" fill="url(#pa)"/>"#,
            4,
        );
        for svg in looped.into_iter().chain([precedence]) {
            assert_eq!(drawn(svg.clone()).unwrap_err(), "svg_too_deep", "{svg}");
        }
    }

    #[test]
    fn reference_chains_deeper_than_the_reach_are_refused() {
        // Each step a level at least.
        let n = MAX_REACH;
        let chain = |make: &dyn Fn(usize) -> String, last: &str, body: &str| {
            defs(&((0..n).map(make).collect::<String>() + last), body, 4)
        };
        let chains = [
            chain(
                &|i| {
                    format!(
                        r#"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="4" height="4"/></clipPath>"#,
                        i + 1
                    )
                },
                &format!(r#"<clipPath id="c{n}"><rect width="4" height="4"/></clipPath>"#),
                r#"<rect width="4" height="4" clip-path="url(#c0)"/>"#,
            ),
            chain(
                &|i| {
                    format!(
                        r#"<mask id="m{i}"><rect width="4" height="4" fill="white" mask="url(#m{})"/></mask>"#,
                        i + 1
                    )
                },
                &format!(r#"<mask id="m{n}"><rect width="4" height="4" fill="white"/></mask>"#),
                r#"<rect width="4" height="4" mask="url(#m0)"/>"#,
            ),
            chain(
                &|i| {
                    format!(
                        r#"<pattern id="p{i}" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="url(#p{})"/></pattern>"#,
                        i + 1
                    )
                },
                &format!(
                    r#"<pattern id="p{n}" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4"/></pattern>"#
                ),
                r#"<rect width="4" height="4" fill="url(#p0)"/>"#,
            ),
            chain(
                &|i| {
                    format!(
                        r#"<marker id="k{i}"><path d="M0 0L4 4" stroke="red" marker-end="url(#k{})"/></marker>"#,
                        i + 1
                    )
                },
                &format!(r#"<marker id="k{n}"><path d="M0 0L4 4" stroke="red"/></marker>"#),
                r#"<path d="M0 0L4 4" stroke="red" marker-end="url(#k0)"/>"#,
            ),
        ];
        for svg in chains {
            assert_eq!(drawn(svg).unwrap_err(), "svg_too_deep");
        }
    }

    #[test]
    fn copies_usvg_would_make_count_towards_the_nodes() {
        // A marker at each of 20,000 vertices.
        let marker = format!(
            r#"<marker id="m">{}</marker>"#,
            r#"<rect width="1" height="1"/>"#.repeat(10)
        );
        let path = format!(
            r#"<path marker-mid="url(#m)" d="M0 0{}"/>"#,
            " L1 1".repeat(20_000)
        );
        assert_eq!(
            drawn(defs(&marker, &path, 4)).unwrap_err(),
            "svg_too_complex"
        );
        // The same through a `<use>`, which passes its marker on.
        let shape = format!(r#"<path id="s" d="M0 0{}"/>"#, " L1 1".repeat(20_000));
        let used = defs(
            &(marker + &shape),
            r##"<use href="#s" marker-mid="url(#m)"/>"##,
            4,
        );
        assert_eq!(drawn(used).unwrap_err(), "svg_too_complex");
        // Text: a node a character.
        let text = format!(r#"<text y="4">{}</text>"#, "x".repeat(MAX_NODES + 1));
        assert_eq!(drawn(defs("", &text, 4)).unwrap_err(), "svg_too_complex");
    }

    #[test]
    fn style_sheets_that_take_too_long_to_match_are_refused() {
        // Each `g` tried at each ancestor: some 10^21 ways for usvg to try.
        let sheet = format!("<style>a {}{{fill:red}}</style>", "g ".repeat(20));
        let deep = nested("<g>", "</g>", 120, 4);
        let deep = String::from_utf8(deep).unwrap().replacen("<g>", &sheet, 0);
        let deep = deep.replacen(">", &format!(">{sheet}"), 1);
        assert_eq!(drawn(deep).unwrap_err(), "svg_too_complex");
    }

    #[test]
    fn an_illustrator_like_sheet_still_draws() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 4 4" width="4" height="4"><defs><style>.c1{fill:none}.c2{clip-path:url(#clip)}.c3{fill:url(#red)}</style><clipPath id="clip"><rect class="c1" width="4" height="4"/></clipPath><linearGradient id="red"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="red"/></linearGradient><symbol id="s"><rect class="c3" width="4" height="4"/></symbol></defs><g class="c2"><use xlink:href="#s"/></g></svg>"##;
        assert_eq!(drawn(svg.into()).unwrap(), (8, [0, 0, 255, 255]));
    }

    #[test]
    fn what_resvg_holds_beyond_the_canvas_shrinks_it() {
        let red = |side: u32| format!(r#"<rect width="{side}" height="{side}" fill="red"/>"#);
        // A pattern's tile: its own size, here 160 GB.
        let tile = defs(
            &format!(
                r#"<pattern id="p" width="100000" height="100000" patternUnits="userSpaceOnUse">{}</pattern>"#,
                red(100_000)
            ),
            r#"<rect width="64" height="64" fill="url(#p)"/>"#,
            64,
        );
        assert_eq!(drawn(tile).unwrap_err(), "svg_too_complex");
        // A filter keeps each primitive's result: 1000 canvases.
        let offsets: String = (0..1000)
            .map(|i| format!(r#"<feOffset dx="{}"/>"#, i % 2))
            .collect();
        let filtered = defs(
            &format!(r#"<filter id="f" x="0" y="0" width="1" height="1">{offsets}</filter>"#),
            &red(2048).replace("/>", r#" filter="url(#f)"/>"#),
            2048,
        );
        let (side, _) = drawn(filtered).unwrap();
        assert!((side as u64).pow(2) * 4 * 1000 <= BUDGET, "{side}");
        // A filter's region, larger than its layer: a buffer of it.
        let flood = defs(
            r#"<filter id="f" filterUnits="userSpaceOnUse" x="-20000" y="-20000" width="40000" height="40000"><feFlood flood-color="red"/></filter>"#,
            r#"<rect width="64" height="64" filter="url(#f)"/>"#,
            64,
        );
        assert_eq!(drawn(flood).unwrap_err(), "svg_too_complex");
        // Masks on masks: each held while the next is drawn.
        let masks: String = (0..12)
            .map(|i| {
                format!(
                    r#"<mask id="m{i}" mask="url(#m{})">{}</mask>"#,
                    i + 1,
                    red(1024).replace("red", "white")
                )
            })
            .collect();
        let masked = defs(
            &format!(
                r#"{masks}<mask id="m12">{}</mask>"#,
                red(1024).replace("red", "white")
            ),
            &red(1024).replace("/>", r#" mask="url(#m0)"/>"#),
            1024,
        );
        let (side, _) = drawn(masked).unwrap();
        let layer = ((side + 4) as f64).powi(2);
        assert!(
            side < 2048 && 4. * layer * (2. + 13. * 1.5) <= BUDGET as f64,
            "{side}"
        );
    }

    #[test]
    fn what_resvg_paints_beyond_the_budget_shrinks_or_refuses_it() {
        let filter = |primitive: &str| {
            defs(
                &format!(r#"<filter id="f">{primitive}</filter>"#),
                r#"<rect width="2048" height="2048" fill="red" filter="url(#f)"/>"#,
                2048,
            )
        };
        // Each pixel's neighbors 4000 across and down; noise a million times over.
        for primitive in [
            r#"<feMorphology radius="1000"/>"#,
            r#"<feTurbulence baseFrequency=".05" numOctaves="1000000"/>"#,
        ] {
            assert_eq!(
                drawn(filter(primitive)).unwrap_err(),
                "svg_too_complex",
                "{primitive}"
            );
        }
        // Dashes a thousandth long: a million pieces a path, at any scale.
        let dashed =
            r#"<path d="M0 0H1000" stroke="red" stroke-dasharray=".0005 .0005"/>"#.repeat(200);
        assert_eq!(
            drawn(String::from_utf8(svg(&dashed, 64)).unwrap()).unwrap_err(),
            "svg_too_complex"
        );
        // 20,000 rects filling a 4096² canvas each: drawn small.
        let fan = svg(
            &r#"<rect width="2048" height="2048" fill="red"/>"#.repeat(20_000),
            2048,
        );
        let (side, px) = drawn(String::from_utf8(fan).unwrap()).unwrap();
        assert!((side as f64).powi(2) * 20_000. <= WORK, "{side}");
        assert_eq!(px, [0, 0, 255, 255]);
    }
}
