// yuzora(L8): included into `window::tests`, in a file of its own so upstream's new tests in window.rs never meet
// these in a merge. The resource chip's band: the window draws it under the layout, and no tree, file or click
// ever holds it.

/// A personal layout with no chip of its own: one terminal.
fn band_lone_terminal() -> serde_json::Value {
    serde_json::json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } })
}

/// The band's copy, while the window keeps one.
fn band_copy(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext) -> Option<crate::mods::Instance> {
    view.update(cx, |this, _| this.copies.get(crate::yuzora::band::SLOT).map(|c| c.inst.clone()))
}

fn band_draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The window's size, and every quad the last frame painted that is not fully transparent: x, y, width, height and
/// its background.
fn band_frame(cx: &mut gpui::VisualTestContext) -> ((f32, f32), Vec<(f32, f32, f32, f32, gpui::Background)>) {
    band_draw(cx);
    cx.update(|window, _| {
        let scale = window.scale_factor();
        let v = window.viewport_size();
        let quads = window
            .painted_quads()
            .into_iter()
            .filter(|q| !q.background.is_transparent())
            .map(|q| (q.bounds.origin.x.as_f32() / scale, q.bounds.origin.y.as_f32() / scale, q.bounds.size.width.as_f32() / scale, q.bounds.size.height.as_f32() / scale, q.background))
            .collect();
        ((v.width.as_f32(), v.height.as_f32()), quads)
    })
}

/// The band as drawn: the height and color of the quad as wide as the window that ends at its bottom edge.
fn band_drawn(cx: &mut gpui::VisualTestContext) -> Option<(f32, gpui::Background)> {
    let ((w, h), quads) = band_frame(cx);
    quads.into_iter().find(|q| q.0.abs() < 0.5 && (q.2 - w).abs() < 0.5 && (q.1 + q.3 - h).abs() < 0.5 && q.3 < 60.0).map(|q| (q.3, q.4))
}

/// The shown theme's color `name`, as a mod paints it.
fn band_theme(name: &str) -> gpui::Background {
    let hex = super::settings::theme()[name].clone();
    gpui::rgb(u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap()).into()
}

/// Where on screen the layout's root `id` ends.
fn band_root_bottom(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, id: &str) -> f32 {
    band_draw(cx);
    view.update(cx, |this, _| this.bounds.borrow().get(id).map(|b| f32::from(b.bottom())).expect("the root is drawn"))
}

fn band_set(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, tree: serde_json::Value) {
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", serde_json::json!({ "layout": tree }), window, cx)));
}

#[test]
fn the_band_shows_under_a_layout_with_no_chip_whatever_the_dock() {
    in_own_process("band_shows_without_a_chip");
}

#[gpui::test]
fn band_shows_without_a_chip(cx: &mut gpui::TestAppContext) {
    if !running_alone("band_shows_without_a_chip") {
        return;
    }
    let (view, cx) = open_window(cx, "unused", "", band_lone_terminal());
    let copy = band_copy(&view, cx).expect("the band's copy");
    assert_eq!((copy.name.as_str(), &copy.props), ("yuzora-perf", &serde_json::json!({ "band": true })));
    let viewport = band_frame(cx).0;
    assert_eq!(band_drawn(cx), Some((26.0, band_theme("chrome"))), "a 26 px band across the window's bottom, in the bars' color");
    assert_eq!(band_root_bottom(&view, cx, "pane"), viewport.1 - 26.0, "the layout ends where the band starts");
    view.update(cx, |this, _| {
        let shown = serde_json::to_string(&this.chrome).unwrap();
        assert!(!shown.contains("yuzora-band") && !shown.contains("yuzora-perf"), "layout() never has it: {shown}");
    });

    // The dock's choice, `off` too, is not the band's.
    for dock in ["off", "drawer"] {
        let mut edited = band_lone_terminal();
        edited["yuzora"] = serde_json::json!({ "dock": dock });
        std::fs::write(super::layout_path("test"), edited.to_string()).unwrap();
        cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
        assert!(band_copy(&view, cx).is_some(), "dock {dock}");
        assert_eq!(band_drawn(cx).map(|b| b.0), Some(26.0), "dock {dock}");
    }

    // Under win98's theme, whose surfaces have a face of their own, the band is that gray, as its taskbar.
    let mut win98 = band_lone_terminal();
    win98["theme"] = serde_json::json!("win98");
    std::fs::write(super::layout_path("test"), win98.to_string()).unwrap();
    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
    assert_eq!(band_drawn(cx), Some((26.0, gpui::rgb(0xc0c0c0).into())), "win98's face");

    // It is as tall as the zoom makes the rest.
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("app.zoom_in", serde_json::json!({}), window, cx)));
    let zoom = super::settings::zoom();
    assert!(zoom > 1.0, "zoomed in: {zoom}");
    let drawn = band_drawn(cx).expect("still drawn").0;
    assert!((drawn - 26.0 * zoom).abs() < 0.5, "{drawn} at zoom {zoom}");
}

