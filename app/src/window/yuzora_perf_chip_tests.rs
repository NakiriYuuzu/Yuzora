// yuzora(L8): included into `window::tests`, in a file of its own so upstream's new tests in window.rs never meet
// these in a merge. The resource chip at the right end of a layout's status bar: drawn there, and no tree, file or
// click ever holds it.

/// A status bar mod: its own background, a 200 px item that does not shrink at its left, and one at its right end.
const CHIP_BAR_JS: &str = r##"import { View, div } from "gpui-kit";
export default class Bar extends View {
  render() {
    return div().size_full().flex().bg("#123456").child(div().flex_none().w(200).h_full().bg("#345678")).child(div().flex_1())
      .child(div().flex_none().w(40).h_full().bg("#654321"));
  }
}
"##;

/// `main`'s shape: the tab, and under it a 28 px status bar showing the mod `bar`.
fn chip_layout() -> serde_json::Value {
    serde_json::json!({ "id": "root", "type": "column", "theme": "yuzora-dark", "children": [
        { "id": "main", "type": "block", "content": { "tab": "active" } },
        { "id": "status", "type": "block", "size": { "px": 28 }, "content": { "mod": "bar" } } ] })
}

/// The window's copies of `yuzora-perf`, by slot.
fn chip_copies(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext) -> Vec<(String, serde_json::Value)> {
    let mut out: Vec<_> = view.update(cx, |this, _| {
        this.copies.iter().filter(|(_, c)| c.inst.name == "yuzora-perf").map(|(k, c)| (k.clone(), c.inst.props.clone())).collect()
    });
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A painted quad: what of it shows (its bounds within its content mask) and its background.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChipQuad {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    bg: gpui::Background,
}

/// The window's size, and the part that shows of every quad the last frame painted that is not fully transparent.
fn chip_frame(cx: &mut gpui::VisualTestContext) -> ((f32, f32), Vec<ChipQuad>) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let v = window.viewport_size();
        let quads = window
            .painted_quads()
            .into_iter()
            .filter(|q| !q.background.is_transparent())
            .filter_map(|q| {
                let shown = q.bounds.intersect(&q.content_mask.bounds);
                (shown.size.width.0 > 0.0 && shown.size.height.0 > 0.0).then(|| ChipQuad {
                    x: shown.origin.x.0 / scale,
                    y: shown.origin.y.0 / scale,
                    w: shown.size.width.0 / scale,
                    h: shown.size.height.0 / scale,
                    bg: q.background,
                })
            })
            .collect();
        ((v.width.as_f32(), v.height.as_f32()), quads)
    })
}

/// Where the block `id` is on screen: x, y, width, height.
fn chip_bounds(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, id: &str) -> Option<(f32, f32, f32, f32)> {
    chip_frame(cx);
    view.update(cx, |this, _| this.bounds.borrow().get(id).map(|b| (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))))
}

fn chip_rgb(hex: u32) -> gpui::Background {
    gpui::rgb(hex).into()
}

/// The shown theme's color `name`, as a mod paints it.
fn chip_theme(name: &str) -> gpui::Background {
    let hex = super::settings::theme()[name].clone();
    chip_rgb(u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap())
}

fn chip_near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

/// The chip's own quad in the block at `bar`: its face, at the block's right end, as tall as the block.
fn chip_drawn(quads: &[ChipQuad], bar: (f32, f32, f32, f32), face: gpui::Background) -> Option<ChipQuad> {
    quads.iter().copied().find(|q| q.bg == face && chip_near(q.x + q.w, bar.0 + bar.2) && chip_near(q.y, bar.1) && chip_near(q.h, bar.3) && q.w < bar.2)
}

fn chip_run(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, name: &str, args: serde_json::Value) {
    cx.update(|window, cx| view.update(cx, |this, cx| this.run(name, args, window, cx)));
}

fn chip_file(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, tree: &serde_json::Value) {
    std::fs::write(super::layout_path("test"), tree.to_string()).unwrap();
    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
}

#[test]
fn the_chip_is_drawn_at_the_right_end_of_the_status_bar() {
    in_own_process("perf_chip_in_the_bar");
}

