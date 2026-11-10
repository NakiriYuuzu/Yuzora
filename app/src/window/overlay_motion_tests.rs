// yuzora(motion): included into `window::tests`. Dialogs, popups and toasts arrive with a short fade and a small
// drop (docs/beta2/motion.md). What is observed is what the last frame painted (`painted_quads`), on a clock the
// test moves, so a frame is a point in time and nothing here depends on how fast the machine is.

/// What a dialog or popup shows: a red 100x40 button in a 90 px tall body. Its clicks are counted in `probe.clicks`.
const ARRIVAL_PROBE: &str = r##"
import { View, div } from "gpui-kit";
import { setShared, shared } from "herdr";
export default class Probe extends View {
  render() {
    return div().id("probe").w_full().h(90).child(
      div().id("hit").absolute().left(20).top(30).w(100).h(40).bg("#ff0000")
        .on_click(() => setShared("probe.clicks", (shared("probe.clicks") ?? 0) + 1)));
  }
}
"##;

/// A text field with the keyboard from the start, like the palette: what is typed lands in `typer.text`.
const ARRIVAL_TYPER: &str = r##"
import { View, div } from "gpui-kit";
import { InputState, Input } from "gpui-base";
import { setShared } from "herdr";
export default class Typer extends View {
  init(_props, cx) {
    this.input = InputState.new({ placeholder: "type" });
    this.input.on("change", () => setShared("typer.text", this.input.value()));
    this.input.focus();
  }
  render() {
    return div().id("typer").w_full().h(90).child(Input.new(this.input).w_full().h(30));
  }
}
"##;

const ARRIVAL_PANEL_MS: u64 = 120;
const ARRIVAL_TOAST_MS: u64 = 160;

/// A painted quad in logical pixels: x, y, width, height, fill.
type ArrivalQuad = (f32, f32, f32, f32, gpui::Background);

fn arrival_mod(name: &str, js: &str) {
    let folder = crate::mods::app_dir().join("mods").join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("mod.json"), r#"{ "place": "none" }"#).unwrap();
    std::fs::write(folder.join("main.js"), js).unwrap();
}

/// A window with the mods an arrival needs, showing a terminal pane (so what a pane does while an overlay arrives
/// can be seen), and nothing else on top of it: no "Updated to" pill, and none of the toasts a new window opens
/// (the terminal that cannot start with no core), which would take the places the toasts under test come to.
fn arrival_window(
    cx: &mut gpui::TestAppContext,
) -> (gpui::Entity<super::AppWindow>, &mut gpui::VisualTestContext) {
    arrival_mod("probe", ARRIVAL_PROBE);
    arrival_mod("typer", ARRIVAL_TYPER);
    cx.update(gpui_base::init);
    let (view, cx) = open_window(
        cx,
        "unused",
        "",
        serde_json::json!({ "id": "pane", "type": "block", "content": { "terminal": "t1" } }),
    );
    // The window installs the shipped `toast` mod over any of that name; a toast draws what is there when it opens.
    arrival_mod("toast", ARRIVAL_PROBE);
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.update_success = false;
            let open: Vec<String> = this.overlays.iter().map(|o| o.id.clone()).collect();
            for id in open {
                this.remove_overlay(&id, window, cx);
            }
        })
    });
    arrival_settle(cx);
    (view, cx)
}

fn arrival_draw(cx: &mut gpui::VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// Draws until nothing is waiting on a task, without the clock moving.
fn arrival_settle(cx: &mut gpui::VisualTestContext) {
    for _ in 0..3 {
        arrival_draw(cx);
        cx.run_until_parked();
    }
}

/// The clock moves on by `ms` and the frame GPUI asks for comes: how many animation frames the frame before asked for.
fn arrival_frame(cx: &mut gpui::VisualTestContext, ms: u64) -> usize {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(ms));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let asked = window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
        asked
    })
}

fn arrival_open(
    view: &gpui::Entity<super::AppWindow>,
    cx: &mut gpui::VisualTestContext,
    id: &str,
    spec: serde_json::Value,
) {
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.open_overlay(id.into(), spec, window, cx)
        })
    });
    arrival_settle(cx);
}