#[test]
fn a_chip_the_layout_places_itself_replaces_the_band() {
    in_own_process("band_gives_way_to_a_placed_chip");
}

#[gpui::test]
fn band_gives_way_to_a_placed_chip(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("band_gives_way_to_a_placed_chip") {
        return;
    }
    let row = json!({ "id": "root", "type": "row", "children": [{ "id": "pane", "type": "block", "content": { "terminal": "t1" } }] });
    let (view, cx) = open_window(cx, "unused", "", row.clone());
    let viewport = band_frame(cx).0;
    assert!(band_copy(&view, cx).is_some());

    // A mod's layout.set adds the chip as a block: the band goes, the block shows it.
    let mut with_block = row.clone();
    with_block["children"].as_array_mut().unwrap().push(json!({ "id": "mine", "type": "block", "size": { "px": 160 }, "content": { "mod": "yuzora-perf" } }));
    band_set(&view, cx, with_block);
    assert!(band_copy(&view, cx).is_none(), "no band beside a placed chip");
    assert_eq!(band_drawn(cx), None);
    assert_eq!(band_root_bottom(&view, cx, "root"), viewport.1, "the layout reaches the window's bottom");
    view.update(cx, |this, _| assert!(this.copies.get("mine").is_some_and(|c| c.inst.name == "yuzora-perf"), "the placed chip runs"));

    // And back when it is taken out again.
    band_set(&view, cx, row.clone());
    assert!(band_copy(&view, cx).is_some());
    assert_eq!(band_drawn(cx).map(|b| b.0), Some(26.0));
    assert_eq!(band_root_bottom(&view, cx, "root"), viewport.1 - 26.0);

    // In a float, hidden or not, it is placed too.
    let mut floated = row.clone();
    floated["floating"] = json!([{ "id": "corner", "content": { "mod": "yuzora-perf" }, "edge": "bottom", "align": "end", "hidden": true }]);
    band_set(&view, cx, floated);
    assert!(band_copy(&view, cx).is_none(), "a float");
    assert_eq!(band_drawn(cx), None);
    band_set(&view, cx, row);
    assert!(band_copy(&view, cx).is_some());
}

/// Every layout file and `.state` a person's work leaves, in order, and what `layout()` read after each step: a
/// click saved, a mod docked at a side, a layout terminal that exits (its slot removed, as `terminal_ended` does),
/// and a mod's layout.set of `layout()` wrapped in a row. Run on a fresh copy of `file`.
fn band_scenario(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext, file: &serde_json::Value) -> Vec<(String, String, String)> {
    use serde_json::json;
    std::fs::write(super::layout_path("test"), file.to_string()).unwrap();
    let _ = std::fs::remove_file(super::state_path("test"));
    let read = |view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext| {
        band_draw(cx);
        let layout = view.update(cx, |this, _| serde_json::to_string(&this.chrome).unwrap());
        (std::fs::read_to_string(super::layout_path("test")).unwrap(), std::fs::read_to_string(super::state_path("test")).unwrap_or_default(), layout)
    };
    let mut seen = Vec::new();
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.load_layout("test");
        this.sync(window, cx);
    }));
    seen.push(read(view, cx));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.find_node_mut("status").unwrap().size = Some(super::Span::Px { px: 52.0 });
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
        assert!(this.chrome.remove_slot("status").is_some(), "the status terminal's slot");
        this.sync(window, cx);
        this.save();
    }));
    seen.push(read(view, cx));
    let wrapped = view.update(cx, |this, _| json!({ "id": "wrap", "type": "row", "children": [serde_json::to_value(&this.chrome).unwrap()] }));
    band_set(view, cx, wrapped);
    seen.push(read(view, cx));
    seen
}

