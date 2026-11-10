//! Yuzora's native motion (docs/beta2/motion.md): a mod's `transition` tweens in what is painted, it costs nothing
//! once it has arrived, and it is skipped when the system asks for less motion. What is observed is where the last
//! frame painted a background (`painted_quads`), not the description, which only names the target.
use super::*;
use gpui::{Entity, VisualTestContext};
use gpui_shell::ShellRoot;
use std::ops::Deref;
use std::sync::{Mutex, MutexGuard};

// The zoom is one value for the whole process (`gpui_shell::set_pixel_scale`), so a test that reads exact
// pixels owns it while its rig lives: the other tests of this file wait, and the zoom is 1 again when it is gone.
static ZOOM: Mutex<()> = Mutex::new(());

/// What a mod's view is mounted with: the props and `layout()` it is given, the shared values and queued commands
/// it starts with, the window's size.
struct Opts {
    props: Value,
    layout: Value,
    shared: Vec<(&'static str, Value)>,
    commands: Vec<&'static str>,
    perf: Value,
    size: (f32, f32),
    zoom: f32,
}

impl Opts {
    fn new(size: (f32, f32)) -> Self {
        Self { props: Value::Null, layout: Value::Null, shared: vec![], commands: vec![], perf: Value::Null, size, zoom: 1.0 }
    }
}

/// A mod mounted in a window that draws on demand, on a clock the test moves.
struct Rig {
    visual: VisualTestContext,
    runtime: Rc<ShellRuntime>,
    root: Entity<ShellRoot>,
    layout: Rc<RefCell<Value>>,
    perf: Rc<RefCell<Value>>,
    calls: Rc<RefCell<Calls>>,
    settings: Rc<RefCell<Value>>,
    _zoom: MutexGuard<'static, ()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        gpui_shell::set_pixel_scale(1.0);
    }
}

/// `yuzora:perf` with a state the test sets: the real one samples processes on a thread of its own.
fn perf_stub(state: Rc<RefCell<Value>>) -> HostModule {
    let read = state.clone();
    HostModule::new(yuzora_perf::NAME)
        .declarations(yuzora_perf::DECLARATIONS)
        .function("version", |_| Ok("test".into()))
        .function("state", move |_| Ok(to_host(&read.borrow())))
        .async_function("tick", move |_| {
            let now = state.borrow().clone();
            Ok(async move { Ok(to_host(&now)) })
        })
}

/// The colors of the theme Yuzora ships, which the mods are drawn for.
fn shipped_theme() -> Value {
    serde_json::from_str::<Value>(include_str!("../../../defaults/themes/yuzora-dark.json")).unwrap()["colors"].clone()
}

fn theme_color(name: &str) -> gpui::Background {
    let hex = shipped_theme()[name].as_str().unwrap_or_else(|| panic!("the theme has no {name}")).to_owned();
    gpui::rgb(u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap()).into()
}

/// The `main.js` of a copy of a shipped mod.
fn shipped(name: &str) -> String {
    format!("export {{ default }} from './mods/{name}/main.js';")
}