fn arrival_close(
    view: &gpui::Entity<super::AppWindow>,
    cx: &mut gpui::VisualTestContext,
    id: &str,
) {
    cx.update(|window, cx| view.update(cx, |this, cx| this.remove_overlay(id, window, cx)));
    arrival_settle(cx);
}

fn arrival_quads(cx: &mut gpui::VisualTestContext) -> Vec<ArrivalQuad> {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        window
            .painted_quads()
            .into_iter()
            .map(|q| {
                let b = q.bounds;
                (
                    b.origin.x.as_f32() / scale,
                    b.origin.y.as_f32() / scale,
                    b.size.width.as_f32() / scale,
                    b.size.height.as_f32() / scale,
                    q.background,
                )
            })
            .collect()
    })
}

fn arrival_viewport(cx: &mut gpui::VisualTestContext) -> (f32, f32) {
    cx.update(|window, _| {
        let v = window.viewport_size();
        (v.width.as_f32(), v.height.as_f32())
    })
}

/// The quad `w` px wide (the body of an overlay is given its width), if one was painted.
fn arrival_wide(quads: &[ArrivalQuad], w: f32) -> Option<ArrivalQuad> {
    quads
        .iter()
        .find(|q| (q.2 - w).abs() < 0.01 && q.3 > 20.0)
        .cloned()
}

/// The red button inside the probe, in whatever share of its color it is painted.
fn arrival_button(quads: &[ArrivalQuad]) -> Option<ArrivalQuad> {
    quads
        .iter()
        .find(|q| (q.2 - 100.0).abs() < 0.01 && (q.3 - 40.0).abs() < 0.01 && !q.4.is_transparent())
        .cloned()
}

/// The dim layer behind a dialog, as the window has always drawn it.
fn arrival_dim() -> gpui::Background {
    gpui::hsla(0.0, 0.0, 0.0, 0.35).into()
}

/// A layer as big as the window painted in `color`.
fn arrival_layer(
    quads: &[ArrivalQuad],
    viewport: (f32, f32),
    color: gpui::Background,
) -> Option<ArrivalQuad> {
    quads
        .iter()
        .find(|q| {
            (q.2 - viewport.0).abs() < 0.01 && (q.3 - viewport.1).abs() < 0.01 && q.4 == color
        })
        .cloned()
}

/// How many times the app has run a mod's script to draw it.
fn arrival_scripts(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext) -> u64 {
    view.read_with(cx, |this, _| {
        this.runtime
            .as_ref()
            .expect("the mods' runtime")
            .read_metrics()
            .script_renders()
    })
}

fn arrival_set_reduce_motion(cx: &mut gpui::VisualTestContext, on: bool) {
    cx.update(|_, cx| cx.set_reduce_motion(on));
}

/// What opens with each kind of overlay, where each one comes to rest (read off the design, not off a run), and the
/// size its body is given.
struct ArrivalCase {
    id: &'static str,
    spec: serde_json::Value,
    body_width: f32,
    rests_at: fn((f32, f32)) -> (f32, f32),
    duration_ms: u64,
}

fn arrival_cases() -> Vec<ArrivalCase> {
    use serde_json::json;
    vec![
        ArrivalCase {
            id: "dialog",
            spec: json!({ "kind": "dialog", "mod": "probe", "width": 300, "height": 120 }),
            body_width: 300.0,
            rests_at: |v| ((v.0 - 300.0) / 2.0, (v.1 - 120.0) / 2.0),
            duration_ms: ARRIVAL_PANEL_MS,
        },
        ArrivalCase {
            id: "popup",
            spec: json!({ "kind": "popup", "mod": "probe", "width": 260, "at": { "x": 200, "y": 150 } }),
            body_width: 260.0,
            rests_at: |_| (200.0, 150.0),
            duration_ms: ARRIVAL_PANEL_MS,
        },
        ArrivalCase {
            id: "toast",
            spec: json!({ "kind": "toast", "notification": { "title": "Saved" } }),
            body_width: 340.0,
            rests_at: |v| (v.0 - 16.0 - 340.0, v.1 - 16.0 - 72.0),
            duration_ms: ARRIVAL_TOAST_MS,
        },
    ]
}

