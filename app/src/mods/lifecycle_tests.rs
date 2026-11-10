use super::*;
use gpui::{IntoElement, Render, TestAppContext, VisualTestContext};
use std::ops::Deref;
use gpui_shell::{ShellRoot, ShellRuntime, policy::Policy};
use std::{cell::Cell, rc::Rc, sync::atomic::{AtomicU64, Ordering}, time::Instant};

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = PathBuf::from(format!("/var/tmp/herdr-lifecycle-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.js"), source).unwrap();
        Self(dir)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
struct Empty;
impl Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

struct Mounted(gpui::Entity<ShellRoot>);
impl Render for Mounted {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

#[gpui::test]
fn size_change_reports_initial_and_resized_layout_without_pointer_input(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().id("root").size_full().border(3).p(7)
      .on_size_change((size, cx) => { note(size.width, size.height); cx.notify(); });
  }
}"#);
    let sizes = Rc::new(RefCell::new(Vec::new()));
    let calls = sizes.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push((a.number(0)?, a.number(1)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().len(), 1, "unchanged layout must not repeat the callback");
    visual.simulate_resize(gpui::size(gpui::px(720.), gpui::px(480.)));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    assert_eq!(sizes.borrow().last().copied(), Some((720., 480.)));
    assert_eq!(sizes.borrow().len(), 2);
}

#[gpui::test]
fn external_file_drops_reach_mod_elements(cx: &mut TestAppContext) {
    use gpui::{ExternalPaths, FileDropEvent, InputEvent};
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  render() {
    return div().size_full().on_drop((event, cx) => { note(event.paths); cx.stop_propagation(); });
  }
}"#);
    let paths = Rc::new(RefCell::new(Vec::new()));
    let calls = paths.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(from_host(a.value(0)?));
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    visual.update(|window, cx| {
        let position = gpui::point(gpui::px(20.), gpui::px(20.));
        window.dispatch_event(FileDropEvent::Entered {
            position,
            paths: ExternalPaths([PathBuf::from("/var/tmp/screenshot.png"), PathBuf::from("/var/tmp/a file.jpg")].into_iter().collect()),
        }.to_platform_input(), cx);
        window.draw(cx).clear(cx);
        window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
    });
    visual.run_until_parked();
    assert_eq!(*paths.borrow(), vec![json!(["/var/tmp/screenshot.png", "/var/tmp/a file.jpg"])]);
}

#[gpui::test]
fn mod_run_rejects_unknown_arguments_before_queueing_a_command(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { run } from "herdr";
import { note } from "fixture";
export default class extends View {
  init() {
    for (const [name, args] of [["space.close", {space: "X", confirmed: true}], ["terminal.kill", {id: "X"}]]) {
      try { run(name, args); note("accepted invalid command"); }
      catch (error) { note(String(error)); }
    }
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "test".into(), props: Value::Null }, 0)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let _root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("space.close: unknown argument(s): space; valid arguments: id, confirmed"), "{}", seen[0]);
    assert!(seen[1].contains("terminal.kill: unknown argument(s): id; valid arguments: terminal"), "{}", seen[1]);
}

#[gpui::test]
fn focus_command_lets_a_mod_take_arrow_keys(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { nextCommand } from "herdr";
import { note } from "fixture";
export default class extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") { this.handle.focus(); note(command); }
        cx.notify();
      }
    });
  }
  render() {
    return div().size_full().track_focus(this.handle)
      .on_key_down((e, cx) => { note(e.key); cx.stop_propagation(); });
  }
}"#);
    let notes = Rc::new(RefCell::new(Vec::new()));
    let calls = notes.clone();
    gpui_shell::policy::set_default(Policy::new()
        .with_host_module(module(Instance { name: "focus-test".into(), props: Value::Null }, 42)).unwrap()
        .with_host_module(HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    visual.run_until_parked();
    set_shown_copies([42].into());
    assert!(send_command_to(42, "focus"));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.simulate_keystrokes("down up");
    visual.run_until_parked();
    assert_eq!(*notes.borrow(), vec!["focus", "down", "up"]);
    set_shown_copies(Default::default());
}

#[gpui::test]
fn close_is_once_and_awaits_its_promise(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { note } from "fixture";
export default class extends View {
  close(cx) {
    note("start");
    return Promise.resolve().then(() => { note("done"); cx.notify(); });
  }
  render() { return div(); }
}"#);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = seen.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(a.string(0)?.to_owned());
            Ok(HostValue::Null)
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        let deadline = Instant::now() + Duration::from_millis(250);
        runtime.close(&root, deadline, window, cx).detach();
        runtime.close(&root, deadline, window, cx).detach();
    }).unwrap();
    cx.run_until_parked();
    assert_eq!(*seen.borrow(), ["start", "done"]);
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn close_starts_native_work_before_releasing_the_copy(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { start } from "fixture";
export default class extends View {
  close() { start(); }
  render() { return div(); }
}"#);
    let started = Rc::new(Cell::new(false));
    let call = started.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("start", move |_| {
            call.set(true);
            // Starting native work must not depend on its promise settling.
            Ok(std::future::pending::<gpui_shell::HostResult>())
        })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| runtime.close(&root, Instant::now() + Duration::from_millis(250), window, cx).detach()).unwrap();
    drop(root);
    cx.run_until_parked();
    assert!(started.get());
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

#[gpui::test]
fn final_close_pumps_native_awaits_before_the_window_goes(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait, note } from "fixture";
export default class extends View {
  async close(cx) { await cx.sleep(1); await wait(); note(); await wait(); note(); }
  render() { return div(); }
}"#);
    let completed = Rc::new(Cell::new(0));
    let calls = completed.clone();
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture")
            .async_function("wait", |_| Ok(async {
                std::thread::sleep(Duration::from_millis(10));
                Ok(HostValue::Null)
            }))
            .function("note", move |_| { calls.set(calls.get() + 1); Ok(HostValue::Null) })
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    window.update(cx, |_, window, cx| {
        // Each native await runs on its own thread; on a loaded runner that took up to 205ms for the
        // whole close. The deadline only bounds a hang, and the pump returns as soon as both finish.
        runtime.close_now(&root, Instant::now() + Duration::from_secs(5), window, cx);
        assert_eq!(completed.get(), 2, "both native awaits must finish before window destruction");
        window.remove_window();
    }).unwrap();
}

#[gpui::test]
fn final_close_stops_waiting_at_its_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
import { wait } from "fixture";
export default class extends View {
  async close() { await wait(); }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new().with_host_module(
        HostModule::new("fixture").async_function("wait", |_| Ok(std::future::pending::<gpui_shell::HostResult>()))
    ).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| {
        runtime.close_now(&root, start + Duration::from_millis(15), window, cx);
        window.remove_window();
    }).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
}

#[gpui::test]
fn close_interrupts_a_synchronous_loop_at_the_shared_deadline(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let fixture = Fixture::new(r#"
import { View, div } from "gpui-kit";
export default class extends View {
  close() { while (true) {} }
  render() { return div(); }
}"#);
    gpui_shell::policy::set_default(Policy::new());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|_, _| Empty);
    let root = window.update(cx, |_, window, cx| runtime.try_load(&fixture.0, window, cx).unwrap()).unwrap();
    let start = Instant::now();
    window.update(cx, |_, window, cx| runtime.close(&root, start + Duration::from_millis(15), window, cx).detach()).unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    cx.run_until_parked();
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
}

// yuzora(L2): a diff follows its repo only for a change that may touch its file: another file's leaves it;
// its own (in another case, as macOS's NFD, after `./`), a folder above it (`.` the top, `src/`),
// anything inside it (a submodule), the file it was renamed from (a `\` in its name the watcher
// writes as `/`), attributes, a Windows short name, and a change that can't be told (the index
// or refs git wrote) load it again. A
// commit's diff never follows. The shipped mod with its imports stubbed; `changes` answers in turn.
#[gpui::test]
fn yuzora_diff_loads_again_only_for_changes_that_may_touch_its_file(cx: &mut TestAppContext) {
    let mut source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods/yuzora-git-diff/main.js")).unwrap();
    let (from, to) = (
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, openOverlay, notify } from \"herdr\";\nimport { t, tokens } from \"yuzora:core\";\nimport { open, changes, diffOpen, diffView, diffText, diffPatch, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from \"yuzora:git\";",
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, openOverlay, notify, t, tokens, open, changes, diffOpen, diffView, diffText, diffPatch, diffStage, diffUnstage, diffDiscard, gitSettings } from \"fixture\";",
    );
    assert_eq!(source.matches(from).count(), 1, "the diff mod no longer has: {from}");
    source = source.replacen(from, to, 1);
    // (paths changed, whether the diff loads again)
    let answers = [
        // The first answer only says where the repo is.
        (json!(null), false),
        (json!(["src/b.txt", "src/\u{30C7}.txt.bak", "sr", "src/\u{30C7}", "old/a.txt2", "a.gitattributes", "src/a~b.txt"]), false),
        (json!(["src/\u{30C7}.txt"]), true),
        (json!(["SRC/\u{30C7}.TXT"]), true),
        // As macOS hands it from the disk: デ as テ and ゛ (NFD); git and the pane have it NFC.
        (json!(["src/\u{30C6}\u{3099}.txt"]), true),
        (json!(null), true),
        (json!(["docs/.gitattributes"]), true),
        (json!([".gitattributes"]), true),
        (json!(["src"]), true),
        (json!(["src/\u{30C7}.txt/m"]), true),
        (json!(["old/a.txt"]), true),
        (json!(["SRC~1/X.TXT"]), true),
        // Pathspecs Yuzora's own stage, unstage and discard may name.
        (json!(["."]), true),
        (json!(["src/"]), true),
        (json!(["./src/\u{30C7}.txt"]), true),
    ];
    let reloads = answers.iter().filter(|(_, again)| *again).count();
    for (mode, loads) in [("unstaged", 1 + reloads), ("staged", 1 + reloads), ("commit", 1)] {
        let fixture = Fixture::new(&source);
        let mut queue: std::collections::VecDeque<Value> = answers.iter().enumerate().map(|(i, (paths, _))| json!({ "generation": i + 1, "paths": paths })).collect();
        // Ran out of time: the same generation, nothing changed.
        queue.push_back(json!({ "generation": answers.len(), "paths": [] }));
        let queue = Rc::new(RefCell::new(queue));
        let opened = Rc::new(Cell::new(0));
        let mut module = HostModule::new("fixture");
        for (name, value) in [
            ("theme", json!({"background": "#101010", "foreground": "#e0e0e0", "border": "#303030", "raised": "#202020", "dim": "#808080", "muted": "#a0a0a0", "accent": "#4080ff", "selected": "#304050", "hover": "#282828", "ansi1": "#f08a8a", "ansi2": "#86d17a"})),
            ("props", json!({"cwd": "/r", "on": null, "path": "src/\u{30C7}.txt", "orig": "old\\a.txt", "mode": mode, "rev": "HEAD", "at": 1})),
            ("tokens", json!({"diff": {}})),
            ("open", json!({"id": "repo"})),
            ("diffView", json!({"id": "d1", "rows": [], "hunks": [], "stageable": true, "add": 0, "del": 0, "leftExists": true, "rightExists": true, "identical": true})),
            ("settings", json!({})),
            ("gitSettings", json!({})),
        ] {
            module = module.function(name, move |_| Ok(to_host(&value)));
        }
        for name in ["shared", "setShared", "run", "panes", "focused", "closeOverlay", "openOverlay", "notify", "diffText", "diffPatch", "diffStage", "diffUnstage", "diffDiscard"] {
            module = module.function(name, |_| Ok(HostValue::Null));
        }
        module = module.function("t", |a| Ok(a.string(0)?.into()));
        let count = opened.clone();
        module = module.function("diffOpen", move |_| {
            count.set(count.get() + 1);
            Ok(to_host(&json!({"id": "d1"})))
        });
        let next = queue.clone();
        module = module.async_function("changes", move |_| {
            let answer: std::pin::Pin<Box<dyn std::future::Future<Output = gpui_shell::HostResult> + Send>> = match next.borrow_mut().pop_front() {
                Some(v) => Box::pin(std::future::ready(Ok(to_host(&v)))),
                None => Box::pin(std::future::pending()),
            };
            Ok(answer)
        });
        gpui_shell::policy::set_default(Policy::new().with_host_module(module).unwrap());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        for _ in 0..3 {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
        }
        assert_eq!(opened.get(), loads, "{mode}: diffOpen calls");
        assert_eq!(queue.borrow().len(), if mode == "commit" { answers.len() + 1 } else { 0 }, "{mode}: answers left");
    }
}