#[gpui::test]
fn perf_chip_in_the_bar(cx: &mut gpui::TestAppContext) {
    if !running_alone("perf_chip_in_the_bar") {
        return;
    }
    let (view, cx) = open_window(cx, "bar", CHIP_BAR_JS, chip_layout());
    assert_eq!(chip_copies(&view, cx), [("status#yuzora-perf".to_owned(), serde_json::json!({ "bar": "bar" }))]);
    for size in [None, Some((700.0, 500.0)), Some((260.0, 500.0))] {
        if let Some((w, h)) = size {
            cx.simulate_resize(gpui::size(gpui::px(w), gpui::px(h)));
        }
        let ((w, h), quads) = chip_frame(cx);
        let bar = chip_bounds(&view, cx, "status").expect("the status bar is drawn");
        assert_eq!(bar, (0.0, h - 28.0, w, 28.0), "the status bar keeps its place and size");
        let chip = chip_drawn(&quads, bar, chip_theme("chrome")).unwrap_or_else(|| panic!("the chip at the bar's right end: {quads:?}"));
        assert!(chip_near(chip.w, 150.0), "150 px wide, at any window width: {chip:?}");
        let line = quads.iter().find(|q| q.bg == chip_theme("border") && chip_near(q.x, chip.x) && chip_near(q.y, bar.1) && chip_near(q.w, 150.0));
        assert_eq!(line.map(|q| q.h), Some(1.0), "the bar's top line, in the bar's color");
        let own = quads.iter().find(|q| q.bg == chip_rgb(0x123456)).expect("the bar's own mod is drawn");
        assert_eq!((own.x, own.y, own.h), (0.0, bar.1, 28.0));
        assert!(chip_near(own.w, w - 150.0), "the bar's mod gets the width left: {own:?} in {w}");
        if w - 150.0 >= 240.0 {
            let item = quads.iter().find(|q| q.bg == chip_rgb(0x654321)).expect("the bar's item at its right end");
            assert!(chip_near(item.x + item.w, w - 150.0), "the bar's own items end where the chip starts: {item:?}");
        }
        let bars = [0x123456, 0x345678, 0x654321].map(chip_rgb);
        assert!(quads.iter().filter(|q| bars.contains(&q.bg)).all(|q| q.x + q.w <= chip.x + 0.5), "nothing of the bar under the chip at {w}: {quads:?}");
    }
    view.update(cx, |this, _| {
        let shown = serde_json::to_string(&this.chrome).unwrap();
        assert!(!shown.contains("yuzora-perf"), "layout() never has it: {shown}");
    });
    // Zoomed in, the bar and the chip grow together.
    chip_run(&view, cx, "app.zoom_in", serde_json::json!({}));
    let zoom = super::settings::zoom();
    assert!(zoom > 1.0, "zoomed in: {zoom}");
    let (_, quads) = chip_frame(cx);
    let bar = chip_bounds(&view, cx, "status").unwrap();
    assert!(chip_near(bar.3, 28.0 * zoom), "{bar:?} at zoom {zoom}");
    let chip = chip_drawn(&quads, bar, chip_theme("chrome")).expect("still at the bar's right end");
    assert!(chip_near(chip.w, 150.0 * zoom), "{chip:?} at zoom {zoom}");
}

#[test]
fn a_layout_without_a_status_bar_or_with_its_own_chip_gets_none() {
    in_own_process("perf_chip_without_a_bar");
}

#[gpui::test]
fn perf_chip_without_a_bar(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("perf_chip_without_a_bar") {
        return;
    }
    // `terminals`' shape: a bar above the tab, none below.
    let terminals = json!({ "id": "root", "type": "column", "children": [
        { "id": "tabs", "type": "block", "size": { "px": 38 }, "content": { "mod": "bar" } },
        { "id": "main", "type": "block", "content": { "tab": "active" } }] });
    let (view, cx) = open_window(cx, "bar", CHIP_BAR_JS, terminals);
    let ((w, _), quads) = chip_frame(cx);
    assert_eq!(chip_copies(&view, cx), [], "no copy");
    let own = quads.iter().find(|q| q.bg == chip_rgb(0x123456)).expect("the top bar is drawn");
    assert_eq!(own.w, w, "drawn whole: no chip in it");
    // `yuzora`'s shape: its status bar places the chip itself, and nothing is added.
    let yuzora = json!({ "id": "root", "type": "column", "children": [
        { "id": "main", "type": "block", "content": { "tab": "active" } },
        { "id": "status", "type": "row", "size": { "px": 26 }, "children": [
            { "id": "bar", "type": "block", "content": { "mod": "bar" } },
            { "id": "perf", "type": "block", "size": { "px": 150 }, "content": { "mod": "yuzora-perf" } }] }] });
    chip_file(&view, cx, &yuzora);
    assert_eq!(chip_copies(&view, cx), [("perf".to_owned(), serde_json::Value::Null)], "only the layout's own");
    // A node named like the chip's slot: no chip, and that node's copy is drawn only where the node is.
    let named = json!({ "id": "root", "type": "column", "children": [
        { "id": "status#yuzora-perf", "type": "block", "size": { "px": 20 }, "content": { "mod": "bar" } },
        { "id": "main", "type": "block", "content": { "tab": "active" } },
        { "id": "status", "type": "block", "size": { "px": 28 }, "content": { "mod": "bar" } }] });
    chip_file(&view, cx, &named);
    let ((w, h), quads) = chip_frame(cx);
    assert_eq!(chip_copies(&view, cx), [], "no copy");
    let status = quads.iter().find(|q| q.bg == chip_rgb(0x123456) && chip_near(q.y, h - 28.0)).expect("the status bar is drawn");
    assert_eq!(status.w, w, "drawn whole, nothing beside it: {quads:?}");
}