fn arrival_near(got: f32, want: f32, tolerance: f32, what: &str) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: {got}, wanted {want} (+-{tolerance})"
    );
}

/// A dialog, a popup at its anchor and a toast: nothing at the first frame, the eased share of the fade and of the
/// drop half way, and exactly the place and colors they had before once it is over.
#[test]
fn a_dialog_a_popup_and_a_toast_arrive_and_come_to_rest_where_they_always_did() {
    in_own_process("overlay_arrival_shapes");
}

#[gpui::test]
fn overlay_arrival_shapes(cx: &mut gpui::TestAppContext) {
    if !running_alone("overlay_arrival_shapes") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let viewport = arrival_viewport(cx);
    for case in arrival_cases() {
        arrival_set_reduce_motion(cx, true);
        arrival_open(&view, cx, case.id, case.spec.clone());
        let rest = arrival_quads(cx);
        let body = arrival_wide(&rest, case.body_width)
            .unwrap_or_else(|| panic!("{}: no body painted at rest", case.id));
        let want = (case.rests_at)(viewport);
        arrival_near(body.0, want.0, 0.5, &format!("{} rests at x", case.id));
        arrival_near(body.1, want.1, 0.5, &format!("{} rests at y", case.id));
        let red: gpui::Background = gpui::rgb(0xff0000).into();
        assert_eq!(
            arrival_button(&rest).map(|q| q.4),
            Some(red),
            "{}: its mod is painted at rest, in full color",
            case.id
        );
        arrival_close(&view, cx, case.id);

        arrival_set_reduce_motion(cx, false);
        arrival_open(&view, cx, case.id, case.spec.clone());
        let first = arrival_quads(cx);
        assert!(
            arrival_wide(&first, case.body_width).is_none_or(|q| q.4.is_transparent()),
            "{}: the first frame still shows the body",
            case.id
        );
        assert!(
            arrival_button(&first).is_none(),
            "{}: the first frame still shows the mod",
            case.id
        );
        if case.id == "dialog" {
            assert!(
                arrival_layer(&first, viewport, arrival_dim()).is_none(),
                "the dim layer is already all there at the first frame"
            );
        }

        let scripts = arrival_scripts(&view, cx);
        let asked = arrival_frame(cx, case.duration_ms / 2);
        assert!(
            asked > 0,
            "{}: an arriving overlay asks for the next frame",
            case.id
        );
        let f = gpui_base::animation::ease_out_cubic(0.5);
        let mid = arrival_quads(cx);
        let body_mid = arrival_wide(&mid, case.body_width)
            .unwrap_or_else(|| panic!("{}: no body half way", case.id));
        assert_eq!(
            body_mid.4,
            body.4.opacity(f),
            "{}: the body is the eased share of its color half way",
            case.id
        );
        let button_mid =
            arrival_button(&mid).unwrap_or_else(|| panic!("{}: no mod content half way", case.id));
        assert_eq!(
            button_mid.4,
            red.opacity(f),
            "{}: what a mod draws fades with its overlay, whatever its views cached",
            case.id
        );
        let rest_button = arrival_button(&rest).unwrap();
        assert_eq!(
            button_mid.0, rest_button.0,
            "{}: it drops straight",
            case.id
        );
        let drop = 6.0 * (1.0 - f);
        let sign = if case.id == "toast" { 1.0 } else { -1.0 };
        arrival_near(
            body_mid.1,
            body.1 + sign * drop,
            0.5,
            &format!("{} is still short of its place half way", case.id),
        );
        arrival_near(
            button_mid.1 - body_mid.1,
            rest_button.1 - body.1,
            0.01,
            &format!("{}: its content moves with it", case.id),
        );
        if case.id == "dialog" {
            assert!(
                arrival_layer(&rest, viewport, arrival_dim()).is_some(),
                "the dim layer at rest"
            );
            assert!(
                arrival_layer(&mid, viewport, arrival_dim().opacity(f)).is_some(),
                "the dim layer fades in with the dialog"
            );
        }

        arrival_frame(cx, case.duration_ms);
        let end = arrival_quads(cx);
        assert_eq!(
            end.len(),
            rest.len(),
            "{}: the same primitives as at rest",
            case.id
        );
        for (got, was) in end.iter().zip(&rest) {
            assert_eq!(
                (got.0, got.1, got.2, got.3),
                (was.0, was.1, was.2, was.3),
                "{}: a quad ended away from where it rests",
                case.id
            );
            assert_eq!(
                got.4, was.4,
                "{}: a quad ended in another color than it rests in",
                case.id
            );
        }
        assert_eq!(
            arrival_frame(cx, 16),
            0,
            "{}: nothing asks for a frame once it has arrived",
            case.id
        );
        assert_eq!(
            arrival_scripts(&view, cx),
            scripts,
            "{}: an arrival runs the mod's script again",
            case.id
        );
        arrival_close(&view, cx, case.id);
    }
}