// yuzora(v010): the shipped Git diff in a window, its herdr and yuzora:git stubbed. A list's rows
// hold no handlers and report no sizes, so `Probed` lays the first row out once more below the
// list: its cells report their widths (`cell`), its texts their width and what the pointer is
// over (`text` and `over`: the text's first character, and the pointer's distance from where the
// text starts), and a press picks the row it landed on, as the list's own click would, in the
// body's text size (a `real` prop leaves the clicks to the list). A `pane` prop puts the diff in a
// box that wide, so the pointer can leave it.
const DIFF_PROBE: &str = r#"
import { note } from "fixture";
export default class Probed extends GitDiff {
  cell(r, side, ...rest) {
    const el = super.cell(r, side, ...rest);
    return this.probing ? el.id(`probe-cell-${side}`).on_size_change((s) => note("cell", side, s.width)) : el;
  }
  toggleLine(h, side, index, cx) {
    note("pick", side, index);
    super.toggleLine(h, side, index, cx);
  }
  shade(th, left, side) {
    const el = super.shade(th, left, side);
    return this.probing ? el.id(`probe-shade-${side}`).on_size_change((s) => note("shade", side, s.width)) : el;
  }

  segments(segs, text, ...rest) {
    const el = super.segments(segs, text, ...rest);
    return this.probing ? el.id(`probe-text-${text[0]}`).on_size_change((s) => note("text", text[0], s.width)).on_mouse_move((e) => note("over", text[0], e.local_position.x)) : el;
  }
  render(cx) {
    let root = props().real ? super.render(cx) : super.render(cx).on_mouse_down("left", (e, cx) => { if (e.position.y >= 28) this.clickRow(Math.floor((e.position.y - 28) / 20), cx); });
    const v = this.view, pane = props().pane;
    if (v && v.rows.length && this.layout !== "unified") {
      this.probing = true;
      const row = this.row(v.rows[0], 0, theme(), tokens().diff ?? {}, 0, this.widths?.());
      this.probing = false;
      root = root.child(div().absolute().top(300).left(0).w_full().text_size(11.5).child(row));
    }
    if (props().real) {
      root = root.child(div().id("t-unified").absolute().top(520).left(0).w(60).h(20).on_click((_e, cx) => { this.layout = "unified"; this.relayout(cx); }));
      root = root.child(div().id("t-other").absolute().top(520).left(70).w(60).h(20).on_click((_e, cx) => { setShared("yuzora.git.diff", { cwd: "/r", on: null, path: "other.ts", mode: "unstaged", at: 99 }); cx.notify(); }));
      root = root.child(div().id("t-reload").absolute().top(520).left(140).w(60).h(20).on_click((_e, cx) => { cx.spawn((cx) => this.load(cx)); }));
    }
    return pane ? div().w(pane).h_full().child(root) : root;
  }
}
"#;

/// What the pane is given: rows and hunks as the Rust side makes them, and the lines of both files.
#[derive(Clone)]
struct Diff {
    rows: Value,
    hunks: Value,
    left: Vec<String>,
    right: Vec<String>,
    stageable: bool,
}

impl Diff {
    /// Rows only: a hunk when any row has one, the lines not needed.
    fn of(rows: Value) -> Self {
        let hunks = usize::from(rows.as_array().is_some_and(|r| r.iter().any(|r| r.get("h").is_some())));
        Diff { rows, hunks: json!(vec![json!({}); hunks]), left: Vec::new(), right: Vec::new(), stageable: true }
    }

    fn files(mut self, left: &[&str], right: &[&str]) -> Self {
        self.left = left.iter().map(|l| l.to_string()).collect();
        self.right = right.iter().map(|l| l.to_string()).collect();
        self
    }
}

struct DiffPane {
    visual: VisualTestContext,
    notes: Rc<RefCell<Vec<Value>>>,
    _fixture: Fixture,
}

type Shared = Rc<RefCell<std::collections::BTreeMap<String, Value>>>;

impl DiffPane {
    /// The diff of `rows`, `pane` wide, with `shared` as herdr's shared state.
    fn mount(cx: &mut TestAppContext, rows: Value, pane: f32, shared: &Shared) -> Self {
        Self::mount_with(cx, rows, pane, pane, shared, json!({}))
    }

    /// The same in a window `wide` wide (more than `pane`: room to leave it), opened with `props` besides.
    fn mount_with(cx: &mut TestAppContext, rows: Value, pane: f32, wide: f32, shared: &Shared, props: Value) -> Self {
        Self::mount_diff(cx, Diff::of(rows), pane, wide, shared, props)
    }

    /// A whole `diff`: besides its rows, the lines of both files that `diffText` hands back.
    fn mount_diff(cx: &mut TestAppContext, diff: Diff, pane: f32, wide: f32, shared: &Shared, props: Value) -> Self {
        use gpui::{px, size};
        let Diff { rows, hunks, left, right, stageable } = diff;
        cx.update(gpui_shell::init);
        let mut source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods/yuzora-git-diff/main.js")).unwrap();
        let class = "export default class GitDiff extends View {";
        assert_eq!(source.matches(class).count(), 1, "the diff mod no longer has: {class}");
        source = source.replacen(class, "class GitDiff extends View {", 1) + DIFF_PROBE;
        let fixture = Fixture::new(&source);
        let notes = Rc::new(RefCell::new(Vec::<Value>::new()));
        let theme = json!({"background": "#101010", "foreground": "#e0e0e0", "border": "#303030", "raised": "#202020", "dim": "#808080", "muted": "#a0a0a0", "accent": "#4080ff", "selected": "#304050", "hover": "#282828", "ansi1": "#f08a8a", "ansi2": "#86d17a"});
        let view = json!({"id": "d1", "rows": rows, "hunks": hunks, "stageable": stageable, "add": 3, "del": 0, "leftExists": true, "rightExists": true});
        let mut herdr = HostModule::new("herdr");
        let mut opened = json!({"cwd": "/r", "on": null, "path": "wide-new.ts", "mode": "unstaged", "at": 1});
        if wide > pane {
            opened["pane"] = json!(pane);
        }
        opened.as_object_mut().unwrap().extend(props.as_object().unwrap().clone());
        for (name, value) in [("theme", theme), ("props", opened), ("settings", json!({}))] {
            herdr = herdr.function(name, move |_| Ok(to_host(&value)));
        }
        for name in ["run", "panes", "focused"] {
            herdr = herdr.function(name, |_| Ok(HostValue::Null));
        }
        let told = notes.clone();
        herdr = herdr.function("notify", move |a| {
            let n = from_host(a.value(0)?);
            told.borrow_mut().push(json!(["toast", n["title"], n["level"]]));
            Ok(HostValue::Null)
        });
        let told = notes.clone();
        herdr = herdr.function("openOverlay", move |a| {
            told.borrow_mut().push(json!(["overlay", from_host(a.value(0)?)]));
            Ok(HostValue::Str("o1".into()))
        });
        let told = notes.clone();
        herdr = herdr.function("closeOverlay", move |a| {
            told.borrow_mut().push(json!(["closed", a.string(0)?]));
            Ok(HostValue::Null)
        });
        let store = shared.clone();
        herdr = herdr.function("shared", move |a| Ok(store.borrow().get(a.string(0)?).map(to_host).unwrap_or(HostValue::Null)));
        let store = shared.clone();
        herdr = herdr.function("setShared", move |a| {
            store.borrow_mut().insert(a.string(0)?.to_owned(), from_host(a.value(1)?));
            Ok(HostValue::Null)
        });
        let mut git = HostModule::new("yuzora:git");
        for (name, value) in [("open", json!({"id": "repo"})), ("diffOpen", json!({"id": "d1"})), ("diffView", view), ("settings", json!({}))] {
            git = git.function(name, move |_| Ok(to_host(&value)));
        }
        for name in ["diffStage", "diffUnstage", "diffDiscard"] {
            git = git.function(name, |_| Ok(HostValue::Null));
        }
        git = git.async_function("changes", |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
        // As the Rust side answers: the lines of one side named by `[side, start, end)` runs, joined by `\n`.
        let told = notes.clone();
        git = git.function("diffText", move |a| {
            let runs = from_host(a.value(1)?);
            told.borrow_mut().push(json!(["text", runs]));
            let mut lines: Vec<&str> = Vec::new();
            for run in runs.as_array().expect("runs are an array") {
                let (side, from, to) = (run[0].as_str().unwrap(), run[1].as_u64().unwrap() as usize, run[2].as_u64().unwrap() as usize);
                lines.extend(if side == "l" { &left } else { &right }[from..to].iter().map(String::as_str));
            }
            Ok(HostValue::Str(lines.join("\n")))
        });
        let told = notes.clone();
        git = git.async_function("diffPatch", move |a| {
            let (hunks, whitespace) = (from_host(a.value(1)?), a.string(2)?.to_owned());
            told.borrow_mut().push(json!(["patch", hunks, whitespace]));
            Ok(std::future::ready(Ok::<_, gpui_shell::HostError>(HostValue::Str(format!("patch of {hunks} {whitespace}")))))
        });
        let core = HostModule::new("yuzora:core")
            .function("t", |a| Ok(a.string(0)?.into()))
            .function("tokens", |_| Ok(to_host(&json!({"diff": {"added": "#86d17a", "removed": "#f08a8a", "added_bg": "#103010", "removed_bg": "#301010"}}))));
        let calls = notes.clone();
        let fixture_module = HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(Value::Array((0..4).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(HostValue::Null)
        });
        // As the app grants every mod: the clipboard is the test window's own.
        let mut policy = Policy::new().with_capabilities(Capabilities::new().clipboard_write(true));
        for module in [herdr, git, core, fixture_module] {
            policy = policy.with_host_module(module).unwrap();
        }
        gpui_shell::policy::set_default(policy);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(size(px(wide), px(600.)));
        let mut view = Self { visual, notes, _fixture: fixture };
        view.settle();
        view
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.visual.update(|window, cx| window.draw(cx).clear(cx));
            self.visual.run_until_parked();
        }
    }