impl Rig {
    fn mount(cx: &mut TestAppContext, staged: &Staged, main: &str, o: Opts) -> Self {
        let zoom = ZOOM.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        gpui_shell::set_pixel_scale(o.zoom);
        let calls = Rc::new(RefCell::new(Calls::default()));
        // "When Yuzora opens" is applied once per run by the first dock: as if that was done, so what the person
        // set never reaches these copies.
        let this_run = from_host(&yuzora_core::host_module().call("session", &gpui_shell::HostArguments::new(Vec::<HostValue>::new())).unwrap());
        calls.borrow_mut().shared.insert("yuzora.startup.applied".into(), this_run);
        for (key, value) in &o.shared {
            calls.borrow_mut().shared.insert((*key).into(), value.clone());
        }
        calls.borrow_mut().commands = o.commands.iter().map(|c| (*c).into()).collect();
        let mut values = fixture(false, o.props, updates("current", "current"));
        values.insert("theme".into(), shipped_theme());
        if !o.layout.is_null() {
            values.insert("layout".into(), o.layout);
        }
        let layout = Rc::new(RefCell::new(values["layout"].clone()));
        let perf = Rc::new(RefCell::new(o.perf));
        let mut person = values["settings"].clone();
        person["zoom"] = json!(o.zoom);
        let settings = Rc::new(RefCell::new(person));
        let (seen, person) = (layout.clone(), settings.clone());
        let herdr = module(values, calls.clone())
            .function("layout", move |_| Ok(to_host(&seen.borrow())))
            .function("settings", move |_| Ok(to_host(&person.borrow())));
        let mut policy = Policy::new()
            .with_storage_path(staged.0.join("store-motion.json"))
            .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
            .with_host_module(herdr)
            .unwrap();
        let _ = crate::yuzora::with_modules(Policy::new(), "");
        for m in crate::yuzora::host_modules("").into_iter().filter(|m| m.name() != yuzora_perf::NAME) {
            policy = policy.with_host_module(m).unwrap();
        }
        gpui_shell::policy::set_default(policy.with_host_module(perf_stub(perf.clone())).unwrap());
        std::fs::write(staged.0.join("main.js"), main).unwrap();
        let runtime = ShellRuntime::new_isolated().unwrap();
        let mut mounted = None;
        let window = cx.add_window(|window, cx| {
            let root = runtime.try_load(&staged.0, window, cx).unwrap();
            mounted = Some(root.clone());
            Mounted(root)
        });
        let visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(gpui::size(gpui::px(o.size.0), gpui::px(o.size.1)));
        let mut rig = Self { visual, runtime, root: mounted.unwrap(), layout, perf, calls, settings, _zoom: zoom };
        rig.settle();
        rig
    }

    fn draw(&mut self) {
        self.visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.draw();
            self.visual.run_until_parked();
        }
    }

    /// The clock moves on by `ms`, and so does the frame GPUI draws then.
    fn advance(&mut self, ms: u64) {
        self.visual.executor().advance_clock(Duration::from_millis(ms));
        self.visual.run_until_parked();
        self.visual.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
    }

    /// Animation frames the last frame asked for (none: the window is at rest).
    fn pending_frames(&mut self) -> usize {
        self.visual.update(|window, cx| window.simulate_next_frame(cx))
    }

    /// What the app does when the state a mod reads changed: the mod renders again.
    fn refresh(&mut self) {
        self.visual.update(|_, cx| self.runtime.refresh(&self.root, cx).unwrap());
        self.draw();
    }

    /// The person changes the zoom: the pixel lengths scale and the mod, which reads `settings()`, renders again.
    fn set_zoom(&mut self, zoom: f32) {
        gpui_shell::set_pixel_scale(zoom);
        self.settings.borrow_mut()["zoom"] = json!(zoom);
        self.refresh();
    }

    fn set_layout(&mut self, layout: Value) {
        *self.layout.borrow_mut() = layout;
        self.refresh();
    }

    /// A call the fixture does not know would leave the mod half working under the test.
    fn done(&self) {
        let calls = self.calls.borrow();
        assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
    }

    /// The commands the mod ran, as `[name, args]`.
    fn runs(&self) -> Vec<Value> {
        self.calls.borrow().effects.iter().filter(|(name, _)| name == "run").map(|(_, args)| json!(args)).collect()
    }

    fn script_renders(&self) -> u64 {
        self.runtime.read_metrics().script_renders()
    }

    /// Every quad the last frame painted that is not fully transparent, in logical pixels: x, y, width, height.
    fn quads(&mut self) -> Vec<(f32, f32, f32, f32, gpui::Background)> {
        let scale = self.visual.update(|window, _| window.scale_factor());
        self.visual.update(|window, _| {
            window
                .painted_quads()
                .into_iter()
                .filter(|q| !q.background.is_transparent())
                .map(|q| {
                    let b = q.bounds;
                    (b.origin.x.as_f32() / scale, b.origin.y.as_f32() / scale, b.size.width.as_f32() / scale, b.size.height.as_f32() / scale, q.background)
                })
                .collect()
        })
    }

    /// The quads that are `w`×`h`, top to bottom.
    fn sized(&mut self, w: f32, h: f32) -> Vec<(f32, f32, f32, f32, gpui::Background)> {
        let mut found: Vec<_> = self.quads().into_iter().filter(|q| (q.2 - w).abs() < 0.01 && (q.3 - h).abs() < 0.01).collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found
    }
}