#[test]
fn the_chip_follows_its_status_bar_through_edits_hiding_and_resizing() {
    in_own_process("perf_chip_follows_the_bar");
}

#[gpui::test]
fn perf_chip_follows_the_bar(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("perf_chip_follows_the_bar") {
        return;
    }
    let (view, cx) = open_window(cx, "bar", CHIP_BAR_JS, chip_layout());
    let face = chip_theme("chrome");
    let drawn = |view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext| {
        let (_, quads) = chip_frame(cx);
        let bar = chip_bounds(view, cx, "status")?;
        chip_drawn(&quads, bar, face)
    };
    assert!(drawn(&view, cx).is_some());
    let chip = vec![("status#yuzora-perf".to_owned(), json!({ "bar": "bar" }))];

    // A mod's layout.set without the status bar, then a file edit that brings it back.
    let mut without = chip_layout();
    without["children"].as_array_mut().unwrap().pop();
    chip_run(&view, cx, "layout.set", json!({ "layout": without }));
    assert_eq!((chip_copies(&view, cx), drawn(&view, cx)), (vec![], None), "no status bar, no chip");
    chip_file(&view, cx, &chip_layout());
    assert_eq!(chip_copies(&view, cx), chip, "back with its status bar");
    assert!(drawn(&view, cx).is_some());

    // A set that places a chip of its own: the added one goes.
    let mut placed = chip_layout();
    placed["children"].as_array_mut().unwrap().insert(0, json!({ "id": "mine", "type": "block", "size": { "px": 30 }, "content": { "mod": "yuzora-perf" } }));
    chip_run(&view, cx, "layout.set", json!({ "layout": placed }));
    assert_eq!(chip_copies(&view, cx), [("mine".to_owned(), serde_json::Value::Null)]);
    assert_eq!(drawn(&view, cx), None);
    chip_run(&view, cx, "layout.set", json!({ "layout": chip_layout() }));
    assert_eq!(chip_copies(&view, cx), chip);

    // Hidden with its status bar, shown again with it.
    chip_run(&view, cx, "layout.hide", json!({ "id": "status" }));
    assert_eq!(chip_copies(&view, cx), []);
    let (_, quads) = chip_frame(cx);
    assert!(!quads.iter().any(|q| q.bg == chip_rgb(0x123456)), "the status bar is hidden");
    assert!(!quads.iter().any(|q| q.bg == face && chip_near(q.w, 150.0)), "and nothing of the chip is drawn");
    chip_run(&view, cx, "layout.show", json!({ "id": "status" }));
    assert_eq!(chip_copies(&view, cx), chip);
    assert!(drawn(&view, cx).is_some(), "drawn again");

    // A click makes the bar taller than any status bar: it still is the layout's, and the chip stays in it.
    chip_run(&view, cx, "layout.resize", json!({ "id": "status", "px": 120 }));
    let bar = chip_bounds(&view, cx, "status").unwrap();
    assert_eq!(bar.3, 120.0);
    assert_eq!(chip_copies(&view, cx), chip);
    assert_eq!(drawn(&view, cx).map(|q| q.h), Some(120.0), "drawn the bar's height");
}

#[test]
fn a_click_on_the_chip_opens_its_details() {
    in_own_process("perf_chip_click");
}