    /// The probe row's cell on `side` ("l" or "r"), as wide as it was last laid out.
    fn width(&self, side: &str) -> f64 {
        self.notes.borrow().iter().rev().find(|n| n[0] == "cell" && n[1] == side).and_then(|n| n[2].as_f64()).unwrap_or_else(|| panic!("no {side} cell reported: {:?}", self.notes.borrow()))
    }

    /// What the pointer was over (the text's first character) and how far from where that text starts, in order.
    fn over(&self) -> Vec<(String, f64)> {
        self.notes.borrow().iter().filter(|n| n[0] == "over").map(|n| (n[1].as_str().unwrap_or("").to_owned(), n[2].as_f64().unwrap_or(f64::NAN))).collect()
    }

    /// How wide the probe row's text starting with `first` is, as it is laid out.
    fn text_width(&self, first: &str) -> f64 {
        self.notes.borrow().iter().rev().find(|n| n[0] == "text" && n[1] == first).and_then(|n| n[2].as_f64()).unwrap_or_else(|| panic!("no {first} text reported"))
    }

    fn at(x: f64, y: f64) -> gpui::Point<gpui::Pixels> {
        gpui::point(gpui::px(x as f32), gpui::px(y as f32))
    }

    fn press(&mut self, x: f64, y: f64, clicks: usize) {
        self.visual.simulate_event(gpui::MouseDownEvent { position: Self::at(x, y), modifiers: gpui::Modifiers::none(), button: gpui::MouseButton::Left, click_count: clicks, first_mouse: false });
        self.settle();
    }

    fn drag(&mut self, x: f64, y: f64) {
        self.visual.simulate_mouse_move(Self::at(x, y), gpui::MouseButton::Left, gpui::Modifiers::none());
        self.settle();
    }

    fn release(&mut self, x: f64, y: f64) {
        self.visual.simulate_event(gpui::MouseUpEvent { position: Self::at(x, y), modifiers: gpui::Modifiers::none(), button: gpui::MouseButton::Left, click_count: 1 });
        self.settle();
    }

    fn hover(&mut self, x: f64, y: f64) {
        self.visual.simulate_mouse_move(Self::at(x, y), None, gpui::Modifiers::none());
        self.settle();
    }

    /// A click, down and up where the list sees it (`shift`: with Shift held).
    fn click(&mut self, x: f64, y: f64, shift: bool) {
        let modifiers = gpui::Modifiers { shift, ..gpui::Modifiers::none() };
        self.visual.simulate_event(gpui::MouseDownEvent { position: Self::at(x, y), modifiers, button: gpui::MouseButton::Left, click_count: 1, first_mouse: false });
        self.visual.simulate_event(gpui::MouseUpEvent { position: Self::at(x, y), modifiers, button: gpui::MouseButton::Left, click_count: 1 });
        self.settle();
    }

    fn right_press(&mut self, x: f64, y: f64) {
        self.visual.simulate_event(gpui::MouseDownEvent { position: Self::at(x, y), modifiers: gpui::Modifiers::none(), button: gpui::MouseButton::Right, click_count: 1, first_mouse: false });
        self.settle();
    }

    fn keys(&mut self, keystrokes: &str) {
        self.visual.simulate_keystrokes(keystrokes);
        self.settle();
    }

    /// The text on the clipboard of this test window (a platform clipboard of its own, not the system's).
    fn clipboard(&self) -> Option<String> {
        self.visual.read_from_clipboard().and_then(|item| item.text())
    }

    fn set_clipboard(&self, text: &str) {
        self.visual.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_owned()));
    }

    /// The notes of one kind, in order.
    fn noted(&self, kind: &str) -> Vec<Value> {
        self.notes.borrow().iter().filter(|n| n[0] == kind).cloned().collect()
    }

    /// A wheel or swipe of `dx`, `dy` pixels with the pointer at `x`, `y`.
    fn wheel(&mut self, x: f64, y: f64, dx: f64, dy: f64) {
        self.hover(x, y);
        self.visual.simulate_event(gpui::ScrollWheelEvent {
            position: Self::at(x, y),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(dx as f32), gpui::px(dy as f32))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        self.settle();
    }

    fn close(mut self) {
        self.visual.update(|window, _| window.remove_window());
    }
}

/// A changed row: `left` characters of `l` on the left, `right` of `r` on the right (none: that side is empty).
fn diff_row(left: Option<(usize, &str)>, right: Option<(usize, &str)>) -> Value {
    let mut r = json!({ "k": "~", "h": 0 });
    if let Some((n, c)) = left { r["l"] = json!(1); r["li"] = json!(0); r["lt"] = json!(c.repeat(n)); }
    if let Some((n, c)) = right { r["r"] = json!(1); r["ri"] = json!(0); r["rt"] = json!(c.repeat(n)); }
    r
}