fn near(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 1.0, "{what}: {got}, wanted {want}");
}

const SLIDER: &str = r##"
import { View, div } from "gpui-kit";
export default class Slider extends View {
  init(_props, cx) {
    this.top = 0;
    cx.timer.after(1000, (cx) => { this.top = 100; cx.notify(); });
  }
  render() {
    return div().id("slider").size_full().child(
      div().id("mark").absolute().left(0).top(this.top).w(40).h(40).bg("#ff0000").transition("top", { duration: 160, easing: "ease-out" }));
  }
}
"##;

// The engine under every test below: a transition in a mod starts where the element was, passes through the
// eased midpoint and arrives.
#[gpui::test]
fn a_transition_in_a_mod_tweens_what_is_painted(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = Rig::mount(cx, &staged, SLIDER, Opts::new((200.0, 200.0)));
    let top = |rig: &mut Rig| rig.sized(40.0, 40.0)[0].1;
    assert_eq!(top(&mut rig), 0.0, "mounted at its first value");
    rig.advance(1000);
    assert_eq!(top(&mut rig), 0.0, "the target moved: the tween starts where it was");
    rig.advance(80);
    near(top(&mut rig), 87.5, "half way through a 160ms ease-out is 87.5 of 100");
    rig.advance(200);
    assert_eq!(top(&mut rig), 100.0, "arrived");
}

impl Rig {
    fn click(&mut self, x: f32, y: f32) {
        let position = gpui::point(gpui::px(x), gpui::px(y));
        let modifiers = gpui::Modifiers::default();
        self.visual.simulate_event(gpui::MouseMoveEvent { position, modifiers, pressed_button: None });
        self.visual.simulate_event(gpui::MouseDownEvent { position, modifiers, button: gpui::MouseButton::Left, click_count: 1, first_mouse: false });
        self.visual.simulate_event(gpui::MouseUpEvent { position, modifiers, button: gpui::MouseButton::Left, click_count: 1 });
        self.draw();
    }

    fn hover(&mut self, x: f32, y: f32) {
        let position = gpui::point(gpui::px(x), gpui::px(y));
        self.visual.simulate_event(gpui::MouseMoveEvent { position, modifiers: gpui::Modifiers::default(), pressed_button: None });
        self.draw();
    }
}

fn bar_layout(shown: Option<&str>) -> Value {
    let closed = dock_layout(include_str!("../../../defaults/layouts/main.json"));
    match shown {
        Some(mode) => dock_shows(closed, &format!("yuzora-dock-{mode}")),
        None => closed,
    }
}

fn bar(cx: &mut TestAppContext, staged: &Staged, shown: Option<&str>) -> Rig {
    let mut o = Opts::new((44.0, 300.0));
    o.props = json!({"dock": "bar"});
    o.layout = bar_layout(shown);
    Rig::mount(cx, staged, &shipped("yuzora-modebar"), o)
}

/// The top of the mode bar's selection (a 34 px button painted in the theme's selected color), if one is shown.
fn selection(rig: &mut Rig) -> Option<f32> {
    let selected = theme_color("selected");
    rig.sized(34.0, 34.0).into_iter().find(|q| q.4 == selected).map(|q| q.1)
}

// Switching modes in the right dock: the selection slides to the mode picked (a 34 px button and a 2 px gap
// apart), in the base speed, and the animation is the engine's alone: the script does not run for it.
#[gpui::test]
fn the_mode_bar_selection_slides_to_the_mode_picked(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = bar(cx, &staged, Some("files"));
    let first = selection(&mut rig).expect("the mode shown is selected");
    let renders = rig.script_renders();
    rig.set_layout(bar_layout(Some("git")));
    assert_eq!(rig.script_renders(), renders + 1, "the layout change is one render");
    near(selection(&mut rig).unwrap(), first, "the slide starts at the mode that was shown");
    rig.advance(80);
    near(selection(&mut rig).unwrap(), first + 36.0 * 0.875, "half way through 160 ms of ease-out");
    rig.advance(200);
    near(selection(&mut rig).unwrap(), first + 36.0, "arrived at the next mode");
    assert_eq!(rig.pending_frames(), 0, "at rest it asks for no frames");
    assert_eq!(rig.script_renders(), renders + 1, "no frame of the slide ran the script");
    rig.set_layout(bar_layout(Some("files")));
    rig.advance(80);
    let back = selection(&mut rig).unwrap();
    assert!(back < first + 36.0 - 1.0 && back > first + 1.0, "it slides back too: {back}");
    rig.done();
}