#[gpui::test]
fn perf_chip_click(cx: &mut gpui::TestAppContext) {
    if !running_alone("perf_chip_click") {
        return;
    }
    cx.update(gpui_base::init);
    let (view, cx) = open_window(cx, "bar", CHIP_BAR_JS, chip_layout());
    let ((w, h), quads) = chip_frame(cx);
    // The details: a 420 px panel, wider than anything else the chip draws.
    let panel = |quads: &[ChipQuad]| quads.iter().find(|q| chip_near(q.w, 420.0)).copied();
    assert_eq!(panel(&quads), None, "closed at first");
    // The chip's caret, at the status bar's right end.
    cx.simulate_click(gpui::point(gpui::px(w - 16.0), gpui::px(h - 14.0)), gpui::Modifiers::default());
    let (_, quads) = chip_frame(cx);
    let open = panel(&quads).expect("a click on the chip opens its details");
    assert!(open.x >= 0.0 && open.y >= 0.0 && open.x + open.w <= w + 0.5, "inside the window: {open:?}");
    assert!(open.y + open.h <= h - 28.0 + 0.5 && open.h > 100.0, "above the status bar, whole (not cut by the bar): {open:?}");
    // The window making the screen match its model again keeps the chip's copy, and so what it shows.
    cx.update(|window, cx| view.update(cx, |this, cx| this.sync(window, cx)));
    assert_eq!(panel(&chip_frame(cx).1), Some(open), "still open after a sync");
    // `yuzora-perf.toggle` reaches it, a shown copy.
    assert!(crate::mods::send_command("yuzora-perf", "toggle"), "the chip's copy takes the command");
    assert_eq!(panel(&chip_frame(cx).1), None, "toggled closed");
}

#[test]
fn in_win98s_taskbar_the_chip_is_its_gray_and_keeps_its_size_zoomed_in() {
    in_own_process("perf_chip_in_win98");
}

#[gpui::test]
fn perf_chip_in_win98(cx: &mut gpui::TestAppContext) {
    if !running_alone("perf_chip_in_win98") {
        return;
    }
    cx.update(gpui_base::init);
    // A taskbar named as win98's, so the chip paints as win98's taskbar does.
    let layout = serde_json::json!({ "id": "root", "type": "column", "theme": "yuzora-dark", "children": [
        { "id": "main", "type": "block", "content": { "tab": "active" } },
        { "id": "taskbar", "type": "block", "size": { "px": 40 }, "content": { "mod": "win98" } }] });
    let (view, cx) = open_window(cx, "win98", CHIP_BAR_JS, layout);
    let lines = |quads: &[ChipQuad], chip: ChipQuad| -> Vec<(f32, gpui::Background)> {
        let mut top: Vec<_> = quads.iter().filter(|q| chip_near(q.x, chip.x) && chip_near(q.w, chip.w) && q.h < 4.0).map(|q| (q.y - chip.y, q.h, q.bg)).collect();
        top.sort_by(|a, b| a.0.total_cmp(&b.0));
        top.into_iter().map(|(_, h, bg)| (h, bg)).collect()
    };
    let (_, quads) = chip_frame(cx);
    let bar = chip_bounds(&view, cx, "taskbar").unwrap();
    let chip = chip_drawn(&quads, bar, chip_rgb(0xc0c0c0)).unwrap_or_else(|| panic!("the taskbar's gray: {quads:?}"));
    assert_eq!(lines(&quads, chip), [(1.0, chip_rgb(0xdfdfdf)), (1.0, chip_rgb(0xffffff))], "the taskbar's raised top edge");
    // Zoomed in, win98's taskbar keeps its size in Win98 pixels, and the chip keeps its size with it.
    assert!(crate::mods::send_command("yuzora-perf", "toggle"));
    let pill = |quads: &[ChipQuad]| quads.iter().filter(|q| q.bg == chip_theme("selected") && q.w < 150.0).map(|q| q.h).next();
    let at_one = pill(&chip_frame(cx).1).expect("the open chip's pill");
    // To 2.0, where a line of one logical pixel would be drawn two pixels high.
    for _ in 0..10 {
        chip_run(&view, cx, "app.zoom_in", serde_json::json!({}));
    }
    let zoom = super::settings::zoom();
    assert!((zoom - 2.0).abs() < 0.01, "zoomed in: {zoom}");
    let (_, quads) = chip_frame(cx);
    assert_eq!(pill(&quads), Some(at_one), "the pill at zoom {zoom}");
    let bar = chip_bounds(&view, cx, "taskbar").unwrap();
    let chip = chip_drawn(&quads, bar, chip_rgb(0xc0c0c0)).expect("still the taskbar's gray");
    assert!(lines(&quads, chip).iter().all(|l| (l.0 - 1.0).abs() < 0.05), "the edge in Win98 pixels: {:?}", lines(&quads, chip));
}