// yuzora(v010): side by side, the two sides of the Git diff share the width by one ratio (half
// and half), whatever their lines hold: an added file's empty side, a line longer than the pane,
// wide characters. They used to be as wide as their own longest line, so a long line took the
// width from the other side.
#[gpui::test]
fn yuzora_diff_sides_share_the_width_evenly_whatever_the_lines(cx: &mut TestAppContext) {
    // (case, rows)
    let cases = [
        ("short", json!([diff_row(Some((10, "a")), Some((20, "b")))])),
        ("added", json!([diff_row(None, Some((257, "b"))), diff_row(None, Some((5, "b")))])),
        ("deleted", json!([diff_row(Some((257, "a")), None)])),
        ("left long", json!([diff_row(Some((300, "a")), Some((5, "b")))])),
        ("right long", json!([diff_row(Some((5, "a")), Some((300, "b")))])),
        ("wide characters", json!([diff_row(Some((300, "\u{6F22}")), Some((250, "\u{3042}")))])),
        ("an unchanged row whose sides differ", json!([{"k": "=", "l": 2, "r": 2, "t": "a".repeat(10), "rt": "b".repeat(400)}, diff_row(Some((3, "a")), Some((3, "b")))])),
    ];
    let mut failures = Vec::new();
    for pane in [1200., 700.] {
        for (case, rows) in &cases {
            let pane_view = DiffPane::mount(cx, rows.clone(), pane, &Shared::default());
            let (left, right) = (pane_view.width("l"), pane_view.width("r"));
            if (left - right).abs() > 1. {
                failures.push(format!("{case} in {pane}px: left {left}, right {right}: not half and half"));
            }
            if (left + 1. + right - pane as f64).abs() > 1. {
                failures.push(format!("{case} in {pane}px: left {left}, right {right} and the 1px divider don't fill the {pane}px"));
            }
            pane_view.close();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a press on a side's line numbers picks that side's line, where the row lays it
// out, at half and at a share kept; the text beside them only makes the hunk current.
#[gpui::test]
fn yuzora_diff_a_press_on_a_sides_numbers_picks_that_sides_line(cx: &mut TestAppContext) {
    const GUTTER: f64 = 44.;
    // (case, rows, the sides that have a line, the share kept)
    let cases = [
        ("both sides", json!([diff_row(Some((10, "a")), Some((20, "b")))]), "lr", None),
        ("added", json!([diff_row(None, Some((257, "b")))]), "r", None),
        ("long lines", json!([diff_row(Some((300, "a")), Some((300, "b")))]), "lr", None),
        ("a share kept", json!([diff_row(Some((10, "a")), Some((20, "b")))]), "lr", Some(0.3)),
        ("a share kept, long lines", json!([diff_row(Some((300, "a")), Some((300, "b")))]), "lr", Some(0.8)),
    ];
    let mut failures = Vec::new();
    for (case, rows, sides, share) in cases {
        let shared = Shared::default();
        if let Some(share) = share {
            shared.borrow_mut().insert(DIFF_SPLIT.into(), json!(share));
        }
        let mut pane = DiffPane::mount(cx, rows, 1200., &shared);
        let left = pane.width("l");
        // Row 0 is 28..48 (under the 28px header). A number's middle, and the text beside it.
        let mut presses = vec![(left + GUTTER / 2. + 1., Some("r")), (left + 1. + GUTTER + 20., None), (GUTTER + 12., None)];
        if sides.contains('l') {
            presses.push((GUTTER / 2. + 1., Some("l")));
        }
        for (x, want) in presses {
            let before = pane.notes.borrow().len();
            pane.press(x, 38., 1);
            let picked: Vec<String> = pane.notes.borrow().iter().skip(before).filter(|n| n[0] == "pick").map(|n| n[1].as_str().unwrap_or("").to_owned()).collect();
            if picked.first().map(String::as_str) != want || picked.len() > 1 {
                failures.push(format!("{case}: a press at x {x} (left {left}) picked {picked:?}, wanted {want:?}"));
            }
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

const DIFF_SPLIT: &str = "yuzora.git.diff.split";

/// Wide lines on both sides: a pane can't show either whole.
fn long_rows() -> Value {
    json!([diff_row(Some((300, "\u{6F22}")), Some((300, "b")))])
}

/// What the diff last kept of its share (null: nothing, so half).
fn kept(shared: &Shared) -> Value {
    shared.borrow().get(DIFF_SPLIT).cloned().unwrap_or(Value::Null)
}

fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() <= 1., "{what}: {a} is not {b}");
}

// yuzora(v010): the divider between the sides drags: the left side's share follows the pointer
// (the sides stay together as wide as the pane), stops at 15% and 85%, and is kept when the
// button is let go, not before. A double press puts half back.
#[gpui::test]
fn yuzora_diff_the_divider_drags_the_share_and_a_double_press_puts_half_back(cx: &mut TestAppContext) {
    const PANE: f64 = 1200.;
    let shared = Shared::default();
    let mut pane = DiffPane::mount(cx, long_rows(), PANE as f32, &shared);
    let half = (PANE - 1.) / 2.;
    near(pane.width("l"), half, "untouched");
    let divider = |pane: &DiffPane| pane.width("l") + 0.5;
    let at = divider(&pane);
    pane.press(at, 100., 1);
    pane.drag(at + 120., 100.);
    near(pane.width("l"), half + 120., "dragged right by 120");
    near(pane.width("l") + 1. + pane.width("r"), PANE, "the sides still fill the pane");
    assert_eq!(kept(&shared), Value::Null, "kept only once let go");
    pane.release(at + 120., 100.);
    assert!((kept(&shared).as_f64().unwrap() - (half + 120.) / (PANE - 1.)).abs() < 0.001, "kept: {:?}", kept(&shared));
    pane.drag(at - 300., 100.);
    near(pane.width("l"), half + 120., "a move after the release changes nothing");

    for (to, share) in [(PANE - 1., 0.85), (0., 0.15)] {
        let from = divider(&pane);
        pane.press(from, 100., 1);
        pane.drag(to, 100.);
        near(pane.width("l"), share * (PANE - 1.), "dragged out of range");
        pane.release(to, 100.);
        assert!((kept(&shared).as_f64().unwrap() - share).abs() < 0.001, "kept: {:?}", kept(&shared));
    }

    let from = divider(&pane);
    pane.press(from, 100., 2);
    near(pane.width("l"), half, "a double press puts half back");
    assert_eq!(kept(&shared), Value::Null, "and keeps nothing");
    pane.close();
}

// yuzora(v010): leaving the pane with the button down ends the drag where it was (its release
// would not be seen out there): the share so far is kept, and coming back in changes nothing.
#[gpui::test]
fn yuzora_diff_the_drag_ends_where_the_pointer_leaves_the_pane(cx: &mut TestAppContext) {
    let shared = Shared::default();
    let mut pane = DiffPane::mount_with(cx, long_rows(), 900., 1200., &shared, json!({}));
    let half = 899. / 2.;
    let at = half + 0.5;
    pane.press(at, 100., 1);
    pane.drag(at + 100., 100.);
    near(pane.width("l"), half + 100., "dragged");
    pane.drag(1100., 100.);
    assert!((kept(&shared).as_f64().unwrap() - (half + 100.) / 899.).abs() < 0.001, "kept what was dragged: {:?}", kept(&shared));
    pane.drag(at - 200., 100.);
    near(pane.width("l"), half + 100., "back in with the button down: not a drag any more");
    pane.close();
}

// yuzora(v010): a diff opens with the share last kept (any pane of it, after it was closed), and
// half where nothing usable is kept.
#[gpui::test]
fn yuzora_diff_opens_with_the_share_last_kept(cx: &mut TestAppContext) {
    const PANE: f64 = 1000.;
    let span = PANE - 1.;
    for (stored, share) in [(json!(0.3), 0.3), (json!(0.85), 0.85), (json!(5), 0.85), (json!(-1), 0.15), (json!("wide"), 0.5), (Value::Null, 0.5), (json!([0.3]), 0.5)] {
        let shared = Shared::default();
        shared.borrow_mut().insert(DIFF_SPLIT.into(), stored.clone());
        let pane = DiffPane::mount(cx, long_rows(), PANE as f32, &shared);
        near(pane.width("l"), share * span, &format!("kept {stored}"));
        near(pane.width("r"), (1. - share) * span, &format!("kept {stored}"));
        pane.close();
    }
    let shared = Shared::default();
    let mut first = DiffPane::mount(cx, long_rows(), PANE as f32, &shared);
    let at = first.width("l") + 0.5;
    first.press(at, 100., 1);
    first.drag(at + 100., 100.);
    first.release(at + 100., 100.);
    first.close();
    let second = DiffPane::mount(cx, long_rows(), PANE as f32, &shared);
    near(second.width("l"), span / 2. + 100., "another diff opened after the first was closed");
    second.close();
}

// yuzora(v010): a long line stays on its side: the pointer is over a side's text only where that
// side's text area is (a line is clipped there, however wide the characters), whether the lines
// are scrolled or not: not over the other side, and, scrolled, not under its own line numbers.
#[gpui::test]
fn yuzora_diff_a_long_line_stays_on_its_side(cx: &mut TestAppContext) {
    let mut pane = DiffPane::mount(cx, long_rows(), 1200., &Shared::default());
    let half = 599.5;
    for scrolled in [false, true] {
        if scrolled {
            pane.wheel(100., 100., -400., 0.);
        }
        let before = pane.over().len();
        pane.hover(half + 100., 310.);
        let right: Vec<String> = pane.over().split_off(before).into_iter().map(|(t, _)| t).collect();
        assert!(!right.is_empty() && right.iter().all(|t| t == "b"), "scrolled={scrolled}: over the right side only its text is hit: {right:?}");
        let before = pane.over().len();
        pane.hover(100., 310.);
        let left: Vec<String> = pane.over().split_off(before).into_iter().map(|(t, _)| t).collect();
        assert!(left.contains(&"\u{6F22}".to_string()) && !left.contains(&"b".to_string()), "scrolled={scrolled}: over the left side only its text is hit: {left:?}");
        let before = pane.over().len();
        pane.hover(20., 310.);
        assert_eq!(pane.over().len(), before, "scrolled={scrolled}: no text under the line numbers");
    }
    pane.close();
}

/// Where the first row's left text starts, from where the pointer is over it (its first character must be "a").
fn left_text_start(pane: &mut DiffPane) -> f64 {
    let before = pane.over().len();
    pane.hover(100., 310.);
    let (first, from) = pane.over().split_off(before).pop().expect("over the left text");
    assert_eq!(first, "a");
    100. - from
}

// yuzora(v010): lines wider than their side scroll sideways (a wheel or swipe), both sides together,
// as far as the widest line ends and no further; a line that fits does not, and a vertical wheel (or
// swipe, with a little sideways in it) is not a sideways one.
#[gpui::test]
fn yuzora_diff_long_lines_scroll_sideways_to_their_end(cx: &mut TestAppContext) {
    const GUTTER: f64 = 44.;
    let rows = |left: usize| json!([diff_row(Some((left, "a")), Some((5, "b")))]);
    let start = left_text_start;
    let mut pane = DiffPane::mount(cx, rows(300), 1200., &Shared::default());
    near(start(&mut pane), GUTTER + 6., "not scrolled: after the numbers and the indent");
    pane.wheel(100., 100., -200., 0.);
    near(start(&mut pane), GUTTER + 6. - 200., "a sideways wheel moves the lines");
    pane.wheel(100., 100., 0., -50.);
    near(start(&mut pane), GUTTER + 6. - 200., "a vertical one does not");
    pane.wheel(100., 100., -10., -80.);
    near(start(&mut pane), GUTTER + 6. - 200., "nor does a vertical one with a little sideways in it");
    pane.wheel(100., 100., 60., -20.);
    near(start(&mut pane), GUTTER + 6. - 140., "a sideways one with a little up or down in it does");
    pane.wheel(100., 100., -100000., 0.);
    let end = start(&mut pane) + pane.text_width("a");
    assert!((599.5 - 30. ..=599.5).contains(&end), "scrolled as far as it goes, the widest line ends at {end}, its cell at 599.5");
    // The left side widens: it needs less scrolling, and the lines come back to end where it does.
    pane.wheel(100., 100., -100000., 0.);
    pane.press(600., 100., 1);
    pane.drag(900., 100.);
    pane.release(900., 100.);
    let end = start(&mut pane) + pane.text_width("a");
    let cell = pane.width("l");
    assert!(end <= cell && end >= cell - 30., "after the divider moved the widest line ends at {end}, its cell at {cell}");
    pane.wheel(100., 100., 100000., 0.);
    near(start(&mut pane), GUTTER + 6., "and back");
    pane.close();

    let mut fits = DiffPane::mount(cx, rows(20), 1200., &Shared::default());
    fits.wheel(100., 100., -200., 0.);
    near(start(&mut fits), GUTTER + 6., "lines that fit do not scroll");
    fits.close();
}

// yuzora(v010): the bar under the lines drags them: its thumb's travel stands for the lines' (the lines
// go farther than the thumb), to the end and back, and lines that fit have no bar to grab.
#[gpui::test]
fn yuzora_diff_the_bar_under_the_lines_drags_them(cx: &mut TestAppContext) {
    let rows = |left: usize| json!([diff_row(Some((left, "a")), Some((5, "b")))]);
    // The 8px bar at the bottom of the 600px window, its thumb in the middle of that. At the left end the
    // thumb covers 527 of the 1200px, and is as long as the lines are shown of what they need.
    let bar = 596.;
    let mut pane = DiffPane::mount(cx, rows(300), 1200., &Shared::default());
    let home = left_text_start(&mut pane);
    pane.wheel(100., 100., -100000., 0.);
    let farthest = home - left_text_start(&mut pane);
    pane.wheel(100., 100., 100000., 0.);
    near(left_text_start(&mut pane), home, "home");

    pane.press(100., bar, 1);
    pane.drag(200., bar);
    pane.release(200., bar);
    let moved = home - left_text_start(&mut pane);
    assert!(moved > 100. && moved < farthest, "the thumb moved 100px and the lines {moved}, of the {farthest} they can");
    // The thumb is at 100..627 now.
    pane.press(300., bar, 1);
    pane.drag(1199., bar);
    pane.release(1199., bar);
    near(home - left_text_start(&mut pane), farthest, "dragged to the end");
    // And at 673..1200.
    pane.press(1000., bar, 1);
    pane.drag(0., bar);
    pane.release(0., bar);
    near(left_text_start(&mut pane), home, "dragged back");
    pane.close();

    let mut fits = DiffPane::mount(cx, rows(20), 1200., &Shared::default());
    fits.press(100., bar, 1);
    fits.drag(900., bar);
    fits.release(900., bar);
    near(left_text_start(&mut fits), home, "lines that fit have no bar");
    fits.close();
}

// yuzora(v010): unified has no divider to grab and its numbers pick lines where they did.
#[gpui::test]
fn yuzora_diff_unified_has_no_divider_and_picks_by_its_numbers(cx: &mut TestAppContext) {
    let rows = json!([{"k": "-", "h": 0, "l": 1, "li": 0, "t": "old"}, {"k": "+", "h": 0, "r": 1, "ri": 0, "t": "new"}]);
    let shared = Shared::default();
    let mut pane = DiffPane::mount_with(cx, rows, 1200., 1200., &shared, json!({"layout": "unified"}));
    for (y, want) in [(38., ("l", 0)), (58., ("r", 0))] {
        let before = pane.notes.borrow().len();
        pane.press(10., y, 1);
        let picked: Vec<(String, i64)> = pane.notes.borrow().iter().skip(before).filter(|n| n[0] == "pick").map(|n| (n[1].as_str().unwrap().to_owned(), n[2].as_i64().unwrap())).collect();
        assert_eq!(picked, vec![(want.0.to_owned(), want.1)], "a press on the numbers at y {y}");
    }
    pane.press(600., 100., 1);
    pane.drag(700., 100.);
    pane.release(700., 100.);
    pane.press(600., 100., 2);
    assert!(shared.borrow().is_empty(), "nothing to drag or reset: {:?}", shared.borrow());
    pane.close();
}

// yuzora(v010): copying from the Git diff. The pane is given the lines of both files by `diffText`,
// as the Rust side answers, so what is on the clipboard is the file's own text: its tabs, its
// trailing spaces, its wide characters, and none of the numbers or +/- the pane draws. A click
// goes through the list as a person's does (`real`).
const COPY_HEADER: f64 = 28.;
const ROW_H: f64 = 20.;

/// The middle of row `i`.
fn row_y(i: usize) -> f64 {
    COPY_HEADER + ROW_H * i as f64 + ROW_H / 2.
}

const TEXT_LEFT: f64 = 100.;
/// In a 1200px pane the right side's text starts at 599.5 + 1 + 44 + 6.
const TEXT_RIGHT: f64 = 680.;

fn copy_pane(cx: &mut TestAppContext, diff: Diff, props: Value) -> DiffPane {
    let mut props = props.as_object().cloned().unwrap_or_default();
    props.insert("real".into(), json!(true));
    DiffPane::mount_diff(cx, diff, 1200., 1200., &Shared::default(), Value::Object(props))
}

/// Side by side: an unchanged line, a changed one (tab, then wide characters), a deleted one with
/// trailing spaces, three unchanged ones folded, and the last line.
fn side_diff() -> Diff {
    let rows = json!([
        {"k": "=", "l": 1, "r": 1, "t": "fn main() {"},
        {"k": "~", "h": 0, "l": 2, "li": 1, "lt": "    let a = 1;", "r": 2, "ri": 1, "rt": "    let a = 10; // \u{6F22}\u{5B57}"},
        {"k": "~", "h": 0, "l": 3, "li": 2, "lt": "    let b = 2;  "},
        {"k": "fold", "n": 3, "l": 4, "r": 3},
        {"k": "=", "l": 7, "r": 6, "t": "}"},
    ]);
    let mut diff = Diff::of(rows).files(
        &["fn main() {", "\tlet a = 1;", "    let b = 2;  ", "// 1", "// 2", "// 3", "}"],
        &["fn main() {", "\tlet a = 10; // \u{6F22}\u{5B57}", "// 1", "// 2", "// 3", "}"],
    );
    diff.hunks = json!([{"index": 0, "a": [1, 3], "b": [1, 2]}]);
    diff
}

/// Unified: an unchanged line, two deleted, one added, three unchanged ones folded, the last line.
fn unified_diff() -> Diff {
    let rows = json!([
        {"k": "=", "l": 1, "r": 1, "t": "keep"},
        {"k": "-", "h": 0, "l": 2, "li": 1, "t": "    old one"},
        {"k": "-", "h": 0, "l": 3, "li": 2, "t": "old two"},
        {"k": "+", "h": 0, "r": 2, "ri": 1, "t": "new one"},
        {"k": "fold", "n": 3, "l": 4, "r": 3},
        {"k": "=", "l": 7, "r": 6, "t": "tail"},
    ]);
    let mut diff = Diff::of(rows).files(
        &["keep", "\told one", "old two", "f1", "f2", "f3", "tail"],
        &["keep", "new one", "f1", "f2", "f3", "tail"],
    );
    diff.hunks = json!([{"index": 0, "a": [1, 3], "b": [1, 2]}]);
    diff
}

#[gpui::test]
fn yuzora_diff_cmd_c_copies_the_lines_selected_as_the_file_has_them(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    assert_eq!(pane.clipboard(), None, "a test window's clipboard starts empty");
    pane.click(TEXT_LEFT, row_y(1), false);
    pane.click(TEXT_LEFT, row_y(2), true);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 1;\n    let b = 2;  "), "the left side's lines 2 and 3: tab and trailing spaces kept, no numbers");
    assert_eq!(pane.noted("toast").len(), 1, "a toast says so: {:?}", pane.noted("toast"));
    pane.keys("ctrl-c");
    assert_eq!(pane.noted("toast").len(), 2, "Ctrl+C copies too (Windows and Linux)");
    pane.close();
}

// yuzora(v010): a selection belongs to the side it began on: the other side's lines are not
// copied, a row that has no line on that side adds none, and a fold adds nothing.
#[gpui::test]
fn yuzora_diff_a_selection_copies_only_the_side_it_began_on(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    pane.click(TEXT_RIGHT, row_y(1), false);
    pane.click(TEXT_LEFT, row_y(4), true);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 10; // \u{6F22}\u{5B57}\n}"), "the right side from row 2 to the last: the deleted row has no right line, the folded lines are left out");
    assert_eq!(pane.noted("text").last().unwrap()[1], json!([["r", 1, 2], ["r", 5, 6]]));
    pane.click(TEXT_LEFT, row_y(0), false);
    pane.click(TEXT_RIGHT, row_y(2), true);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("fn main() {\n\tlet a = 1;\n    let b = 2;  "), "begun on the left: the left side down to the row shift-clicked on the right");
    pane.close();
}

#[gpui::test]
fn yuzora_diff_unified_copies_the_lines_as_they_are_shown(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, unified_diff(), json!({"layout": "unified"}));
    pane.click(300., row_y(0), false);
    pane.click(300., row_y(5), true);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("keep\n\told one\nold two\nnew one\ntail"), "deleted lines, then added ones, from their own side; the fold left out");
    pane.close();
}

// yuzora(v010): a selection is where the rows are, not where the list is scrolled: scrolled away
// from (the lines are not on screen) and back, it copies the same lines.
#[gpui::test]
fn yuzora_diff_a_selection_stays_when_scrolled_away_and_back(cx: &mut TestAppContext) {
    let rows: Vec<Value> = (0..200).map(|i| json!({"k": "=", "l": i + 1, "r": i + 1, "t": format!("line {i}")})).collect();
    let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut pane = copy_pane(cx, Diff::of(json!(rows)).files(&refs, &refs), json!({}));
    pane.click(TEXT_LEFT, row_y(3), false);
    pane.click(TEXT_LEFT, row_y(5), true);
    pane.wheel(300., 300., 0., -2500.);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("line 3\nline 4\nline 5"), "copied from far below, lines that are not on screen included");
    pane.wheel(300., 300., 0., 2500.);
    pane.keys("cmd-c");
    assert_eq!(pane.noted("toast").len(), 2);
    pane.close();
}