// Opening or closing the panel is not a slide: the selection is there with the panel and gone without it,
// and a mode opened after a close does not start from where the last one was.
#[gpui::test]
fn the_mode_bar_selection_does_not_slide_in_or_out_with_the_panel(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = bar(cx, &staged, Some("files"));
    let first = selection(&mut rig).unwrap();
    rig.set_layout(bar_layout(None));
    assert_eq!(selection(&mut rig), None, "a closed panel selects nothing");
    rig.set_layout(bar_layout(Some("database")));
    assert_eq!(selection(&mut rig), Some(first + 72.0), "it appears at the mode opened");
    rig.advance(80);
    assert_eq!(selection(&mut rig), Some(first + 72.0), "and stays");
    rig.done();
}

#[gpui::test]
fn the_mode_bar_selection_jumps_when_the_system_asks_for_less_motion(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let staged = Staged::new();
    let mut rig = bar(cx, &staged, Some("files"));
    let first = selection(&mut rig).unwrap();
    rig.set_layout(bar_layout(Some("ssh")));
    assert_eq!(selection(&mut rig), Some(first + 108.0), "straight to the mode picked");
    assert_eq!(rig.pending_frames(), 0);
    rig.done();
}

// The selection takes no press of its own: one over it reaches the button, selected or not.
#[gpui::test]
fn the_mode_bar_buttons_take_presses_over_the_selection(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = bar(cx, &staged, Some("files"));
    let (x, top) = {
        let q = rig.sized(34.0, 34.0).into_iter().find(|q| q.4 == theme_color("selected")).unwrap();
        (q.0 + 17.0, q.1)
    };
    rig.click(x, top + 36.0 + 17.0);
    assert_eq!(rig.runs(), select_and_show("git"), "another mode opens");
    rig.click(x, top + 17.0);
    assert_eq!(rig.runs().last(), Some(&json!(["layout.hide", {"id": "panel"}])), "the mode shown closes the panel");
    rig.done();
}

// Every zoom the person can pick: tenths from ZOOM_MIN to ZOOM_MAX (settings.rs).
fn zoom_levels() -> impl Iterator<Item = f32> {
    (5..=30).map(|tenth| tenth as f32 / 10.0)
}

/// A mode bar wide enough for its buttons at any zoom.
fn wide_bar(cx: &mut TestAppContext, staged: &Staged, shown: Option<&str>, zoom: f32) -> Rig {
    let mut o = Opts::new((120.0, 600.0));
    o.props = json!({"dock": "bar"});
    o.layout = bar_layout(shown);
    o.zoom = zoom;
    Rig::mount(cx, staged, &shipped("yuzora-modebar"), o)
}

/// Where button `at` is drawn: the box its hover color fills while the pointer is over it, which is the one thing a
/// button paints of its own. A mode that is shown is not hovered, so another is shown while this reads.
fn button_box(rig: &mut Rig, at: usize, zoom: f32) -> (f32, f32, f32, f32) {
    let other = DOCK_MODES[(at + 1) % DOCK_MODES.len()];
    rig.set_layout(bar_layout(Some(other)));
    rig.advance(300);
    rig.hover(60.0, (6.0 + at as f32 * 36.0 + 17.0) * zoom);
    let hover = theme_color("hover");
    let found: Vec<_> = rig.quads().into_iter().filter(|q| q.4 == hover).collect();
    assert_eq!(found.len(), 1, "the button under the pointer is the one that is hovered (zoom {zoom})");
    (found[0].0, found[0].1, found[0].2, found[0].3)
}

