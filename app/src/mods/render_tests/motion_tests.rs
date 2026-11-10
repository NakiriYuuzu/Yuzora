//! Yuzora's native motion (docs/beta2/motion.md): a mod's `transition` tweens in what is painted, it costs nothing
//! once it has arrived, and it is skipped when the system asks for less motion. What is observed is where the last
//! frame painted a background (`painted_quads`), not the description, which only names the target.
use super::*;
use gpui::{Entity, VisualTestContext};
use gpui_shell::ShellRoot;
use std::ops::Deref;

/// What a mod's view is mounted with: the props and `layout()` it is given, the shared values and queued commands
/// it starts with, the window's size.
struct Opts {
    props: Value,
    layout: Value,
    shared: Vec<(&'static str, Value)>,
    commands: Vec<&'static str>,
    perf: Value,
    size: (f32, f32),
}

impl Opts {
    fn new(size: (f32, f32)) -> Self {
        Self { props: Value::Null, layout: Value::Null, shared: vec![], commands: vec![], perf: Value::Null, size }
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

/// The `main.js` of a copy of a shipped mod.
fn shipped(name: &str) -> String {
    format!("export {{ default }} from './mods/{name}/main.js';")
}

impl Rig {
    fn mount(cx: &mut TestAppContext, staged: &Staged, main: &str, o: Opts) -> Self {
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
        if !o.layout.is_null() {
            values.insert("layout".into(), o.layout);
        }
        let layout = Rc::new(RefCell::new(values["layout"].clone()));
        let perf = Rc::new(RefCell::new(o.perf));
        let seen = layout.clone();
        let herdr = module(values, calls.clone()).function("layout", move |_| Ok(to_host(&seen.borrow())));
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
        let mut rig = Self { visual, runtime, root: mounted.unwrap(), layout, perf, calls };
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

    fn set_layout(&mut self, layout: Value) {
        *self.layout.borrow_mut() = layout;
        self.refresh();
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