// yuzora(v010): the selected rows are marked on the side they were selected on (the probe row is
// row 1: a mark appears when it enters the selection and goes when the selection moves away).
#[gpui::test]
fn yuzora_diff_the_selected_rows_are_marked_on_their_side(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    let marks = |pane: &DiffPane| pane.noted("shade").len();
    assert_eq!(marks(&pane), 0, "nothing selected, nothing marked");
    pane.click(TEXT_LEFT, row_y(0), false);
    assert_eq!(marks(&pane), 1, "row 1 selected on the left");
    pane.click(TEXT_RIGHT, row_y(0), false);
    assert_eq!(marks(&pane), 2, "the right side is marked, the left one is not (its mark went away)");
    pane.click(TEXT_LEFT, row_y(0), false);
    assert_eq!(marks(&pane), 3, "and the left one comes back as a new mark");
    pane.click(TEXT_LEFT, row_y(2), false);
    pane.click(TEXT_LEFT, row_y(0), false);
    assert_eq!(marks(&pane), 4, "a mark on row 1 appeared once more only after the selection left it");
    pane.close();
}

#[gpui::test]
fn yuzora_diff_cmd_c_with_nothing_selected_copies_nothing(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard(), None, "the key does nothing here");
    assert!(pane.noted("toast").is_empty(), "and says nothing");
    pane.click(TEXT_LEFT, row_y(1), false);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 1;"), "with a row selected it copies");
    pane.close();
}