fn selection_box(rig: &mut Rig, mode: &str) -> (f32, f32, f32, f32) {
    rig.set_layout(bar_layout(Some(mode)));
    rig.advance(300);
    let selected = theme_color("selected");
    let found: Vec<_> = rig.quads().into_iter().filter(|q| q.4 == selected).collect();
    assert_eq!(found.len(), 1, "one mode is selected");
    (found[0].0, found[0].1, found[0].2, found[0].3)
}

// The selection is where the button it selects is, to the pixel: buttons and the gap between them are rounded one
// by one by the layout, so the selection cannot be a position worked out once.
#[gpui::test]
fn the_mode_bar_selection_is_exactly_on_its_button_at_every_zoom(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = wide_bar(cx, &staged, Some("files"), 1.0);
    for zoom in zoom_levels() {
        rig.set_zoom(zoom);
        for (at, mode) in DOCK_MODES.iter().enumerate() {
            let button = button_box(&mut rig, at, zoom);
            let selection = selection_box(&mut rig, mode);
            let same = |a: f32, b: f32| (a - b).abs() < 0.01;
            assert!(
                same(button.0, selection.0) && same(button.1, selection.1) && same(button.2, selection.2) && same(button.3, selection.3),
                "zoom {zoom}, {mode}: the button is {button:?}, the selection {selection:?}"
            );
        }
    }
    rig.done();
}

// The badge of the mode shown is painted on its button, so it must be painted after (over) the selection.
#[gpui::test]
fn the_mode_bar_selection_is_painted_under_the_button(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut o = Opts::new((44.0, 300.0));
    o.props = json!({"dock": "bar"});
    o.layout = bar_layout(Some("files"));
    o.shared = vec![("yuzora.badge.files", json!(3))];
    let mut rig = Rig::mount(cx, &staged, &shipped("yuzora-modebar"), o);
    let tokens: Value = serde_json::from_str(include_str!("../../../defaults/yuzora/tokens.json")).unwrap();
    let chip: gpui::Background = gpui::rgb(u32::from_str_radix(tokens["status"]["done"]["color"].as_str().unwrap().trim_start_matches('#'), 16).unwrap()).into();
    let (selected, chips) = rig.visual.update(|window, _| {
        let quads = window.painted_quads();
        let order = |background: gpui::Background| quads.iter().filter(|q| q.background == background).map(|q| q.order).collect::<Vec<_>>();
        (order(theme_color("selected")), order(chip))
    });
    assert_eq!((selected.len(), chips.len()), (1, 1), "the selection and the badge of the mode shown are painted");
    assert!(chips[0] > selected[0], "the selection (order {}) is painted over the badge (order {})", selected[0], chips[0]);
    rig.done();
}

// A new zoom moves every button at once, and so the selection with them: only picking another mode slides it.
#[gpui::test]
fn the_mode_bar_selection_does_not_slide_when_the_zoom_changes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = wide_bar(cx, &staged, Some("ssh"), 1.0);
    let renders = rig.script_renders();
    rig.set_zoom(1.5);
    assert_eq!(rig.script_renders(), renders + 1, "the zoom is one render");
    let now = selection_box(&mut rig, "ssh");
    rig.set_zoom(1.2);
    let at = rig.quads().into_iter().find(|q| q.4 == theme_color("selected")).map(|q| (q.0, q.1, q.2, q.3)).unwrap();
    assert_eq!(rig.pending_frames(), 0, "nothing is left to animate");
    rig.advance(300);
    let rest = rig.quads().into_iter().find(|q| q.4 == theme_color("selected")).map(|q| (q.0, q.1, q.2, q.3)).unwrap();
    assert_eq!(at, rest, "straight to the new zoom (it was {now:?} at 1.5)");
    rig.done();
}

#[gpui::test]
fn the_mode_bar_names_its_motion_in_the_description(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = dock_copy(cx, &staged, "mods/yuzora-modebar", json!({"dock": "bar"}), &bar_layout(Some("git")), Value::Null, &[], &[], None).tree;
    assert!(tree.contains(":transition(top, 160ms, 0ms, ease-out)"), "{tree}");
}

