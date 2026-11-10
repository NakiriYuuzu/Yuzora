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
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay } from \"herdr\";\nimport { t, tokens, session } from \"yuzora:core\";\nimport { open, changes, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from \"yuzora:git\";",
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, session, t, tokens, open, changes, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, gitSettings } from \"fixture\";",
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
        for name in ["shared", "setShared", "run", "panes", "focused", "closeOverlay", "diffStage", "diffUnstage", "diffDiscard"] {
            module = module.function(name, |_| Ok(HostValue::Null));
        }
        module = module.function("t", |a| Ok(a.string(0)?.into()));
        module = module.function("session", |_| Ok("fixture-run".into()));
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

// yuzora(L2): side by side, each side of the Git diff is as wide as its own longest line. An
// added file's empty left side stays narrow, so its lines start in view (they used to start past
// the longest line's width), and a press on a side's numbers picks that side's line where the
// row really lays it out. The shipped mod with its imports stubbed, in the real layout.
#[gpui::test]
fn yuzora_diff_sides_are_as_wide_as_their_own_lines(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px, size};
    const GUTTER: f64 = 44.;
    const CHAR: f64 = 7.;
    const PANE: f64 = 1200.;
    let mut source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods/yuzora-git-diff/main.js")).unwrap();
    for (from, to) in [
        (
            "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay } from \"herdr\";\nimport { t, tokens, session } from \"yuzora:core\";\nimport { open, changes, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from \"yuzora:git\";",
            "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, session, t, tokens, open, changes, diffOpen, diffView, diffStage, diffUnstage, diffDiscard, gitSettings, note } from \"fixture\";",
        ),
        // A list's rows can't report their size, so the list's own first row is built once more
        // in the same render and laid out as the list lays out its rows: as wide as the list
        // (absolute in the rows box, below the rows pressed further down).
        (
            "import { v_flex, h_flex, v_virtual_list, Scrollbar, VirtualListScrollHandle } from \"gpui-base\";",
            "import { v_flex, h_flex, v_virtual_list as list, Scrollbar, VirtualListScrollHandle } from \"gpui-base\";\nlet probe = null;\nconst v_virtual_list = (id, n, h, key, items) => { probe = items({ start: 0, end: 1 })[0]; return list(id, n, h, key, items); };",
        ),
        (".track_scroll(this.scroll).size_full())", ".track_scroll(this.scroll).size_full()).child(div().absolute().top(300).left(0).w_full().child(probe))"),
        // Each cell reports its laid-out width: side, whether its row has a hunk (drawn with a
        // 2px left border), width.
        ("return h_flex().flex_1()", "return h_flex().id(`c${side}`).on_size_change((s) => note(\"cell\", side, r.h !== undefined, s.width)).flex_1()"),
        ("toggleLine(h, side, index, cx) {", "toggleLine(h, side, index, cx) { note(\"pick\", side, index);"),
        // The list's item click (the row's key) never reaches the mod in a test window, so the
        // rows box's own press, which records where it landed, hands clickRow the row under it.
        (
            ".on_mouse_down(\"left\", (e) => { this.downX = e.local_position?.x ?? null; })",
            ".on_mouse_down(\"left\", (e, cx) => { this.downX = e.local_position?.x ?? null; this.clickRow(Math.floor(e.local_position.y / ROW), cx); })",
        ),
    ] {
        assert_eq!(source.matches(from).count(), 1, "the diff mod no longer has: {from}");
        source = source.replacen(from, to, 1);
    }
    let row = |left: Option<usize>, right: Option<usize>| {
        let mut r = json!({ "k": "~", "h": 0 });
        if let Some(n) = left { r["l"] = json!(1); r["li"] = json!(0); r["lt"] = json!("a".repeat(n)); }
        if let Some(n) = right { r["r"] = json!(1); r["ri"] = json!(0); r["rt"] = json!("b".repeat(n)); }
        r
    };
    // (case, rows, the longest line on the left and on the right)
    let cases = [
        // An added file: nothing on the left.
        ("added", json!([row(None, Some(257)), row(None, Some(5)), row(None, Some(40))]), 0, 257),
        // The right side needs more than half: it keeps its width, the left takes the rest. An
        // unchanged row whose sides differ in whitespace shows its `rt` on the right.
        ("right wider", json!([row(Some(10), Some(80)), {"k": "=", "l": 2, "r": 2, "t": "a".repeat(10), "rt": "b".repeat(90)}]), 10, 90),
        ("left wider", json!([row(Some(90), Some(10)), row(Some(5), None)]), 90, 10),
        // Both fit in half: the row splits evenly, as before.
        ("both fit", json!([row(Some(10), Some(20))]), 10, 20),
    ];
    let mut failures = Vec::new();
    for (case, rows, left_longest, right_longest) in cases {
        let fixture = Fixture::new(&source);
        let notes = Rc::new(RefCell::new(Vec::<Value>::new()));
        let mut module = HostModule::new("fixture");
        for (name, value) in [
            ("theme", json!({"background": "#101010", "foreground": "#e0e0e0", "border": "#303030", "raised": "#202020", "dim": "#808080", "muted": "#a0a0a0", "accent": "#4080ff", "selected": "#304050", "hover": "#282828", "ansi1": "#f08a8a", "ansi2": "#86d17a"})),
            ("props", json!({"cwd": "/r", "on": null, "path": "wide-new.ts", "mode": "unstaged", "at": 1})),
            ("tokens", json!({"diff": {"added": "#86d17a", "removed": "#f08a8a", "added_bg": "#103010", "removed_bg": "#301010"}})),
            ("open", json!({"id": "repo"})),
            ("diffOpen", json!({"id": "d1"})),
            ("diffView", json!({"id": "d1", "rows": rows, "hunks": [{}], "stageable": true, "add": 3, "del": 0, "leftExists": true, "rightExists": true})),
            ("settings", json!({})),
            ("gitSettings", json!({})),
        ] {
            module = module.function(name, move |_| Ok(to_host(&value)));
        }
        for name in ["shared", "setShared", "run", "panes", "focused", "closeOverlay", "diffStage", "diffUnstage", "diffDiscard"] {
            module = module.function(name, |_| Ok(HostValue::Null));
        }
        module = module.function("t", |a| Ok(a.string(0)?.into()));
        module = module.function("session", |_| Ok("fixture-run".into()));
        module = module.async_function("changes", |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
        let calls = notes.clone();
        module = module.function("note", move |a| {
            calls.borrow_mut().push(Value::Array((0..4).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(HostValue::Null)
        });
        gpui_shell::policy::set_default(Policy::new().with_host_module(module).unwrap());
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
        let mut visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(size(px(PANE as f32), px(600.)));
        for _ in 0..3 {
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
        }
        // The first row's cells (the row has a hunk).
        let width = |side: &str| notes.borrow().iter().rev()
            .find(|n| n[0] == "cell" && n[1] == side && n[2] == true)
            .and_then(|n| n[3].as_f64());
        let (Some(left), Some(right)) = (width("l"), width("r")) else {
            failures.push(format!("{case}: no cell widths reported: {:?}", notes.borrow()));
            continue;
        };
        let holds = |longest: usize| GUTTER + 6. + longest as f64 * CHAR;
        // The right side starts after the row's 2px border, the left side and the 1px divider.
        let right_at = 2. + left + 1.;
        if left < holds(left_longest) || right < holds(right_longest) {
            failures.push(format!("{case}: left {left} / right {right} can't hold {left_longest} / {right_longest} characters"));
        }
        if right_at + GUTTER + 6. + 10. * CHAR > PANE {
            failures.push(format!("{case}: the right side starts at {right_at}, past the {PANE}px pane"));
        }
        if case == "both fit" && (left - right).abs() > 1. {
            failures.push(format!("{case}: the row didn't split evenly: {left} / {right}"));
        }
        // Presses on row 0 (y 28–48, under the 28px header): the middle of each side's numbers
        // picks its line; the text beside them only makes the hunk current.
        let mut presses = vec![(right_at + GUTTER / 2., Some("r")), (right_at + GUTTER + 20., None), (2. + GUTTER + 12., None)];
        if left_longest > 0 {
            presses.push((2. + GUTTER / 2., Some("l")));
        }
        for (x, want) in presses {
            if x >= PANE {
                failures.push(format!("{case}: x {x} is past the pane"));
                continue;
            }
            let before = notes.borrow().iter().filter(|n| n[0] == "pick").count();
            visual.simulate_mouse_down(point(px(x as f32), px(38.)), MouseButton::Left, Modifiers::none());
            visual.update(|window, cx| window.draw(cx).clear(cx));
            visual.run_until_parked();
            let picks: Vec<Value> = notes.borrow().iter().filter(|n| n[0] == "pick").skip(before).cloned().collect();
            if picks.first().and_then(|p| p[1].as_str()) != want || picks.len() > 1 {
                failures.push(format!("{case}: a press at x {x} (left {left}, right {right}) picked {picks:?}, wanted {want:?}"));
            }
        }
        visual.update(|window, _| window.remove_window());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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
    let s = state.clone();
    module = module.async_function("status", move |a| {
        let root = a.string(0)?.to_owned();
        s.effects.borrow_mut().push(("status".into(), json!(root)));
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
    module = module.async_function("messages", move |_| Ok(answer(Ok(json!({"history": [], "template": null, "last": null})))));
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

/// [`git_sidebar_mount`], keeping what the window asks a mod to draw again with when herdr's data changes.
fn git_sidebar_mount_live(
    cx: &mut TestAppContext,
    source: &str,
    module: HostModule,
) -> (
    Fixture,
    VisualTestContext,
    Rc<ShellRuntime>,
    gpui::Entity<ShellRoot>,
) {
    let fixture = Fixture::new(source);
    gpui_shell::policy::set_default(Policy::new().with_host_module(module).unwrap());
    let runtime = ShellRuntime::new_isolated().unwrap();
    let window =
        cx.add_window(|window, cx| Mounted(runtime.try_load(&fixture.0, window, cx).unwrap()));
    let root = window
        .update(cx, |mounted, _, _| mounted.0.clone())
        .unwrap();
    let mut visual = VisualTestContext::from_window(*window.deref(), cx);
    git_sidebar_settle(&mut visual);
    (fixture, visual, runtime, root)
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

// yuzora(v0.1.0 E1): the sidebar looks at where it is in the frame the focus moves in: the window draws a mod
// again when herdr's data changes, and no second goes by before the new place is opened (the clock stays where it
// is). A place seen before is shown from what was read of it, so going back opens it again but shows its changes
// at once.
#[gpui::test]
fn yuzora_git_sidebar_follows_its_place_in_the_frame_it_changes(cx: &mut TestAppContext) {
    let state = Rc::new(GitSidebar::default());
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    let module = git_sidebar_module(state.clone(), Value::Null, &[], |_| {
        Ok(json!({"repos": [], "truncated": false}))
    });
    let (_fixture, mut visual, runtime, root) =
        git_sidebar_mount_live(cx, &git_sidebar_source(&[]), module);
    let opened = |state: &GitSidebar| state.opened.borrow().clone();
    assert_eq!(opened(&state), ["/sp/a"]);
    let draw_again = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| runtime.refresh(&root, cx).unwrap());
        git_sidebar_settle(visual);
    };
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p2"});
    draw_again(&mut visual);
    assert_eq!(
        opened(&state),
        ["/sp/a", "/sp/b/c"],
        "the focus moved to another pane's folder"
    );
    assert_eq!(
        state.shared.borrow().get("yuzora.badge.git").cloned(),
        Some(json!(2)),
        "and its changes are the badge"
    );
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    draw_again(&mut visual);
    assert_eq!(opened(&state), ["/sp/a", "/sp/b/c", "/sp/a"], "and back");
    assert_eq!(
        state.shared.borrow().get("yuzora.badge.git").cloned(),
        Some(json!(1))
    );
    visual.update(|window, _| window.remove_window());
}

// yuzora(v0.1.0 E1 r1): an arrival ends at its first wait that finds its place left. The focus goes to another
// pane and back before the first `open` has answered (two draws, nothing run between): the arrival for the pane
// left throws and the sidebar catches it, in QuickJS as it does in the bun tests, and only the place the focus
// ended at is shown and followed.
#[gpui::test]
fn yuzora_git_sidebar_arrival_for_a_place_left_ends_quietly(cx: &mut TestAppContext) {
    let state = Rc::new(GitSidebar::default());
    *state.focused.borrow_mut() = json!({"space": "s1", "pane": "p1"});
    let module = git_sidebar_module(state.clone(), Value::Null, &[], |_| {
        Ok(json!({"repos": [], "truncated": false}))
    });
    let (_fixture, mut visual, runtime, root) =
        git_sidebar_mount_live(cx, &git_sidebar_source(&[]), module);
    for pane in ["p2", "p1"] {
        *state.focused.borrow_mut() = json!({"space": "s1", "pane": pane});
        visual.update(|window, cx| {
            runtime.refresh(&root, cx).unwrap();
            window.draw(cx).clear(cx);
        });
    }
    git_sidebar_settle(&mut visual);
    assert_eq!(state.opened.borrow().clone(), ["/sp/a", "/sp/b/c", "/sp/a"]);
    let badges: Vec<Value> = state
        .effects
        .borrow()
        .iter()
        .filter(|(name, v)| name == "setShared" && v[0] == "yuzora.badge.git")
        .map(|(_, v)| v[1].clone())
        .collect();
    assert!(
        !badges.contains(&json!(2)),
        "the pane that was left was shown: {badges:?}"
    );
    let read: Vec<Value> = state
        .effects
        .borrow()
        .iter()
        .filter(|(name, _)| name == "status")
        .map(|(_, v)| v.clone())
        .collect();
    assert!(
        !read.contains(&json!("/sp/b/c")),
        "the pane that was left was followed: {read:?}"
    );
    assert_eq!(
        state.shared.borrow().get("yuzora.badge.git").cloned(),
        Some(json!(1))
    );
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
        let source = git_sidebar_source(&[(".child(v.error ?? t(\"loading\"))", ".child((note(\"error\", v.error ?? null), v.error ?? t(\"loading\")))")]);
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