/// A popup the window's edge pushes in is pushed in the same way once it has arrived.
#[test]
fn a_popup_that_the_window_edge_pushes_in_rests_where_it_was_pushed() {
    in_own_process("overlay_arrival_edge");
}

#[gpui::test]
fn overlay_arrival_edge(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("overlay_arrival_edge") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let (vw, vh) = arrival_viewport(cx);
    let spec = json!({ "kind": "popup", "mod": "probe", "width": 260, "at": { "x": vw - 30.0, "y": vh - 20.0 } });
    arrival_set_reduce_motion(cx, true);
    arrival_open(&view, cx, "edge", spec.clone());
    let rest = arrival_quads(cx);
    let body = arrival_wide(&rest, 260.0).expect("the popup at rest");
    arrival_near(
        body.0 + body.2,
        vw - 8.0,
        0.5,
        "pushed in from the right edge, with its margin",
    );
    arrival_near(
        body.1 + body.3,
        vh - 8.0,
        0.5,
        "pushed in from the bottom edge, with its margin",
    );
    arrival_close(&view, cx, "edge");
    arrival_set_reduce_motion(cx, false);
    arrival_open(&view, cx, "edge", spec);
    arrival_frame(cx, 30);
    arrival_frame(cx, 200);
    let end = arrival_wide(&arrival_quads(cx), 260.0).expect("the popup after arriving");
    assert_eq!(
        (end.0, end.1, end.2, end.3),
        (body.0, body.1, body.2, body.3)
    );
}

/// It is played when an overlay is added, and not by anything that happens to an overlay that is there.
#[test]
fn an_overlay_arrives_once_and_again_each_time_it_is_added() {
    in_own_process("overlay_arrival_once");
}

#[gpui::test]
fn overlay_arrival_once(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("overlay_arrival_once") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let spec = json!({ "kind": "dialog", "mod": "probe", "width": 300, "height": 120 });
    let shown = |cx: &mut gpui::VisualTestContext| {
        arrival_wide(&arrival_quads(cx), 300.0).filter(|q| !q.4.is_transparent())
    };

    arrival_open(&view, cx, "once", spec.clone());
    assert!(shown(cx).is_none(), "added: starts from nothing");
    arrival_frame(cx, 400);
    let rest = shown(cx).expect("arrived");

    arrival_frame(cx, 16);
    cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
    assert_eq!(
        arrival_frame(cx, 16),
        0,
        "a render of the window asks for no frames"
    );
    assert_eq!(
        shown(cx).unwrap().4,
        rest.4,
        "a render of the window does not play it again"
    );
    cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(700.0)));
    arrival_settle(cx);
    assert_eq!(arrival_frame(cx, 16), 0, "a resize asks for no frames");
    assert!(
        shown(cx).is_some_and(|q| !q.4.is_transparent() && q.4 == rest.4),
        "a resize does not play it again"
    );
    cx.update(|window, cx| view.update(cx, |this, cx| window.focus(&this.focus, cx)));
    arrival_settle(cx);
    assert_eq!(
        arrival_frame(cx, 16),
        0,
        "a change of focus asks for no frames"
    );
    assert_eq!(
        shown(cx).unwrap().4,
        rest.4,
        "a change of focus does not play it again"
    );
    cx.update(|_, cx| {
        view.update(cx, |this, cx| {
            for copy in this.copies.values() {
                if let Some(root) = &copy.root {
                    root.update(cx, |_, cx| cx.notify());
                }
            }
        })
    });
    arrival_settle(cx);
    assert_eq!(arrival_frame(cx, 16), 0, "new content asks for no frames");
    assert_eq!(
        shown(cx).unwrap().4,
        rest.4,
        "new content does not play it again"
    );

    arrival_close(&view, cx, "once");
    arrival_open(&view, cx, "once", spec.clone());
    assert!(
        shown(cx).is_none(),
        "added again: starts from nothing again"
    );
    assert!(arrival_frame(cx, 16) > 0, "and is on its way");
    arrival_frame(cx, 400);
    assert!(shown(cx).is_some(), "and arrives again");

    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.remove_overlay("once", window, cx);
            this.open_overlay("once".into(), spec.clone(), window, cx);
        })
    });
    arrival_draw(cx);
    assert!(
        shown(cx).is_none(),
        "taken out and put back before a frame was drawn: still a new overlay"
    );
    arrival_frame(cx, 400);
    assert!(shown(cx).is_some());
}