fn perf_state(app: f64, core: f64, tools: f64) -> Value {
    let usage = |mb: f64| json!({"cpu": 1.0, "memory": mb * 1_000_000.0, "count": 1});
    json!({
        "snapshot": {"total": usage(app + core + tools), "app": usage(app), "core": usage(core), "tools": usage(tools), "core_pid": 42, "core_error": null},
        "health": {"attempts": 3, "failures": 0, "empty": 0, "skipped": 0, "last_error": null},
        "cost": null,
    })
}

// The details of the resource chip: a share's bar follows the new sample on a poll, each bar its own category's
// (the rows sort by memory, so a category can change places as its share changes).
#[gpui::test]
fn the_resource_bars_follow_a_new_sample_by_category(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut o = Opts::new((460.0, 700.0));
    o.commands = vec!["toggle"];
    o.perf = perf_state(500.0, 300.0, 200.0);
    let mut rig = Rig::mount(cx, &staged, &shipped("yuzora-perf"), o);
    let track = theme_color("divider");
    let fill = theme_color("dim");
    let bars = |rig: &mut Rig| -> (f32, Vec<f32>) {
        let quads = rig.quads();
        let width = quads.iter().filter(|q| q.3 == 3.0 && q.4 == track).map(|q| q.2).fold(0.0, f32::max);
        let mut fills: Vec<_> = quads.iter().filter(|q| q.3 == 3.0 && q.4 == fill).collect();
        fills.sort_by(|a, b| a.1.total_cmp(&b.1));
        (width, fills.iter().map(|q| q.2).collect())
    };
    let (width, now) = bars(&mut rig);
    assert!(width > 300.0, "the details are open and have bars: {width}");
    let want = |shares: [f32; 3]| shares.map(|s| s * width);
    for (got, want) in now.iter().zip(want([0.5, 0.3, 0.2])) {
        near(*got, want, "a bar is its share of the track");
    }
    *rig.perf.borrow_mut() = perf_state(200.0, 500.0, 300.0);
    rig.advance(2000);
    let renders = rig.script_renders();
    let (_, now) = bars(&mut rig);
    for (got, want) in now.iter().zip(want([0.3, 0.2, 0.5])) {
        near(*got, want, "the sample came in: each bar still shows its category's old share");
    }
    rig.advance(80);
    let (_, now) = bars(&mut rig);
    for (got, want) in now.iter().zip(want([0.3 + 0.2 * 0.875, 0.2 + 0.1 * 0.875, 0.5 - 0.3 * 0.875])) {
        near(*got, want, "half way through 160 ms of ease-out");
    }
    rig.advance(200);
    let (_, now) = bars(&mut rig);
    for (got, want) in now.iter().zip(want([0.5, 0.3, 0.2])) {
        near(*got, want, "arrived");
    }
    assert_eq!(rig.pending_frames(), 0, "at rest it asks for no frames");
    assert_eq!(rig.script_renders(), renders, "no frame of the tween ran the script");
    rig.done();
}

// A new zoom resizes the track, and a bar is its share of it from that frame on: only a new sample moves a bar.
#[gpui::test]
fn the_resource_bars_do_not_slide_when_the_zoom_changes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut o = Opts::new((1300.0, 1400.0));
    o.commands = vec!["toggle"];
    o.perf = perf_state(500.0, 300.0, 200.0);
    let mut rig = Rig::mount(cx, &staged, &shipped("yuzora-perf"), o);
    let fills = |rig: &mut Rig| {
        let fill = theme_color("dim");
        let mut widths: Vec<_> = rig.quads().into_iter().filter(|q| q.4 == fill && q.3 > 2.0 && q.3 < 5.0 * 3.0).collect();
        widths.sort_by(|a, b| a.1.total_cmp(&b.1));
        widths.into_iter().map(|q| q.2).collect::<Vec<_>>()
    };
    let before = fills(&mut rig);
    assert_eq!(before.len(), 3, "the details are open and have three bars");
    rig.set_zoom(1.5);
    let at = fills(&mut rig);
    assert_eq!(rig.pending_frames(), 0, "nothing is left to animate");
    rig.advance(300);
    assert_eq!(at, fills(&mut rig), "straight to the new zoom");
    assert!(at[0] > before[0] * 1.4, "and it is the new zoom: {at:?} from {before:?}");
    rig.done();
}