// yuzora(v010): a press on the divider drags the divider and selects nothing; a press on the
// numbers of a diff that can stage picks the line (nothing selected); where nothing can be
// staged the numbers select the row.
#[gpui::test]
fn yuzora_diff_the_divider_and_the_pick_numbers_do_not_select(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    let divider = 599.5 + 0.5;
    pane.press(divider, row_y(1), 1);
    pane.drag(divider + 40., row_y(1));
    pane.release(divider + 40., row_y(1));
    pane.click(10., row_y(1), false);
    assert_eq!(pane.noted("pick").len(), 1, "the numbers pick the line");
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard(), None, "the drag and the pick selected nothing");
    pane.close();
    let mut diff = side_diff();
    diff.stageable = false;
    let mut pane = copy_pane(cx, diff, json!({}));
    pane.click(10., row_y(1), false);
    assert!(pane.noted("pick").is_empty(), "nothing to pick in a diff that cannot stage");
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 1;"), "the numbers select the row there");
    pane.close();
}

// yuzora(v010): a selection names rows of the diff as it was: another layout and another file
// leave none, and neither does a diff read again. (`t-unified`, `t-other` and `t-reload` are the
// probe's own controls: what the layout pill does, another file asked for through the shared
// state, and the reload a change of the file brings.)
#[gpui::test]
fn yuzora_diff_the_selection_goes_with_the_rows_it_named(cx: &mut TestAppContext) {
    for (control, what) in [(10., "another layout: the rows are others"), (100., "another file"), (170., "a diff read again")] {
        let mut pane = DiffPane::mount_diff(cx, side_diff(), 1200., 1200., &Shared::default(), json!({"real": true}));
        pane.click(TEXT_LEFT, row_y(1), false);
        pane.keys("cmd-c");
        assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 1;"), "{what}: the selection copies until it goes");
        pane.set_clipboard("untouched");
        pane.click(control, 530., false);
        pane.keys("cmd-c");
        assert_eq!(pane.clipboard().as_deref(), Some("untouched"), "{what}: nothing is selected now");
        pane.close();
    }
}

/// The menu a right-click on `(x, row)` asked for, as a popup of the diff opened with what it was given.
fn menu_pane(cx: &mut TestAppContext, diff: Diff, press: &[(f64, usize, bool)]) -> (DiffPane, DiffPane) {
    let mut pane = copy_pane(cx, diff.clone(), json!({}));
    for (i, (x, row, shift)) in press.iter().enumerate() {
        if i + 1 == press.len() {
            pane.right_press(*x, row_y(*row));
        } else {
            pane.click(*x, row_y(*row), *shift);
        }
    }
    let spec = pane.noted("overlay").last().unwrap_or_else(|| panic!("no menu opened: {:?}", pane.notes.borrow()))[1].clone();
    assert_eq!((spec["kind"].as_str(), spec["at"].as_str(), spec["mod"].as_str()), (Some("popup"), Some("pointer"), Some("yuzora-git-diff")));
    let mut props = spec["props"].as_object().cloned().unwrap();
    props.insert("overlay".into(), json!("o1"));
    props.insert("real".into(), json!(true));
    let menu = DiffPane::mount_diff(cx, diff, 210., 210., &Shared::default(), Value::Object(props));
    (pane, menu)
}

/// Where the menu's rows are: Copy, Copy hunk, Copy as patch.
fn menu_y(item: usize) -> f64 {
    4. + 28. * item as f64 + 14.
}

// yuzora(v010): a right-click on a row selects it (when it is not in the selection already) and
// opens a menu of this mod's own: Copy the selection, Copy the hunk's new lines, Copy the
// selection's hunks as a patch. Each puts its text on the clipboard once, says so, and closes.
#[gpui::test]
fn yuzora_diff_the_menu_copies_the_selection_the_hunk_and_a_patch(cx: &mut TestAppContext) {
    let (pane, mut menu) = menu_pane(cx, side_diff(), &[(TEXT_LEFT, 2, false)]);
    menu.click(100., menu_y(0), false);
    assert_eq!(menu.clipboard().as_deref(), Some("    let b = 2;  "), "Copy: the row right-clicked, as the file has it");
    assert_eq!(menu.noted("closed"), vec![json!(["closed", "o1"])], "and the menu closes");
    menu.click(100., menu_y(1), false);
    assert_eq!(menu.clipboard().as_deref(), Some("\tlet a = 10; // \u{6F22}\u{5B57}"), "Copy hunk: the hunk's lines as they are after the change");
    menu.click(100., menu_y(2), false);
    assert_eq!(menu.noted("patch"), vec![json!(["patch", [0], "none"])], "Copy as patch asks for the hunk the selection is in");
    assert_eq!(menu.clipboard().as_deref(), Some("patch of [0] none"));
    assert_eq!(menu.noted("toast").len(), 3, "one toast each: {:?}", menu.noted("toast"));
    menu.close();
    pane.close();
}

#[gpui::test]
fn yuzora_diff_a_right_click_keeps_the_selection_it_is_in_and_replaces_the_one_it_is_not_in(cx: &mut TestAppContext) {
    let (pane, mut menu) = menu_pane(cx, side_diff(), &[(TEXT_LEFT, 1, false), (TEXT_LEFT, 2, true), (TEXT_LEFT, 2, false)]);
    menu.click(100., menu_y(0), false);
    assert_eq!(menu.clipboard().as_deref(), Some("\tlet a = 1;\n    let b = 2;  "), "inside the selection: all of it");
    pane.close();
    menu.close();

    let (pane, mut menu) = menu_pane(cx, side_diff(), &[(TEXT_LEFT, 1, false), (TEXT_LEFT, 2, true), (TEXT_LEFT, 4, false)]);
    menu.click(100., menu_y(0), false);
    assert_eq!(menu.clipboard().as_deref(), Some("}"), "outside it: that row only");
    let (before, toasts) = (menu.clipboard(), menu.noted("toast").len());
    menu.click(100., menu_y(1), false);
    menu.click(100., menu_y(2), false);
    assert_eq!((menu.clipboard(), menu.noted("toast").len()), (before, toasts), "no hunk on that row: Copy hunk and Copy as patch do nothing");
    assert!(menu.noted("patch").is_empty());
    pane.close();
    menu.close();
}

// yuzora(v010): a selection across hunks copies the patch of each; a fold has no menu.
#[gpui::test]
fn yuzora_diff_the_patch_is_asked_for_every_hunk_the_selection_touches_and_a_fold_has_no_menu(cx: &mut TestAppContext) {
    let rows = json!([
        {"k": "~", "h": 0, "l": 1, "li": 0, "lt": "a", "r": 1, "ri": 0, "rt": "A"},
        {"k": "=", "l": 2, "r": 2, "t": "same"},
        {"k": "~", "h": 1, "l": 3, "li": 2, "lt": "b", "r": 3, "ri": 2, "rt": "B"},
        {"k": "fold", "n": 4, "l": 4, "r": 4},
    ]);
    let mut diff = Diff::of(rows).files(&["a", "same", "b"], &["A", "same", "B"]);
    diff.hunks = json!([{"index": 0, "a": [0, 1], "b": [0, 1]}, {"index": 1, "a": [2, 3], "b": [2, 3]}]);
    let (pane, mut menu) = menu_pane(cx, diff.clone(), &[(TEXT_LEFT, 0, false), (TEXT_LEFT, 2, true), (TEXT_LEFT, 1, false)]);
    menu.click(100., menu_y(2), false);
    assert_eq!(menu.noted("patch"), vec![json!(["patch", [0, 1], "none"])]);
    pane.close();
    menu.close();

    // Copy hunk is the hunk under the pointer, not the first one the selection touches.
    let (pane, mut menu) = menu_pane(cx, diff, &[(TEXT_LEFT, 0, false), (TEXT_LEFT, 2, true), (TEXT_LEFT, 2, false)]);
    menu.click(100., menu_y(1), false);
    assert_eq!(menu.clipboard().as_deref(), Some("B"), "the second hunk's new line");
    pane.close();
    menu.close();

    let mut pane = copy_pane(cx, side_diff(), json!({}));
    pane.right_press(TEXT_LEFT, row_y(3));
    assert!(pane.noted("overlay").is_empty(), "a fold opens no menu");
    pane.close();
}

// yuzora(L2): the Git sidebar's main.js with its herdr, yuzora:core and yuzora:git imports
// from `fixture`; `extra` replaces more of it.
fn git_sidebar_source(extra: &[(&str, &str)]) -> String {
    let mut source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods/yuzora-sidebar-git/main.js")).unwrap();
    let imports = (
        "import { spaces, panes, focused, theme, run, openOverlay, closeOverlay, notify, shared, setShared, props, nextCommand } from \"herdr\";\nimport { t, tokens } from \"yuzora:core\";\nimport { open, status, changed, stage, unstage, discard, commitChecks, commit, messages, push, pushPreview, sequencer, fetch, trust, repos, settings as gitSettings } from \"yuzora:git\";",
        "import { spaces, panes, focused, theme, run, openOverlay, closeOverlay, notify, shared, setShared, props, nextCommand, t, tokens, open, status, changed, stage, unstage, discard, commitChecks, commit, messages, push, pushPreview, sequencer, fetch, trust, repos, gitSettings, note } from \"fixture\";",
    );
    for (from, to) in std::iter::once(imports).chain(extra.iter().copied()) {
        assert_eq!(source.matches(from).count(), 1, "the Git sidebar no longer has: {from}");
        source = source.replacen(from, to, 1);
    }
    source
}

/// What the stubbed Git sidebar was told and did.
#[derive(Default)]
struct GitSidebar {
    focused: RefCell<Value>,
    shared: RefCell<std::collections::BTreeMap<String, Value>>,
    opened: RefCell<Vec<String>>,
    listed: RefCell<Vec<Value>>,
    effects: RefCell<Vec<(String, Value)>>,
    notes: RefCell<Vec<Value>>,
    /// What nextCommand() hands out, in order; with none left it never returns.
    commands: RefCell<std::collections::VecDeque<String>>,
}

