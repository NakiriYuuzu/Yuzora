//! The graph column as a native element: `GraphCell.new(id, {log, row})`
//! paints one row of a log session's graph (its node, the edges through it
//! and arrows where long edges hide) with GPUI paths, from the row's cached
//! print elements. A script's virtual list places one per visible row, so
//! scrolling draws only what is on screen and never crosses into script.

use gpui::{BorderStyle, Bounds, Hsla, IntoElement, PathBuilder, Pixels, Styled, Window, canvas, point, px, quad, size};
use yuzora_core::HostValue;

use crate::graph::{PALETTE_SIZE, RowPrint};

/// The design's branch colors (blue first: HEAD's branch).
pub const PALETTE: [u32; PALETTE_SIZE] = [0x4f6bed, 0xe8a27f, 0x86d17a, 0xc084fc, 0x93c5fd, 0xf472b6, 0xfacc15, 0x2dd4bf];

#[derive(Clone, Debug, PartialEq)]
pub struct CellStyle {
    pub lane: f32,
    pub height: f32,
    pub dot: f32,
    pub stroke: f32,
    pub background: Hsla,
    pub head: Hsla,
    pub colors: Vec<Hsla>,
}

pub fn hex_color(s: &str) -> Option<Hsla> {
    let h = s.trim().trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some(gpui::rgb(v).into()),
        8 => Some(gpui::rgba(v).into()),
        _ => None,
    }
}

impl CellStyle {
    pub fn from_props(p: &HostValue) -> Self {
        let num = |k: &str, d: f32| p.get(k).and_then(HostValue::as_number).map_or(d, |n| n as f32);
        let color = |k: &str, d: u32| p.get(k).and_then(HostValue::as_str).and_then(hex_color).unwrap_or_else(|| gpui::rgb(d).into());
        let colors = p
            .get("colors")
            .and_then(HostValue::as_array)
            .map(|a| a.iter().filter_map(|c| c.as_str().and_then(hex_color)).collect::<Vec<_>>())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| PALETTE.iter().map(|c| gpui::rgb(*c).into()).collect());
        CellStyle { lane: num("lane", 16.0), height: num("height", 26.0), dot: num("dot", 9.0), stroke: num("stroke", 1.6), background: color("background", 0x17181b), head: color("headColor", 0x93c5fd), colors }
    }

    fn color(&self, i: u8) -> Hsla {
        self.colors[i as usize % self.colors.len()]
    }
}

/// The element for one row (empty when the session or row is gone).
pub fn cell(props: &HostValue) -> gpui::AnyElement {
    let style = CellStyle::from_props(props);
    let log = props.get("log").and_then(HostValue::as_number).unwrap_or(0.0) as u64;
    let row = props.get("row").and_then(HostValue::as_number).unwrap_or(0.0) as u32;
    let head = props.get("head").and_then(HostValue::as_bool).unwrap_or(false);
    let print = crate::log::try_get(log).and_then(|s| s.row_print(row));
    let width = props.get("width").and_then(HostValue::as_number).map_or_else(|| print.as_ref().map_or(1.0, |p| p.width as f32) * style.lane, |w| w as f32);
    let height = style.height;
    canvas(|_, _, _| (), move |bounds, (), window, _cx| {
        if let Some(p) = &print {
            paint_row(p, &style, head, bounds, window);
        }
    })
    .w(px(width))
    .h(px(height))
    .flex_none()
    .into_any_element()
}

fn x(style: &CellStyle, bounds: &Bounds<Pixels>, column: f32) -> Pixels {
    bounds.origin.x + px(style.lane * (column + 0.5))
}

fn line(window: &mut Window, from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, width: f32, color: Hsla) {
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(from);
    b.line_to(to);
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

pub fn paint_row(p: &RowPrint, style: &CellStyle, head: bool, bounds: Bounds<Pixels>, window: &mut Window) {
    let top = bounds.origin.y;
    let mid = top + px(style.height / 2.0);
    let bottom = top + px(style.height);
    for s in &p.segments {
        let color = style.color(s.color);
        let at = point(x(style, &bounds, s.at), mid);
        if s.arrow {
            // A short stub toward the hidden part, ending in a chevron.
            let reach = px(style.height * 0.32);
            let (end, dir) = if s.up { (mid - reach, -1.0) } else { (mid + reach, 1.0) };
            let tip = point(at.x, end);
            line(window, at, tip, style.stroke, color);
            let wing = px(3.0);
            let back = px(3.0 * dir);
            line(window, tip, point(tip.x - wing, tip.y - back), style.stroke, color);
            line(window, tip, point(tip.x + wing, tip.y - back), style.stroke, color);
            continue;
        }
        let end = point(x(style, &bounds, s.to), if s.up { top } else { bottom });
        line(window, at, end, style.stroke, color);
    }
    let r = style.dot / 2.0;
    let center = point(x(style, &bounds, p.node as f32), mid);
    let dot = Bounds::new(point(center.x - px(r), center.y - px(r)), size(px(style.dot), px(style.dot)));
    let color = style.color(p.color);
    let (fill, border) = if head { (style.head, style.head) } else if p.merge { (color, color) } else { (style.background, color) };
    window.paint_quad(quad(dot, px(r), fill, px(if head || p.merge { 0.0 } else { 2.0 }), border, BorderStyle::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_choose_the_look() {
        let p = HostValue::Object(vec![("lane".into(), HostValue::Number(12.0)), ("colors".into(), HostValue::Array(vec![HostValue::Str("#ff0000".into())])), ("background".into(), HostValue::Str("#000000".into()))]);
        let s = CellStyle::from_props(&p);
        assert_eq!(s.lane, 12.0);
        assert_eq!(s.colors.len(), 1);
        assert_eq!(s.color(5), s.colors[0]);
        assert_eq!(CellStyle::from_props(&HostValue::Null).colors.len(), PALETTE_SIZE);
        assert!(hex_color("#4f6bed").is_some() && hex_color("zz").is_none());
    }
}