/// With the system asking for less motion an overlay is there at once, and nothing asks for frames.
#[test]
fn an_overlay_arrives_at_once_when_the_system_asks_for_less_motion() {
    in_own_process("overlay_arrival_reduced");
}

#[gpui::test]
fn overlay_arrival_reduced(cx: &mut gpui::TestAppContext) {
    if !running_alone("overlay_arrival_reduced") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let viewport = arrival_viewport(cx);
    arrival_set_reduce_motion(cx, true);
    for case in arrival_cases() {
        arrival_open(&view, cx, case.id, case.spec.clone());
        let quads = arrival_quads(cx);
        let body = arrival_wide(&quads, case.body_width)
            .unwrap_or_else(|| panic!("{}: not there at the first frame", case.id));
        assert!(
            !body.4.is_transparent(),
            "{}: not opaque at the first frame",
            case.id
        );
        let want = (case.rests_at)(viewport);
        arrival_near(
            body.1,
            want.1,
            0.5,
            &format!("{} is in its place at the first frame", case.id),
        );
        arrival_frame(cx, 16);
        assert_eq!(
            arrival_frame(cx, 16),
            0,
            "{}: frames follow one another, though nothing is animated",
            case.id
        );
        arrival_close(&view, cx, case.id);
    }
}

/// An overlay is there to be used from its first frame, in the place it is drawn: a click lands on the button where
/// it is seen, and what is typed goes in.
#[test]
fn an_arriving_overlay_takes_clicks_where_it_is_drawn_and_keys_at_once() {
    in_own_process("overlay_arrival_input");
}