/// A space `s1` (folder `/sp`, panes `p1` in `/sp/a` and `p2` in `/sp/b/c`; `p3` is another
/// space's), and yuzora:git: `open` takes any folder but `fails` names (with that error; and
/// `/sp/nested` and `/sp/broken` open as a repo above, `/sp` and `/sp/r`; `/sp/alias` as
/// `/sp/ALIAS`, the same folder spelled another way), `status`
/// has as many changes as the repo's name says (`/sp/a` 1, `/sp/b/c` 2, `/sp/r` 3, `/sp/ALIAS` 4) and fails for
/// `/sp/vanish`, `repos` answers with `found`.
fn git_sidebar_module(state: Rc<GitSidebar>, props: Value, fails: &'static [(&'static str, &'static str)], found: fn(&str) -> Result<Value, String>) -> HostModule {
    let mut module = HostModule::new("fixture");
    for (name, value) in [
        ("theme", json!({"background": "#101010", "foreground": "#e0e0e0", "border": "#303030", "raised": "#202020", "dim": "#808080", "muted": "#a0a0a0", "accent": "#4080ff", "selected": "#304050", "hover": "#282828", "chrome": "#181818", "ansi1": "#f08a8a", "ansi2": "#86d17a"})),
        ("props", props),
        ("spaces", json!([{"id": "s1", "active": true, "folder": "/sp", "machine": null, "tabs": [{"id": "tab1"}]}, {"id": "s2", "active": false, "folder": "/other", "machine": null, "tabs": [{"id": "tab2"}]}])),
        ("panes", json!([{"pane": "p1", "tab": "tab1", "cwd": "/sp/a"}, {"pane": "p2", "tab": "tab1", "cwd": "/sp/b/c"}, {"pane": "p3", "tab": "tab2", "cwd": "/other"}])),
        ("tokens", json!({})),
        ("gitSettings", json!({})),
    ] {
        module = module.function(name, move |_| Ok(to_host(&value)));
    }
    module = module.function("t", |a| Ok(a.string(0)?.into()));
    module = module.function("run", |_| Ok(HostValue::Null));
    let s = state.clone();
    module = module.function("focused", move |_| Ok(to_host(&s.focused.borrow())));
    let s = state.clone();
    module = module.function("shared", move |a| Ok(s.shared.borrow().get(a.string(0)?).map(to_host).unwrap_or(HostValue::Null)));
    let s = state.clone();
    module = module.function("setShared", move |a| {
        let (key, value) = (a.string(0)?.to_owned(), a.get(1).map(from_host).unwrap_or(Value::Null));
        s.effects.borrow_mut().push(("setShared".into(), json!([key, value])));
        s.shared.borrow_mut().insert(key, value);
        Ok(HostValue::Null)
    });
    for name in ["notify", "openOverlay", "closeOverlay"] {
        let s = state.clone();
        module = module.function(name, move |a| {
            s.effects.borrow_mut().push((name.into(), a.get(0).map(from_host).unwrap_or(Value::Null)));
            Ok(HostValue::Str("o9".into()))
        });
    }
    let s = state.clone();
    module = module.function("note", move |a| {
        s.notes.borrow_mut().push(Value::Array((0..3).filter_map(|i| a.get(i).map(from_host)).collect()));
        Ok(HostValue::Null)
    });
    type Answer = std::pin::Pin<Box<dyn std::future::Future<Output = gpui_shell::HostResult> + Send>>;
    let answer = |r: Result<Value, String>| -> Answer { Box::pin(std::future::ready(r.map(|v| to_host(&v)).map_err(HostError::new))) };
    let s = state.clone();
    module = module.async_function("open", move |a| {
        let cwd = a.string(0)?.to_owned();
        s.opened.borrow_mut().push(cwd.clone());
        let top = match cwd.as_str() { "/sp/nested" => "/sp".to_owned(), "/sp/broken" => "/sp/r".to_owned(), "/sp/alias" => "/sp/ALIAS".to_owned(), _ => cwd.clone() };
        Ok(answer(match fails.iter().find(|(root, _)| *root == cwd) {
            Some((_, e)) => Err(e.to_string()),
            None => Ok(json!({"id": top, "root": top, "machine": null, "gitDir": format!("{top}/.git"), "commonDir": format!("{top}/.git")})),
        }))
    });
    module = module.async_function("status", move |a| {
        let root = a.string(0)?.to_owned();
        let n = match root.as_str() { "/sp/a" => 1, "/sp/b/c" => 2, "/sp/r" => 3, "/sp/ALIAS" => 4, _ => 0 };
        let unstaged: Vec<Value> = (0..n).map(|i| json!({"path": format!("f{i}"), "kind": "M", "add": 1, "del": 0, "binary": false})).collect();
        Ok(answer(if root == "/sp/vanish" { Err("fatal: cannot change to '/sp/vanish': No such file or directory".into()) } else { Ok(json!({
            "repo": root, "root": root, "machine": null, "generation": 1,
            "branch": {"head": "main", "oid": "abc", "upstream": null, "ahead": 0, "behind": 0, "detached": false, "initial": false},
            "operation": {"kind": "none", "step": null, "onto": null, "head": null, "interactive": false},
            "staged": [], "unstaged": unstaged, "untracked": [], "conflicts": [],
            "counts": {"staged": 0, "unstaged": n, "untracked": 0, "conflicts": 0, "total": n},
            "stash": 0, "truncated": false, "numstatSkipped": false
        })) }))
    });
    module = module.async_function("messages", move |_| Ok(answer(Ok(json!({"history": [], "template": null, "last": null, "authors": []})))));
    module = module.async_function("changed", |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
    let s = state.clone();
    module = module.async_function("nextCommand", move |_| {
        Ok(match s.commands.borrow_mut().pop_front() {
            Some(c) => answer(Ok(Value::String(c))),
            None => Box::pin(std::future::pending()) as Answer,
        })
    });
    let s = state.clone();
    module = module.async_function("repos", move |a| {
        let args: Vec<Value> = (0..3).map(|i| a.get(i).map(from_host).unwrap_or(Value::Null)).collect();
        let folder = args[0].as_str().unwrap_or_default().to_owned();
        s.listed.borrow_mut().push(Value::Array(args));
        Ok(answer(found(&folder)))
    });
    for name in ["stage", "unstage", "discard", "commitChecks", "commit", "push", "pushPreview", "sequencer", "fetch", "trust"] {
        module = module.async_function(name, move |_| Ok(answer(Err(format!("unexpected {name}")))));
    }
    module
}

/// The sidebar loaded with `module`, drawn and settled.
fn git_sidebar_mount(cx: &mut TestAppContext, source: &str, module: HostModule) -> (Fixture, VisualTestContext) {
    let fixture = Fixture::new(source);
    gpui_shell::policy::set_default(Policy::new().with_host_module(module).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    git_sidebar_settle(&mut visual);
    (fixture, visual)
}

fn git_sidebar_settle(visual: &mut VisualTestContext) {
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
    }
}

/// Past the sidebar's once-a-second look at where it should be.
fn git_sidebar_tick(visual: &mut VisualTestContext) {
    visual.executor().advance_clock(std::time::Duration::from_millis(1100));
    git_sidebar_settle(visual);
}

// yuzora(L2): the Git sidebar's repo, with several in a space: the focused pane's (as before),
// or the one pinned for the active space (`yuzora.git.repo.<space id>` in shared state; another
// space's pin doesn't count); pinned, the focus doesn't move it, and the badge is the pinned
// repo's. Unpinned, it follows the focus again.
#[gpui::test]
fn yuzora_git_repo_follows_the_focus_or_the_pin_of_its_space(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, px};
    let state = Rc::new(GitSidebar::default());
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    let module = git_sidebar_module(state.clone(), Value::Null, &[], |_| Ok(json!({"repos": [], "truncated": false})));
    let (_fixture, mut visual) = git_sidebar_mount(cx, &git_sidebar_source(&[]), module);
    let last = |state: &GitSidebar| (state.opened.borrow().len(), state.opened.borrow().last().cloned().unwrap_or_default(), state.shared.borrow().get("yuzora.badge.git").cloned().unwrap_or(Value::Null));
    assert_eq!(last(&state), (1, "/sp/a".into(), json!(1)), "the focused pane's repo");
    // The picker (a click on the repo in the header) gets the active space's folder and its
    // panes' folders, not another space's.
    let picker = |state: &GitSidebar, visual: &mut VisualTestContext| {
        visual.simulate_click(point(px(120.), px(16.)), Modifiers::none());
        git_sidebar_settle(visual);
        state.effects.borrow().iter().rev().find(|(name, _)| name == "openOverlay").map(|(_, v)| v["props"]["picker"].clone())
    };
    assert_eq!(picker(&state, &mut visual), Some(json!({"space": "s1", "folder": "/sp", "on": null, "cwds": ["/sp/a", "/sp/b/c"], "current": "/sp/a", "pinned": false})));
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p2"});
    git_sidebar_tick(&mut visual);
    assert_eq!(last(&state), (2, "/sp/b/c".into(), json!(2)), "the focus moved");
    state.shared.borrow_mut().insert("yuzora.git.repo.s2".into(), json!({"root": "/other", "on": null}));
    git_sidebar_tick(&mut visual);
    assert_eq!(last(&state).0, 2, "another space's pin");
    state.shared.borrow_mut().insert("yuzora.git.repo.s1".into(), json!({"root": "/sp/r", "on": null}));
    git_sidebar_tick(&mut visual);
    assert_eq!(last(&state), (3, "/sp/r".into(), json!(3)), "pinned");
    assert_eq!(picker(&state, &mut visual).map(|p| (p["current"].clone(), p["pinned"].clone())), Some((json!("/sp/r"), json!(true))));
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    git_sidebar_tick(&mut visual);
    assert_eq!(last(&state).0, 3, "the focus moved a pinned sidebar");
    state.shared.borrow_mut().remove("yuzora.git.repo.s1");
    git_sidebar_tick(&mut visual);
    assert_eq!(last(&state), (4, "/sp/a".into(), json!(1)), "unpinned");
    visual.update(|window, _| window.remove_window());
}