fn settings_page(cx: &mut TestAppContext, staged: &Staged) -> Rig {
    Rig::mount(cx, staged, &shipped("yuzora-settings-general"), Opts::new((760.0, 900.0)))
}

/// The knob (16 px) and the track (36 px) of the first switch on the page.
fn first_switch(rig: &mut Rig) -> ((f32, f32), (f32, f32)) {
    first_switch_at(rig, 1.0)
}

fn first_switch_at(rig: &mut Rig, zoom: f32) -> ((f32, f32), (f32, f32)) {
    let track = rig.sized(36.0 * zoom, 20.0 * zoom);
    let knob = rig.sized(16.0 * zoom, 16.0 * zoom);
    let (t, k) = (track.first().expect("a switch"), knob.first().expect("its knob"));
    ((t.0, t.1), (k.0, k.1))
}

// A switch flips: the knob slides across its track in the fast speed.
#[gpui::test]
fn a_settings_switch_slides_its_knob(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = settings_page(cx, &staged);
    let (track, knob) = first_switch(&mut rig);
    assert_eq!(knob.0, track.0 + 2.0, "off: the knob is at the start");
    let renders = rig.script_renders();
    rig.click(track.0 + 18.0, track.1 + 10.0);
    assert_eq!(rig.script_renders(), renders + 1, "the press is one render");
    near(first_switch(&mut rig).1.0, track.0 + 2.0, "the knob starts where it was");
    rig.advance(60);
    near(first_switch(&mut rig).1.0, track.0 + 2.0 + 16.0 * 0.875, "half way through 120 ms of ease-out");
    rig.advance(200);
    near(first_switch(&mut rig).1.0, track.0 + 18.0, "on: the knob is at the end");
    assert_eq!(rig.pending_frames(), 0, "at rest it asks for no frames");
    assert_eq!(rig.script_renders(), renders + 1, "no frame of the slide ran the script");
    rig.done();
    let tree = describe(cx, &staged, "mods/yuzora-settings-general", false, props(), updates("current", "current"), None).unwrap();
    assert!(tree.contains(":transition(left, 120ms, 0ms, ease-out)"), "{tree}");
}

// A new zoom moves the knob with the switch; only a press slides it.
#[gpui::test]
fn a_settings_switch_does_not_slide_when_the_zoom_changes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut rig = Rig::mount(cx, &staged, &shipped("yuzora-settings-general"), Opts::new((1200.0, 2400.0)));
    let before = first_switch(&mut rig);
    rig.set_zoom(1.5);
    let at = first_switch_at(&mut rig, 1.5);
    assert_eq!(rig.pending_frames(), 0, "nothing is left to animate");
    rig.advance(300);
    assert_eq!(at, first_switch_at(&mut rig, 1.5), "straight to the new zoom");
    assert!(at.1.0 != before.1.0 || at.0.0 != before.0.0, "and it is the new zoom");
    rig.done();
}

#[gpui::test]
fn a_settings_switch_jumps_when_the_system_asks_for_less_motion(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let staged = Staged::new();
    let mut rig = settings_page(cx, &staged);
    let (track, _) = first_switch(&mut rig);
    rig.click(track.0 + 18.0, track.1 + 10.0);
    assert_eq!(first_switch(&mut rig).1.0, track.0 + 18.0, "straight to on");
    assert_eq!(rig.pending_frames(), 0);
    rig.done();
}

// `loop` does not follow the system's reduce-motion setting, a mod cannot read that setting, and a loop keeps its
// view drawing about 30 times a second. A Yuzora mod animates with `transition` (see docs/beta2/motion.md).
#[test]
fn no_yuzora_mod_loops_a_property() {
    let loops: Vec<&str> = DEFAULT_MODS
        .iter()
        .filter(|(path, bytes)| path.starts_with("yuzora-") && path.ends_with(".js") && String::from_utf8_lossy(bytes).contains(".loop("))
        .map(|(path, _)| *path)
        .collect();
    assert!(loops.is_empty(), "a loop never stops for reduced motion and keeps its view drawing, use transition: {loops:?}");
}