#[test]
fn the_band_leaves_the_layout_file_and_its_clicks_as_without_it() {
    in_own_process("band_leaves_files_alone");
}

#[gpui::test]
fn band_leaves_files_alone(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("band_leaves_files_alone") {
        return;
    }
    // `main`'s shape (a 40 px status bar under the work area), its status a terminal of the layout.
    let file = json!({ "id": "root", "type": "column", "children": [
        { "id": "work", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "hidden": true, "size": { "px": 368 }, "children": [
                { "id": "panel-body", "type": "stack", "active": "files", "children": [
                    { "id": "files", "type": "block", "content": { "mod": "files" } } ] } ] } ] },
        { "id": "status", "type": "block", "size": { "px": 40 }, "content": { "terminal": "t-status" } } ] });
    let (view, cx) = open_window(cx, "unused", "", file.clone());
    crate::yuzora::band::set_for_tests(false);
    let without = band_scenario(&view, cx, &file);
    assert!(band_copy(&view, cx).is_none());
    crate::yuzora::band::set_for_tests(true);
    let with = band_scenario(&view, cx, &file);
    for (step, (w, wo)) in ["shown", "a click saved", "a mod docked", "a terminal exited", "a wrapped layout.set"].iter().zip(with.iter().zip(&without)) {
        assert_eq!(w, wo, "{step}: the layout file, .state and layout() are what the window writes without the band");
        for text in [&w.0, &w.1, &w.2] {
            assert!(!text.contains("yuzora-band") && !text.contains("yuzora-dock-perf") && !text.contains("yuzora-perf"), "{step}: {text}");
        }
    }
    assert!(with[1].1.contains("52"), "the click is kept: {}", with[1].1);
    assert!(with[2].0.contains("docked") && with[4].0.contains("\"wrap\""), "the edits are written: {:?}", with[4].0);
    assert!(band_copy(&view, cx).is_some(), "the band showed throughout");
}

#[test]
fn a_click_on_the_bands_chip_opens_its_details() {
    in_own_process("band_chip_click");
}

#[gpui::test]
fn band_chip_click(cx: &mut gpui::TestAppContext) {
    if !running_alone("band_chip_click") {
        return;
    }
    cx.update(gpui_base::init);
    let (view, cx) = open_window(cx, "unused", "", band_lone_terminal());
    let ((w, h), quads) = band_frame(cx);
    // The details: a 420 px panel, wider than anything else the chip draws.
    let panel = |quads: &[(f32, f32, f32, f32, gpui::Background)]| quads.iter().find(|q| (q.2 - 420.0).abs() < 0.5).map(|q| (q.0, q.1, q.2, q.3));
    assert_eq!(panel(&quads), None, "closed at first");
    // The chip's caret, at the band's right end.
    cx.simulate_click(gpui::point(gpui::px(w - 16.0), gpui::px(h - 13.0)), gpui::Modifiers::default());
    let (_, quads) = band_frame(cx);
    let open = panel(&quads).expect("a click on the chip opens its details");
    assert!(open.1 >= 0.0 && open.0 >= 0.0 && open.0 + open.2 <= w + 0.5, "inside the window: {open:?}");
    assert!(open.1 + open.3 <= h - 26.0 && open.3 > 100.0, "above the band, whole: {open:?}");
    // The window making the screen match its model again keeps the band's copy, and so what it shows.
    cx.update(|window, cx| view.update(cx, |this, cx| this.sync(window, cx)));
    assert_eq!(panel(&band_frame(cx).1), Some(open), "still open after a sync");
    // `yuzora-perf.toggle` reaches the band's copy, a shown one.
    assert!(crate::mods::send_command("yuzora-perf", "toggle"), "the band's copy takes the command");
    let (_, quads) = band_frame(cx);
    assert_eq!(panel(&quads), None, "toggled closed");
}