// yuzora(L2): a pinned repo that's gone (no `.git` left in its folder, or no folder: `repos`
// reads the file system, no git runs) is unpinned with a note, and the sidebar follows the focus
// again; found gone when it opens, or later, when status fails, or when git opens it as another
// repo (the one above). One that can't be looked at (a machine away, no permission: `repos`
// fails) or still has its `.git` stays pinned, with the error: git opening the repo above one
// whose `.git` it won't take now doesn't make the sidebar show that repo. One git spells another
// way (a subst drive on Windows, say) is the pinned repo when `repos` gives the same real path.
#[gpui::test]
fn yuzora_git_repo_pinned_and_gone_follows_the_focus_again(cx: &mut TestAppContext) {
    const FAILS: &[(&str, &str)] = &[("/sp/gone", "fatal: not a git repository"), ("/sp/moved", "fatal: not a git repository"), ("/sp/away", "machine lab is not connected"), ("/sp/still", "fatal: bad config line 1")];
    fn found(folder: &str) -> Result<Value, String> {
        match folder {
            "/sp/away" => Err("machine lab is not connected".into()),
            "/sp/still" => Ok(json!({"repos": [{"root": "/sp/still", "name": "still", "rel": ".", "trusted": false}], "truncated": false})),
            "/sp/broken" => Ok(json!({"repos": [{"root": "/sp/broken", "name": "broken", "rel": ".", "trusted": false}], "truncated": false})),
            // The repos git opens instead: each its own top level, not the pinned one.
            "/sp" => Ok(json!({"repos": [{"root": "/sp", "name": "sp", "rel": ".", "trusted": false}], "truncated": false})),
            "/sp/r" => Ok(json!({"repos": [{"root": "/sp/r", "name": "r", "rel": ".", "trusted": false}], "truncated": false})),
            // As git spells it: the same folder.
            "/sp/ALIAS" => Ok(json!({"repos": [{"root": "/sp/alias", "name": "alias", "rel": ".", "trusted": false}], "truncated": false})),
            // Its `.git` gone: the repo it is in now is the one above.
            "/sp/moved" => Ok(json!({"repos": [{"root": "/sp", "name": "sp", "rel": "/sp", "trusted": false}], "truncated": false})),
            _ => Ok(json!({"repos": [], "truncated": false})),
        }
    }
    let mut failures = Vec::new();
    // (pinned, gone, the folders `repos` is asked about, the badge when it stays pinned)
    let cases: [(&str, bool, &[&str], Value); 8] = [
        ("/sp/gone", true, &["/sp/gone"], Value::Null),
        ("/sp/moved", true, &["/sp/moved"], Value::Null),
        ("/sp/vanish", true, &["/sp/vanish"], Value::Null),
        ("/sp/nested", true, &["/sp", "/sp/nested"], Value::Null),
        ("/sp/away", false, &["/sp/away"], Value::Null),
        ("/sp/still", false, &["/sp/still"], Value::Null),
        // Not the repo above's changes (`/sp/r` has 3).
        ("/sp/broken", false, &["/sp/r", "/sp/broken"], Value::Null),
        ("/sp/alias", false, &["/sp/ALIAS"], json!(4)),
    ];
    for (root, gone, asked, want_badge) in cases {
        let state = Rc::new(GitSidebar::default());
        *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
        state.shared.borrow_mut().insert("yuzora.git.repo.s1".into(), json!({"root": root, "on": null}));
        let module = git_sidebar_module(state.clone(), Value::Null, FAILS, found);
        // The error the sidebar shows.
        let source = git_sidebar_source(&[(".child(this.error ?? t(\"loading\"))", ".child((note(\"error\", this.error ?? null), this.error ?? t(\"loading\")))")]);
        let (_fixture, mut visual) = git_sidebar_mount(cx, &source, module);
        git_sidebar_tick(&mut visual);
        let pin = state.shared.borrow().get("yuzora.git.repo.s1").cloned().unwrap_or(Value::Null);
        let notes: Vec<Value> = state.effects.borrow().iter().filter(|(name, _)| name == "notify").map(|(_, v)| v.clone()).collect();
        let opened = state.opened.borrow().clone();
        let listed = state.listed.borrow().clone();
        if listed != asked.iter().map(|f| json!([f, null, []])).collect::<Vec<_>>() {
            failures.push(format!("{root}: asked the file system {listed:?}"));
        }
        let want_opened: Vec<String> = if gone { vec![root.into(), "/sp/a".into()] } else { vec![root.into()] };
        if gone && (pin != Value::Null || notes.len() != 1 || notes[0]["title"] != "repo.gone" || notes[0]["level"] != "warning" || opened != want_opened) {
            failures.push(format!("{root}: gone, but pin {pin}, notes {notes:?}, opened {opened:?}"));
        }
        let badge = state.shared.borrow().get("yuzora.badge.git").cloned().unwrap_or(Value::Null);
        if !gone && (pin["root"] != root || !notes.is_empty() || opened != want_opened || badge != want_badge) {
            failures.push(format!("{root}: still there, but pin {pin}, notes {notes:?}, opened {opened:?}, badge {badge}"));
        }
        let shows_above = state.notes.borrow().contains(&json!(["error", "repo.opensAbove"]));
        if (root == "/sp/broken") != shows_above {
            failures.push(format!("{root}: shows {:?}", state.notes.borrow().last()));
        }
        visual.update(|window, _| window.remove_window());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(L2): the repo picker works from the keyboard. The command "yuzora-sidebar-git.pick-repo"
// opens it, centered (no pointer to put it at; other commands open nothing). In it ↑↓ move
// between "follow the focus" and the repos, round, starting on the one picked now; Enter picks
// it, Esc closes without picking.
#[gpui::test]
fn yuzora_git_repo_picker_works_from_the_keyboard(cx: &mut TestAppContext) {
    let state = Rc::new(GitSidebar::default());
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    state.commands.borrow_mut().extend(["something-else".to_owned(), "pick-repo".to_owned()]);
    let module = git_sidebar_module(state.clone(), Value::Null, &[], |_| Ok(json!({"repos": [], "truncated": false})));
    let (_fixture, mut visual) = git_sidebar_mount(cx, &git_sidebar_source(&[]), module);
    let opened: Vec<Value> = state.effects.borrow().iter().filter(|(name, _)| name == "openOverlay").map(|(_, v)| v.clone()).collect();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let (o, picker) = (&opened[0], &opened[0]["props"]["picker"]);
    assert_eq!((&o["mod"], &o["kind"], o.get("at")), (&json!("yuzora-sidebar-git"), &json!("dialog"), None));
    assert_eq!((&picker["space"], &picker["folder"], &picker["cwds"]), (&json!("s1"), &json!("/sp"), &json!(["/sp/a", "/sp/b/c"])));
    visual.update(|window, _| window.remove_window());

    fn found(_: &str) -> Result<Value, String> {
        Ok(json!({"repos": [{"root": "/sp/a", "name": "a", "rel": "a", "trusted": false}, {"root": "/sp/b", "name": "b", "rel": "b", "trusted": false}], "truncated": false}))
    }
    let (a, b) = (json!({"root": "/sp/a", "on": null}), json!({"root": "/sp/b", "on": null}));
    let pin = |root: &Value| vec![("setShared".to_owned(), json!(["yuzora.git.repo.s1", root])), ("closeOverlay".to_owned(), json!("o1"))];
    let close = vec![("closeOverlay".to_owned(), json!("o1"))];
    for (pinned, keys, want) in [
        (None, "enter", pin(&Value::Null)),
        (None, "down enter", pin(&a)),
        (None, "down down enter", pin(&b)),
        (None, "up enter", pin(&b)),
        (None, "down down down enter", pin(&Value::Null)),
        (Some("/sp/b"), "enter", pin(&b)),
        (Some("/sp/b"), "up enter", pin(&a)),
        (None, "down escape", close.clone()),
    ] {
        let state = Rc::new(GitSidebar::default());
        let props = json!({"overlay": "o1", "picker": {"space": "s1", "folder": "/sp", "on": null, "cwds": [], "current": pinned.unwrap_or("/sp/a"), "pinned": pinned.is_some()}});
        let module = git_sidebar_module(state.clone(), props, &[], found);
        let (_fixture, mut visual) = git_sidebar_mount(cx, &git_sidebar_source(&[]), module);
        visual.simulate_keystrokes(keys);
        git_sidebar_settle(&mut visual);
        assert_eq!(*state.effects.borrow(), want, "pinned {pinned:?}, keys {keys}");
        visual.update(|window, _| window.remove_window());
    }
}

// yuzora(L2): the repo picker (the sidebar as a popup) lists the space's repos from `repos` (its
// folder, machine and panes' folders), marks the trusted ones and a list cut short, and pins the
// one clicked for its space, or unpins (follow the focus).
#[gpui::test]
fn yuzora_git_repo_picker_lists_the_spaces_repos_and_pins_one(cx: &mut TestAppContext) {
    use gpui::{Modifiers, point, px};
    let source = git_sidebar_source(&[
        ("r.trusted ? t(\"repo.trusted\") : null", "r.trusted ? (note(\"trusted\", r.name), t(\"repo.trusted\")) : null"),
        (".child(t(\"repo.truncated\", { count: list.length }))", ".child((note(\"truncated\", list.length), t(\"repo.truncated\", { count: list.length })))"),
    ]);
    fn found(_: &str) -> Result<Value, String> {
        Ok(json!({"repos": [{"root": "/sp/a", "name": "a", "rel": "a", "trusted": true}, {"root": "/sp/b", "name": "b", "rel": "b", "trusted": false}], "truncated": true}))
    }
    let props = json!({"overlay": "o1", "picker": {"space": "s1", "folder": "/sp", "on": null, "cwds": ["/sp/a"], "current": "/sp/a", "pinned": false}});
    // Rows of 26 px: the follow row under 4 px of padding, a 1 px line with 2 px around it, then the repos.
    for (y, want) in [(74., json!({"root": "/sp/b", "on": null})), (17., Value::Null)] {
        let state = Rc::new(GitSidebar::default());
        let module = git_sidebar_module(state.clone(), props.clone(), &[], found);
        let (_fixture, mut visual) = git_sidebar_mount(cx, &source, module);
        assert_eq!(state.listed.borrow().clone(), [json!(["/sp", null, ["/sp/a"]])]);
        assert!(state.notes.borrow().contains(&json!(["trusted", "a"])) && !state.notes.borrow().contains(&json!(["trusted", "b"])), "{:?}", state.notes.borrow());
        assert!(state.notes.borrow().contains(&json!(["truncated", 2])), "{:?}", state.notes.borrow());
        assert!(state.opened.borrow().is_empty(), "the picker opened a repo");
        visual.simulate_click(point(px(60.), px(y)), Modifiers::none());
        git_sidebar_settle(&mut visual);
        let effects = state.effects.borrow().clone();
        assert_eq!(effects, [("setShared".to_string(), json!(["yuzora.git.repo.s1", want])), ("closeOverlay".to_string(), json!("o1"))], "a click at y {y}");
        visual.update(|window, _| window.remove_window());
    }
}