/// Every layout file and `.state` a person's work leaves, in order, and what `layout()` read after each step: a click
/// saved, a mod docked at a side, a layout terminal that exits (its slot removed, as `terminal_ended` does), and a
/// mod's layout.set of `layout()` wrapped in a row. Run on a fresh copy of `file`. Also whether the chip showed.
fn chip_scenario(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, file: &serde_json::Value) -> Vec<(String, String, String, bool)> {
    use serde_json::json;
    std::fs::write(super::layout_path("test"), file.to_string()).unwrap();
    let _ = std::fs::remove_file(super::state_path("test"));
    let read = |view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext| {
        chip_frame(cx);
        let layout = view.update(cx, |this, _| serde_json::to_string(&this.chrome).unwrap());
        let chip = !chip_copies(view, cx).is_empty();
        (std::fs::read_to_string(super::layout_path("test")).unwrap(), std::fs::read_to_string(super::state_path("test")).unwrap_or_default(), layout, chip)
    };
    let mut seen = Vec::new();
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.load_layout("test");
        this.sync(window, cx);
    }));
    seen.push(read(view, cx));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.find_node_mut("status").unwrap().size = Some(super::Span::Px { px: 40.0 });
        this.save();
        this.sync(window, cx);
    }));
    seen.push(read(view, cx));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        let mut block = crate::layout::Node::block("docked", crate::layout::Content::Mod { name: "unused".into(), props: serde_json::Value::Null });
        block.size = Some(super::Span::Px { px: 320.0 });
        super::dock(&mut this.chrome, block, "left", "docked-row".into());
        this.save();
        this.sync(window, cx);
    }));
    seen.push(read(view, cx));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        assert!(this.chrome.remove_slot("term").is_some(), "the exited terminal's slot");
        this.sync(window, cx);
        this.save();
    }));
    seen.push(read(view, cx));
    let wrapped = view.update(cx, |this, _| json!({ "id": "wrap", "type": "row", "children": [serde_json::to_value(&this.chrome).unwrap()] }));
    chip_run(view, cx, "layout.set", json!({ "layout": wrapped }));
    seen.push(read(view, cx));
    seen
}

#[test]
fn the_chip_leaves_the_layout_file_and_its_clicks_as_without_it() {
    in_own_process("perf_chip_leaves_files_alone");
}

#[gpui::test]
fn perf_chip_leaves_files_alone(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("perf_chip_leaves_files_alone") {
        return;
    }
    // `main`'s shape: the work area, a layout terminal under it, and the status bar last.
    let file = json!({ "id": "root", "type": "column", "children": [
        { "id": "work", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "hidden": true, "size": { "px": 368 }, "children": [
                { "id": "panel-body", "type": "stack", "active": "files", "children": [
                    { "id": "files", "type": "block", "content": { "mod": "files" } } ] } ] } ] },
        { "id": "term", "type": "block", "size": { "px": 200 }, "content": { "terminal": "t-term" } },
        { "id": "status", "type": "block", "size": { "px": 28 }, "content": { "mod": "bar" } } ] });
    let (view, cx) = open_window(cx, "bar", CHIP_BAR_JS, file.clone());
    crate::yuzora::perf_chip::set_for_tests(false);
    let without = chip_scenario(&view, cx, &file);
    crate::yuzora::perf_chip::set_for_tests(true);
    let with = chip_scenario(&view, cx, &file);
    for (step, (w, wo)) in ["shown", "a click saved", "a mod docked", "a terminal exited", "a wrapped layout.set"].iter().zip(with.iter().zip(&without)) {
        assert_eq!((&w.0, &w.1, &w.2), (&wo.0, &wo.1, &wo.2), "{step}: the layout file, .state and layout() are what the window writes without the chip");
        for text in [&w.0, &w.1, &w.2] {
            assert!(!text.contains("yuzora-perf") && !text.contains("yuzora-band") && !text.contains("yuzora-dock-perf"), "{step}: {text}");
        }
        assert!(w.3 && !wo.3, "{step}: the chip showed with it, not without");
    }
    assert!(with[1].1.contains("40"), "the click is kept: {}", with[1].1);
    assert!(with[2].0.contains("docked") && with[4].0.contains("\"wrap\""), "the edits are written: {:?}", with[4].0);
}