#[gpui::test]
fn overlay_arrival_input(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("overlay_arrival_input") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let spec = json!({ "kind": "dialog", "mod": "probe", "width": 300, "height": 120 });
    arrival_set_reduce_motion(cx, true);
    arrival_open(&view, cx, "rest", spec.clone());
    let button = arrival_button(&arrival_quads(cx)).expect("the button at rest");
    arrival_close(&view, cx, "rest");
    let clicks = || {
        crate::mods::shared("probe.clicks")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    };
    let click = |cx: &mut gpui::VisualTestContext, x: f32, y: f32| {
        cx.simulate_click(
            gpui::point(gpui::px(x), gpui::px(y)),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
    };

    arrival_set_reduce_motion(cx, false);
    arrival_open(&view, cx, "moving", spec);
    let x = button.0 + 50.0;
    click(cx, x, button.1 + 37.0);
    assert_eq!(
        clicks(),
        0,
        "6 px below where it is drawn at the first frame is not on it, though it is where it will rest"
    );
    click(cx, x, button.1 - 3.0);
    assert_eq!(
        clicks(),
        1,
        "the first frame takes a click on the button where it is drawn"
    );
    arrival_frame(cx, 60);
    click(cx, x, button.1 + 20.0);
    assert_eq!(clicks(), 2, "and so does a frame on the way");
    arrival_frame(cx, 400);
    click(cx, x, button.1 + 20.0);
    assert_eq!(clicks(), 3);
    arrival_close(&view, cx, "moving");

    arrival_open(
        &view,
        cx,
        "typing",
        json!({ "kind": "popup", "mod": "typer", "width": 300, "at": "top", "offset": 60 }),
    );
    cx.simulate_input("go");
    cx.run_until_parked();
    assert_eq!(
        crate::mods::shared("typer.text"),
        Some(json!("go")),
        "typing at the first frame is not lost"
    );
    arrival_frame(cx, 60);
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(
        crate::mods::shared("typer.text"),
        Some(json!("go!")),
        "nor on the way"
    );
}

/// An overlay arriving moves nothing under it: from the frame that opened it on, the panes are not drawn again, so no
/// terminal is told another size.
#[test]
fn an_arriving_overlay_does_not_draw_or_resize_the_panes_under_it() {
    in_own_process("overlay_arrival_panes");
}

#[gpui::test]
fn overlay_arrival_panes(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    use std::sync::atomic::Ordering::Relaxed;
    if !running_alone("overlay_arrival_panes") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    arrival_frame(cx, 3000);
    arrival_open(
        &view,
        cx,
        "over",
        json!({ "kind": "dialog", "mod": "probe", "width": 300, "height": 120 }),
    );
    let before = (
        crate::bench::TERMINAL.load(Relaxed),
        crate::bench::GRID.load(Relaxed),
    );
    assert!(
        before.0 > 0 && before.1 > 0,
        "the pane is drawn in this window, or its counts prove nothing"
    );
    let pane = view.update(cx, |this, cx| this.terms["pane"].view.read(cx).dimensions());
    for _ in 0..12 {
        arrival_frame(cx, 12);
    }
    assert_eq!(
        (
            crate::bench::TERMINAL.load(Relaxed),
            crate::bench::GRID.load(Relaxed)
        ),
        before,
        "a pane under an arriving overlay was drawn again"
    );
    assert_eq!(
        view.update(cx, |this, cx| this.terms["pane"].view.read(cx).dimensions()),
        pane,
        "a pane under an arriving overlay changed its size"
    );
}

/// A window is put and stays, and an overlay of blocks holds views (a terminal's) that GPUI draws from caches of
/// their own: neither arrives. They are all there at the first frame, and stay as they are.
#[test]
fn a_window_and_an_overlay_of_blocks_do_not_arrive() {
    in_own_process("overlay_arrival_exempt");
}

#[gpui::test]
fn overlay_arrival_exempt(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("overlay_arrival_exempt") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let blocks = json!({ "kind": "dialog", "width": 300, "height": 120, "layout": { "id": "blk", "type": "block", "content": { "mod": "probe" } } });
    let window = json!({ "kind": "window", "mod": "probe", "width": 300, "height": 120 });
    for (id, spec) in [("blocks", blocks), ("window", window)] {
        arrival_open(&view, cx, id, spec);
        let quads = arrival_quads(cx);
        let body = arrival_wide(&quads, 300.0)
            .unwrap_or_else(|| panic!("{id}: not there at the first frame"));
        assert!(
            !body.4.is_transparent(),
            "{id}: not in full at the first frame"
        );
        assert!(
            arrival_button(&quads).is_some_and(|q| q.4 == gpui::rgb(0xff0000).into()),
            "{id}: what is drawn in it is not in full at the first frame"
        );
        arrival_frame(cx, 16);
        assert!(
            arrival_wide(&arrival_quads(cx), 300.0).is_some_and(|q| q.4 == body.4),
            "{id}: changes after its first frame"
        );
        arrival_close(&view, cx, id);
    }
}

/// An overlay whose mod did not load (it shows the reason in a box) arrives too, and each frame of it asks for the next
/// one itself: there is no mod view to ask for them.
#[test]
fn an_overlay_whose_mod_did_not_load_arrives_too() {
    in_own_process("overlay_arrival_unloaded");
}

#[gpui::test]
fn overlay_arrival_unloaded(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("overlay_arrival_unloaded") {
        return;
    }
    let (view, cx) = arrival_window(cx);
    let spec = json!({ "kind": "dialog", "mod": "not-there", "width": 300, "height": 120 });
    arrival_set_reduce_motion(cx, true);
    arrival_open(&view, cx, "unloaded", spec.clone());
    let body = arrival_wide(&arrival_quads(cx), 300.0).expect("the dialog at rest");
    arrival_close(&view, cx, "unloaded");
    arrival_set_reduce_motion(cx, false);
    arrival_open(&view, cx, "unloaded", spec);
    assert!(
        arrival_wide(&arrival_quads(cx), 300.0).is_none_or(|q| q.4.is_transparent()),
        "the first frame already shows it"
    );
    for step in 0..5 {
        assert!(
            arrival_frame(cx, ARRIVAL_PANEL_MS / 10) > 0,
            "frame {step} of the arrival does not ask for the next"
        );
    }
    let mid = arrival_wide(&arrival_quads(cx), 300.0).expect("the dialog half way");
    assert_eq!(
        mid.4,
        body.4.opacity(gpui_base::animation::ease_out_cubic(0.5))
    );
    arrival_frame(cx, ARRIVAL_PANEL_MS);
    assert_eq!(
        arrival_wide(&arrival_quads(cx), 300.0).map(|q| q.4),
        Some(body.4)
    );
    assert_eq!(arrival_frame(cx, 16), 0, "and stops once it has arrived");
}

/// A dialog with as many rows as a settings page has: what an arrival costs scales with what a frame redraws.
const ARRIVAL_HEAVY: &str = r##"
import { View, div } from "gpui-kit";
export default class Heavy extends View {
  render() {
    const rows = [];
    for (let i = 0; i < 300; i++) {
      rows.push(div().id("row-" + i).h(24).px(8).flex().items_center().bg(i % 2 ? "#202020" : "#262626").text_color("#dddddd").child("row " + i + " of a long list"));
    }
    return div().id("heavy").size_full().overflow_hidden().children(rows);
  }
}
"##;

/// What an arrival costs the main thread, as the time `Window::draw` takes (layout, prepaint and the scene it
/// builds; no GPU): the frame that opens a dialog, and the nine after it that a 120 ms arrival fills at 60 Hz, with
/// and without the system asking for less motion (which draws what the code drew before). One JSON line a round,
/// rounds alternating which comes first, to `$YUZORA_ARRIVAL_COST`.
///
///     YUZORA_ARRIVAL_COST=/tmp/cost.jsonl cargo test --bin herdr-app -- --ignored the_cost_of_an_arrival
#[test]
#[ignore = "a measurement, not a check"]
fn the_cost_of_an_arrival() {
    in_own_process("overlay_arrival_cost");
}

#[gpui::test]
fn overlay_arrival_cost(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    use std::io::Write;
    if !running_alone("overlay_arrival_cost") {
        return;
    }
    let Some(out) = std::env::var_os("YUZORA_ARRIVAL_COST") else {
        return;
    };
    arrival_mod("heavy", ARRIVAL_HEAVY);
    let (view, cx) = arrival_window(cx);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out)
        .unwrap();
    let timed = |cx: &mut gpui::VisualTestContext, ms: u64| {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(ms));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            let started = std::time::Instant::now();
            window.draw(cx).clear(cx);
            started.elapsed().as_secs_f64() * 1000.0
        })
    };
    for (name, module) in [("light", "probe"), ("heavy", "heavy")] {
        for round in 0..11 {
            for reduced in if round % 2 == 0 {
                [true, false]
            } else {
                [false, true]
            } {
                arrival_set_reduce_motion(cx, reduced);
                cx.update(|window, cx| {
                    view.update(cx, |this, cx| {
                        this.open_overlay(
                            "cost".into(),
                            json!({ "kind": "dialog", "mod": module, "width": 300, "height": 400 }),
                            window,
                            cx,
                        )
                    })
                });
                let first = timed(cx, 0);
                let rest: f64 = (0..9).map(|_| timed(cx, 16)).sum();
                writeln!(file, "{}", json!({ "mod": name, "reduced": reduced, "round": round, "first_ms": first, "after_ms": rest })).unwrap();
                arrival_close(&view, cx, "cost");
                arrival_frame(cx, 400);
            }
        }
    }
}
