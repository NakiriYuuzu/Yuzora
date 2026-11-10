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
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, openOverlay, notify } from \"herdr\";\nimport { t, tokens, session } from \"yuzora:core\";\nimport { open, changes, diffOpen, diffView, diffText, diffPatch, diffStage, diffUnstage, diffDiscard, settings as gitSettings } from \"yuzora:git\";",
        "import { theme, props, shared, setShared, settings, run, panes, focused, closeOverlay, openOverlay, notify, session, t, tokens, open, changes, diffOpen, diffView, diffText, diffPatch, diffStage, diffUnstage, diffDiscard, gitSettings } from \"fixture\";",
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

// yuzora(v010): the shipped Git diff in a window, its herdr and yuzora:git stubbed. A list's rows
// hold no handlers and report no sizes, so `Probed` lays the first row out once more below the
// list: its cells report their widths (`cell`), its texts their width and what the pointer is
// over (`text` and `over`: the text's first character, and the pointer's distance from where the
// text starts), and a press picks the row it landed on, as the list's own click would, in the
// body's text size (a `real` prop leaves the clicks to the list, and reports the keys that get past the diff to what
// holds it: the list's own click picks the row; a `hold` prop stops the ranking after its first slice till F12 is
// pressed, and F11 reads the diff again: a list of many rows leaves no place for a control to click). A press in the
// header's empty part loads the view again (`relayout`). `rendered` is noted when a frame was
// drawn whole, `shown` how many times the rows' properties were read in taking the diff in, and `work`
// in a frame (the rows are proxies that count, made as they are read, so a diff of 200,000 rows costs no more to
// take in than the rows it is read of), `ranked` the same for a slice of the ranking that follows, `range` how far
// the lines scroll as the mod worked it out in each frame, and `list` how tall the rows' list is. A `pane` prop
// puts the diff in a box that wide, so the pointer can leave it.
//
// What a frame costs is counted beyond the rows: the module's built-in objects are frozen, so the string and
// array methods cannot be metered, and what `Probed` counts is every read of the rows and of the array that
// holds them (`reads`), what goes through the module's own scans (`bound` and what ranks the lines: the characters
// and rows they are given, `scans`; the rows a slice ranked, `rows_ranked`) and the texts a frame builds and their
// characters (`built`, in `work`), which `shown` and `work` note in that order. What a loop over a copy of the rows
// costs cannot be counted that way, so a diff of 200,000 rows is drawn: a frame has 50 ms. The window's text system
// counts the characters it lays out for each frame (`MeasuredFont`, `laid`). `pending` is noted with each frame:
// whether the diff still has rows to rank or lines to lay out out of sight, so a test draws until it has none.
const DIFF_PROBE: &str = r#"
import { note } from "fixture";
const scanned = { n: 0 }, built = { n: 0 };
if (typeof bound === "function") { const f = bound; bound = (s, ...rest) => { scanned.n += s.length; return f(s, ...rest); }; }
if (typeof ranks === "function") { const f = ranks; ranks = (rows, ...rest) => { scanned.n += rows.length; return f(rows, ...rest); }; }
// A machine the engine cuts off earlier than this one: a `cut_event` prop is the units of the ranking an event gets, a
// `cut_render` the cost of the batch a frame gets. Past them the script goes on till the engine's own clock cuts it off (an
// uncatchable error, that no `catch` or `finally` sees), as on a slower machine, whatever the speed of this one. These are
// standing limits: every slice or frame past them is cut. The others cut once, as a machine that stops for a moment does: a
// `stall_slice` prop is the units the first slice that goes so far in is cut at, `cut_first` the first frame that has a
// batch to lay out, `cut_big` the first frame whose batch has more lines than that, and `cut_last` the first frame in which
// nothing is left to find out.
let failed = 0, stalled = false;
if (typeof scan === "function") {
  const f = scan;
  scan = (fit, meter) => {
    if (failed < (Number(props().fail) || 0)) { failed++; throw new Error("a row that is not what the rows are"); }
    const from = fit.at, limit = Number(props().cut_event) || 0, stall = Number(props().stall_slice) || 0, start = meter.left;
    const metered = limit || stall ? { get left() { return meter.left; }, set left(v) {
      meter.left = v;
      if (limit && start - v > limit) for (;;) {}
      if (stall && !stalled && start - v > stall) { stalled = true; for (;;) {} }
    } } : meter;
    try { return f(fit, metered); } finally { scanned.n += fit.at - from; }
  };
}
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
    built.n += 1 + text.length;
    const el = super.segments(segs, text, ...rest), first = String.fromCodePoint(text.codePointAt(0));
    return this.probing ? el.id(`probe-text-${first}`).on_size_change((s) => note("text", first, s.width)).on_mouse_move((e) => note("over", first, e.local_position.x)) : el;
  }
  rowList(...rest) {
    return div().size_full().on_size_change((s) => note("list", s.height)).child(super.rowList(...rest));
  }
  show(view) {
    if (props().timed) { const at = Date.now(); super.show(view); note("took", Date.now() - at); return; }
    const touched = this.touched = { n: 0 }, wrapped = new Map();
    const counting = { get(t, k, rc) { touched.n++; return Reflect.get(t, k, rc); } };
    const rows = new Proxy(view.rows, { get(t, k, rc) {
      touched.n++;
      const v = Reflect.get(t, k, rc);
      if (typeof k !== "string" || v === null || typeof v !== "object" || !(Number(k) >= 0)) return v;
      if (!wrapped.has(k)) wrapped.set(k, new Proxy(v, counting));
      return wrapped.get(k);
    } });
    const before = scanned.n;
    super.show({ ...view, rows });
    note("shown", touched.n, scanned.n - before, this.fit?.at ?? view.rows.length);
  }
  clickRow(i, cx) { note("clickRow", i); super.clickRow(i, cx); }
  ranked(fit, cx) {
    if (props().hold && fit.at > 0 && !this.released) return;
    const reads = this.touched?.n ?? 0, before = scanned.n, from = fit.at, at = Date.now();
    note("slice", fit.at);
    super.ranked(fit, cx);
    note("ranked", (this.touched?.n ?? 0) - reads, scanned.n - before, fit.at - from, Date.now() - at);
  }
  batch(name) {
    const batch = super.batch(name);
    if (batch.lines.length) {
      this.asked = true; this.cost = Math.max(this.cost, batch.cost); this.big = Math.max(this.big, batch.lines.length);
      note("batch", name, batch.from);
    }
    return batch;
  }
  probes(...rest) {
    const out = super.probes(...rest), limit = Number(props().cut_render) || 0;
    if (limit && this.cost > limit) for (;;) {}
    if (props().cut_first && !this.cutFirst && this.asked) { this.cutFirst = true; for (;;) {} }
    if (Number(props().cut_big) && !this.cutBig && this.big > Number(props().cut_big)) { this.cutBig = true; for (;;) {} }
    if (props().cut_last && !this.spun && this.fit.done && !this.waiting()) { this.spun = true; for (;;) {} }
    return out;
  }
  measured(...rest) {
    if ((this.dropped ?? 0) < (props().drop ?? 0)) { this.dropped = (this.dropped ?? 0) + 1; return; }
    super.measured(...rest);
  }
  render(cx) {
    const before = this.touched?.n ?? 0, scan = scanned.n, made = built.n;
    this.asked = false; this.cost = 0; this.big = 0;
    note("frame");
    note("at", this.fit?.at ?? null);
    let root = super.render(cx);
    note("work", (this.touched?.n ?? 0) - before, scanned.n - scan, built.n - made);
    note("range", this.lay?.reach ?? null);
    note("offset", this.lay?.sx ?? null, this.lay?.reach ?? null);
    note("mark", this.mark ? [this.mark.side, this.mark.anchor, this.mark.head] : null);
    note("now", this.scroll ? this.scroll.offset() : null);
    root = root.on_mouse_down("left", (e, cx) => {
      if (e.position.y < 28) { if (this.view) this.relayout(cx); }
      else if (!props().real) this.clickRow(Math.floor((e.position.y - 28) / 20), cx);
    });
    const v = this.view, pane = props().pane;
    if (v && v.rows.length && this.layout !== "unified") {
      this.probing = true;
      const row = this.row(v.rows[0], 0, theme(), tokens().diff ?? {}, 0);
      this.probing = false;
      root = root.child(div().absolute().top(300).left(0).w_full().font_family(settings().font_family ?? "JetBrains Mono").text_size(11.5).child(row));
    }
    note("rendered", v ? v.id : null, this.error);
    note("pending", this.asked || this.fit?.done === false || (this.gauge && this.unit?.key !== this.gauge.key) ? 1 : 0);
    if (props().real) {
      root = root.child(div().id("t-unified").absolute().top(520).left(0).w(60).h(20).on_click((_e, cx) => { this.layout = "unified"; this.relayout(cx); }));
      root = root.child(div().id("t-other").absolute().top(520).left(70).w(60).h(20).on_click((_e, cx) => { setShared("yuzora.git.diff", { cwd: "/r", on: null, path: "other.ts", mode: "unstaged", at: 99 }); cx.notify(); }));
      root = root.child(div().id("t-reload").absolute().top(520).left(140).w(60).h(20).on_click((_e, cx) => { cx.spawn((cx) => this.load(cx)); }));
    }
    if (props().real) return div().size_full().on_scroll_wheel((_e, cx) => {
      if (props().hold) { this.released = true; this.ranked(this.fit, cx); }
      if (props().reload) cx.spawn((cx) => this.load(cx));
    }).on_key_down((e, cx) => {
      note("ancestor", e.keystroke);
      if (e.key === "f11") cx.spawn((cx) => this.load(cx));
      if (e.key === "f12") { this.released = true; this.ranked(this.fit, cx); }
      if (e.key === "f9") note("hunk", this.hunk);
      if (e.key === "f8") note("scrolled", typeof this.scroll.offset === "function" ? this.scroll.offset() : null);
    }).child(root);
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
    /// What `diffView` answers next, in order, before the view the diff was opened with.
    views: Rc<RefCell<std::collections::VecDeque<Value>>>,
    wide: f32,
    /// The window's height, and the zoom the person set (`settings().zoom`) with the runtime and root that tell
    /// the mod it changed.
    high: f32,
    zoom: f32,
    person: Rc<RefCell<Value>>,
    runtime: Rc<ShellRuntime>,
    root: gpui::Entity<ShellRoot>,
    /// How many frames the last [`Self::settle`] drew after the three it always does, for the diff to have nothing
    /// left to lay out out of sight.
    extra: usize,
    /// The characters the window laid out for each frame the mod rendered (only in a [`MeasuredFont`] window).
    laid: Rc<RefCell<Vec<usize>>>,
    /// Whether the diff is meant to have lines to lay out that its frames keep asking for (a `drop` prop loses the
    /// first sizes it is told, so a batch stays in flight).
    expects_pending: bool,
    /// The clicks this window sent again because the list reported none: see [`Resends`].
    resent: Resends,
    _fixture: Fixture,
}

/// How many clicks a window may send again. The press makes the window draw, and the engine gives the list's rows 50 ms of the
/// machine's own time: on a busy machine (a debug build, 66 rows in a window 1300 px tall) the rows are cut off in that draw, the
/// list paints none, its rows forget the press, and the release is no click. Measured with `clickRow` not being called, with 16 busy
/// loops added to a load of 25: of 1,800 clicks, 5 were lost, each with a draw in the press that built no row (and 5 such draws
/// after the press lost none); and the mod before the drag, the one at the head of the review and this one, taken in turn, lost
/// 5, 2 and 2 of 1,800 each, which is no difference to speak of (the first alone lost 0 and 9 of 300 in two runs). A click sent again
/// is the same click. Counting them, and failing past the limit, keeps a click that is lost for real from passing as a busy machine.
#[derive(Default)]
struct Resends(usize);

impl Resends {
    const LIMIT: usize = 3;

    fn another(&mut self, x: f64, y: f64) {
        self.0 += 1;
        eprintln!(
            "diff test: the list reported no click at ({x}, {y}); sent again ({} so far in this window)",
            self.0
        );
        assert!(
            self.0 <= Self::LIMIT,
            "the list reported no click at ({x}, {y}), the {}th time in this window: a machine this busy cannot run this test, or the click is lost for real",
            self.0
        );
    }
}

#[test]
fn yuzora_diff_a_window_may_send_a_few_clicks_again_and_counts_them() {
    let mut resends = Resends::default();
    for _ in 0..Resends::LIMIT {
        resends.another(0., 0.);
    }
    assert_eq!(resends.0, Resends::LIMIT);
}

#[gpui::test]
fn yuzora_diff_a_click_the_list_never_reports_is_sent_again_a_few_times_and_then_fails(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pane.click_arrives(600., COPY_HEADER / 2., false)
    }));
    let message = outcome.expect_err("a click on the header is never reported by the list");
    let message = message
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(message.contains("the list reported no click"), "{message}");
    assert_eq!(pane.resent.0, Resends::LIMIT + 1);
    pane.close();
}

#[test]
#[should_panic(expected = "the list reported no click")]
fn yuzora_diff_a_window_that_sends_clicks_again_more_than_it_may_fails() {
    let mut resends = Resends::default();
    for _ in 0..=Resends::LIMIT {
        resends.another(0., 0.);
    }
}
type Shared = Rc<RefCell<std::collections::BTreeMap<String, Value>>>;

impl DiffPane {
    /// The diff of `rows`, `pane` wide, with `shared` as herdr's shared state.
    fn mount(cx: &mut TestAppContext, rows: Value, pane: f32, shared: &Shared) -> Self {
        Self::mount_with(cx, rows, pane, pane, shared, json!({}))
    }

    /// The same in a window `wide` wide (more than `pane`: room to leave it), opened with `props` besides.
    fn mount_with(cx: &mut TestAppContext, rows: Value, pane: f32, wide: f32, shared: &Shared, props: Value) -> Self {
        Self::mount_at(cx, Diff::of(rows), pane, (wide, 600.), 1., shared, props)
    }

    /// A whole `diff`: besides its rows, the lines of both files that `diffText` hands back.
    fn mount_diff(cx: &mut TestAppContext, diff: Diff, pane: f32, wide: f32, shared: &Shared, props: Value) -> Self {
        Self::mount_at(cx, diff, pane, (wide, 600.), 1., shared, props)
    }

    /// The diff of `rows` in a 1200px window as tall as it takes to show the probe row at `zoom` (a test that
    /// sets the zoom runs alone, in a process of its own: see [`alone`]).
    fn zoomed(cx: &mut TestAppContext, rows: Value, zoom: f32, props: Value) -> Self {
        gpui_shell::set_pixel_scale(zoom);
        Self::mount_at(cx, Diff::of(rows), 1200., (1200., 1300.), zoom, &Shared::default(), props)
    }

    /// As [`Self::mount_with`], in a window `(wide, high)` big, the zoom set to `zoom` (see [`Self::zoomed`]).
    fn mount_at(cx: &mut TestAppContext, diff: Diff, pane: f32, (wide, high): (f32, f32), zoom: f32, shared: &Shared, props: Value) -> Self {
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
        let opened_drops = ["drop", "cut_event", "cut_render", "cut_last", "cut_first", "cut_big", "stall_slice", "fail", "hold"]
            .iter()
            .any(|name| props.get(name).is_some());
        let mut opened =
            json!({"cwd": "/r", "on": null, "path": "wide-new.ts", "mode": "unstaged", "at": 1});
        if wide > pane {
            opened["pane"] = json!(pane);
        }
        opened.as_object_mut().unwrap().extend(props.as_object().unwrap().clone());
        for (name, value) in [("theme", theme), ("props", opened)] {
            herdr = herdr.function(name, move |_| Ok(to_host(&value)));
        }
        let person = Rc::new(RefCell::new(json!({"zoom": zoom})));
        let read = person.clone();
        herdr = herdr.function("settings", move |_| Ok(to_host(&read.borrow())));
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
        for (name, value) in [("open", json!({"id": "repo"})), ("diffOpen", json!({"id": "d1"})), ("settings", json!({}))] {
            git = git.function(name, move |_| Ok(to_host(&value)));
        }
        let views = Rc::new(RefCell::new(std::collections::VecDeque::<Value>::new()));
        let queued = views.clone();
        git = git.function("diffView", move |_| Ok(to_host(&queued.borrow_mut().pop_front().unwrap_or_else(|| view.clone()))));
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
            .function("session", |_| Ok("fixture-run".into()))
            .function("tokens", |_| Ok(to_host(&json!({"diff": {"added": "#86d17a", "removed": "#f08a8a", "added_bg": "#103010", "removed_bg": "#301010"}}))));
        let calls = notes.clone();
        let laid = Rc::new(RefCell::new(Vec::<usize>::new()));
        let counted = laid.clone();
        let fixture_module = HostModule::new("fixture").function("note", move |a| {
            if a.string(0).is_ok_and(|name| name == "pending") {
                counted.borrow_mut().push(LAID.with(Cell::take));
            }
            calls.borrow_mut().push(Value::Array((0..5).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(HostValue::Null)
        });
        // As the app grants every mod: the clipboard is the test window's own.
        let mut policy = Policy::new().with_capabilities(Capabilities::new().clipboard_write(true));
        for module in [herdr, git, core, fixture_module] {
            policy = policy.with_host_module(module).unwrap();
        }
        gpui_shell::policy::set_default(policy);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let mut mounted = None;
        // Opened at its size, not resized after: made by `add_window` and resized, the window lays out the frame a diff comes in
        // during at the display's size (1920 by 1080) when a hover callback runs in the first frame, as one on the pane's root
        // does with the pointer at the window's corner, and that frame measures the rows at it.
        let bounds = gpui::Bounds::new(gpui::point(px(0.), px(0.)), size(px(wide), px(high)));
        let window = cx
            .update(|cx| {
                cx.open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                        ..Default::default()
                    },
                    |window, cx| {
                        let root = runtime.try_load(&fixture.0, window, cx).unwrap();
                        mounted = Some(root.clone());
                        gpui::AppContext::new(cx, |_| Mounted(root))
                    },
                )
            })
            .unwrap();
        let visual = VisualTestContext::from_window(*window.deref(), cx);
        let mut view = Self { visual, notes, views, wide, high, zoom, person, runtime, root: mounted.unwrap(), extra: 0, laid, expects_pending: opened_drops, resent: Resends::default(), _fixture: fixture };
        view.settle();
        view
    }

    /// The next `diffView` calls answer with `views`, in turn.
    fn answer_next(&self, views: Vec<Value>) {
        self.views.borrow_mut().extend(views);
    }

    /// The window `width` wide from now on.
    fn resize(&mut self, width: f32) {
        use gpui::{px, size};
        self.wide = width;
        self.visual.simulate_resize(size(px(width), px(self.high)));
        self.settle();
    }

    /// Draws the diff again (the window a pixel narrower, then as it was).
    fn redraw(&mut self) {
        let wide = self.wide;
        self.resize(wide - 1.);
        self.resize(wide);
    }

    /// The frames drawn whole, in order: the view each showed and its error.
    fn rendered(&self) -> Vec<(Value, Value)> {
        self.notes.borrow().iter().filter(|n| n[0] == "rendered").map(|n| (n[1].clone(), n[2].clone())).collect()
    }

    /// How many times the rows' properties were read (taking the diff in, and in each frame).
    fn reads(&self, what: &str) -> Vec<f64> {
        self.notes.borrow().iter().filter(|n| n[0] == what).map(|n| n[1].as_f64().unwrap()).collect()
    }

    /// How many rows were ranked by each `shown` (taking the diff in) or `ranked` (a slice after it) note.
    fn rows_ranked(&self, what: &str) -> Vec<f64> {
        self.notes
            .borrow()
            .iter()
            .filter(|n| n[0] == what)
            .map(|n| n[3].as_f64().unwrap())
            .collect()
    }

    /// How many rows the ranking has been through by now, as the last frame saw it: a slice the engine cut off says nothing
    /// when it is back, and the rows it did are in the ranking all the same.
    fn rows_ranked_in_all(&self) -> f64 {
        self.notes
            .borrow()
            .iter()
            .rev()
            .find(|n| n[0] == "at")
            .and_then(|n| n[1].as_f64())
            .unwrap_or(0.)
    }

    /// Settles till the first slice of the ranking has been through some rows. Taking the diff in and the first slice are each
    /// a frame and an event away, however fast or slow the machine is that they run on, so how many frames it takes is not
    /// known beforehand; the clock the frames are drawn by is the test's own.
    fn settle_until_ranked_some(&mut self) {
        for _ in 0..30 {
            if self.rows_ranked_in_all() > 0. {
                break;
            }
            self.settle();
        }
    }

    /// How much the module's scans were given (characters of a line, lines of a side), taking the diff in, and in each frame.
    fn scans(&self, what: &str) -> Vec<f64> {
        self.notes.borrow().iter().filter(|n| n[0] == what).map(|n| n[2].as_f64().unwrap()).collect()
    }

    /// How tall the rows' list was laid out last.
    fn list_height(&self) -> f64 {
        self.notes.borrow().iter().rev().find(|n| n[0] == "list").and_then(|n| n[1].as_f64()).expect("no list height reported")
    }

    /// Draws three frames, and more while the diff has lines left to lay out out of sight (one batch of them is asked
    /// for and answered a frame).
    fn settle(&mut self) {
        let draw = |pane: &mut Self| {
            pane.visual.update(|window, cx| window.draw(cx).clear(cx));
            pane.visual.run_until_parked();
        };
        for _ in 0..3 {
            draw(self);
        }
        self.extra = 0;
        let limit = if self.expects_pending { 10 } else { 400 };
        while (self.pending() || self.cut_frame()) && self.extra < limit {
            self.visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(150));
            draw(self);
            self.extra += 1;
        }
        assert!(self.expects_pending || !self.pending(), "the diff still had lines to lay out after {} frames", 3 + self.extra);
    }

    /// How far the lines scroll as the mod worked it out, in each frame it rendered (none: no room for a bar yet).
    fn ranges(&self) -> Vec<f64> {
        self.notes.borrow().iter().filter(|n| n[0] == "range").map(|n| n[1].as_f64().unwrap_or(0.)).collect()
    }

    /// How many notes there are: what [`Self::drawn_since`] counts from.
    fn mark(&self) -> usize {
        self.notes.borrow().len()
    }

    /// The offset the lines were drawn at and the range they could scroll, in each frame drawn since `mark`.
    fn drawn_since(&self, mark: usize) -> Vec<(f64, f64)> {
        self.notes
            .borrow()
            .iter()
            .skip(mark)
            .filter(|n| n[0] == "offset")
            .map(|n| (n[1].as_f64().unwrap_or(0.), n[2].as_f64().unwrap_or(0.)))
            .collect()
    }

    /// What the last frame painted of the unified rows: how far the lines are scrolled and how wide the rows are (their
    /// backgrounds are the widest quads there are, as wide as the box they scroll in, and start that far to the left), how
    /// many rows are tinted (a quad the row's width less its numbers: 88px, and 2px more in the row of the current hunk,
    /// which has a border), and how much room is left after the end of the line that ends in a highlighted word (the
    /// window's width less where that word ends).
    fn unified_extent(&mut self) -> (f64, f64, usize, f64) {
        let scale = self.visual.update(|window, _| window.scale_factor());
        // (where it starts, how wide, how far down)
        let quads: Vec<(f64, f64, f64)> = self
            .visual
            .update(|window, _| window.painted_quads())
            .into_iter()
            .filter(|q| !q.background.is_transparent())
            .map(|q| {
                let at = |v: f32| (v / scale) as f64;
                (
                    at(q.bounds.origin.x.as_f32()),
                    at(q.bounds.size.width.as_f32()),
                    at(q.bounds.origin.y.as_f32()),
                )
            })
            .collect();
        let widest = quads.iter().map(|q| q.1).fold(0., f64::max);
        let start = quads
            .iter()
            .filter(|q| q.1 >= widest - 0.5)
            .map(|q| q.0)
            .fold(f64::INFINITY, f64::min);
        let tints = quads
            .iter()
            .filter(|q| q.1 > widest - 91. && q.1 < widest - 87.)
            .count();
        let word_end = quads
            .iter()
            .filter(|q| q.2 >= COPY_HEADER && q.1 < 40.)
            .map(|q| q.0 + q.1)
            .fold(f64::NEG_INFINITY, f64::max);
        (
            -start,
            widest,
            tints,
            self.wide as f64 / self.zoom as f64 - word_end,
        )
    }

    /// How far the lines scroll, from the thumb as the last frame painted it (width² / (width + reach)); none: no bar.
    fn reach(&mut self) -> Option<f64> {
        let w = self.wide as f64 / self.zoom as f64;
        self.thumb()
            .map(|(_, long)| w * w / (long / self.zoom as f64) - w)
    }

    /// How many times the script built the diff's frame (a frame the engine cut off included), since it was opened.
    fn script_renders(&self) -> u64 {
        self.runtime.read_metrics().script_renders()
    }

    /// Whether the last frame the script began was not finished: the engine cut it off, and a clock tick asks for another.
    fn cut_frame(&self) -> bool {
        self.notes
            .borrow()
            .iter()
            .rev()
            .find(|n| n[0] == "frame" || n[0] == "rendered")
            .is_some_and(|n| n[0] == "frame")
    }

    /// How many of the `begin` notes had no `end` after them: what the engine cut off, of what a test asked to be cut.
    fn cut(&self, begin: &str, end: &str) -> usize {
        let count = |name: &str| self.notes.borrow().iter().filter(|n| n[0] == name).count();
        count(begin) - count(end)
    }

    /// Frames a clock tick apart, till nothing asks for another: the tick is what asks again for a frame that the engine
    /// cut off, and nothing else does. Returns how many times the script built a frame to get there. A diff that never gets
    /// there is a test that fails, in `limit` ticks.
    fn settle_through_cuts(&mut self, limit: usize) -> u64 {
        let start = self.script_renders();
        let mut quiet = 0;
        for _ in 0..limit {
            let before = self.script_renders();
            self.visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(150));
            self.visual.update(|window, cx| window.draw(cx).clear(cx));
            self.visual.run_until_parked();
            quiet = if self.script_renders() == before {
                quiet + 1
            } else {
                0
            };
            if quiet >= 5 {
                return self.script_renders() - start;
            }
        }
        panic!(
            "the diff still asked for frames after {limit} ticks: {} built so far",
            self.script_renders() - start
        );
    }

    /// Whether the diff builds a frame in a clock of `seconds` more, with a frame drawn each tick of it.
    fn builds_in(&mut self, seconds: u64) -> u64 {
        let start = self.script_renders();
        for _ in 0..seconds * 1000 / 150 {
            self.visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(150));
            self.visual.update(|window, cx| window.draw(cx).clear(cx));
            self.visual.run_until_parked();
        }
        self.script_renders() - start
    }

    /// How many different batches of lines were asked to be laid out out of sight (a side, and where in its order it
    /// began), since the diff was opened: a batch asked for again, after a frame was cut off, is one.
    fn batches(&self) -> usize {
        let asked: std::collections::BTreeSet<(String, i64)> = self
            .notes
            .borrow()
            .iter()
            .filter(|n| n[0] == "batch")
            .map(|n| (n[1].to_string(), n[2].as_i64().unwrap_or(-1)))
            .collect();
        asked.len()
    }

    /// How many frames asked for lines to be laid out out of sight, since the diff was opened.
    fn asked_frames(&self) -> usize {
        self.notes
            .borrow()
            .iter()
            .filter(|n| n[0] == "pending" && n[1].as_f64() == Some(1.))
            .count()
    }

    /// Whether the last frame asked for lines to be laid out out of sight.
    fn pending(&self) -> bool {
        self.notes.borrow().iter().rev().find(|n| n[0] == "pending").is_some_and(|n| n[1].as_f64() == Some(1.))
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

    /// A press with Shift held (or not), down only: the button stays down till [`Self::release`].
    fn press_with(&mut self, x: f64, y: f64, shift: bool) {
        let modifiers = gpui::Modifiers {
            shift,
            ..gpui::Modifiers::none()
        };
        self.visual.simulate_event(gpui::MouseDownEvent {
            position: Self::at(x, y),
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.settle();
    }

    /// How far the list says it is scrolled now (window pixels), asked by a key that needs the pane to have the focus (a
    /// press has given it), or none if it cannot say.
    fn scrolled(&mut self) -> Option<f64> {
        self.keys("f8");
        self.notes
            .borrow()
            .iter()
            .rev()
            .find(|n| n[0] == "scrolled")
            .and_then(|n| n[1].as_f64())
    }

    /// `ticks` of `ms` milliseconds of the clock, a frame drawn after each: what a held pointer lets happen without moving.
    fn wait(&mut self, ticks: usize, ms: u64) {
        for _ in 0..ticks {
            self.visual
                .executor()
                .advance_clock(std::time::Duration::from_millis(ms));
            self.visual.update(|window, cx| window.draw(cx).clear(cx));
            self.visual.run_until_parked();
        }
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

    /// A click the list reports. The pointer is moved there first, as a platform does before a press. One the list does not report
    /// is sent again, counted by [`Resends`].
    fn click_arrives(&mut self, x: f64, y: f64, shift: bool) {
        for _ in 0..=Resends::LIMIT {
            let seen = self.noted("clickRow").len();
            self.hover(x, y);
            self.click(x, y, shift);
            if self.noted("clickRow").len() > seen {
                return;
            }
            self.resent.another(x, y);
        }
        panic!("the list reported no click at ({x}, {y})");
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
        self.wheel_there(x, y, dx, dy);
    }

    /// The same with the left button down: the pointer is moved there by a drag, as a platform reports it.
    fn wheel_held(&mut self, x: f64, y: f64, dx: f64, dy: f64) {
        self.drag(x, y);
        self.wheel_there(x, y, dx, dy);
    }

    fn wheel_there(&mut self, x: f64, y: f64, dx: f64, dy: f64) {
        self.visual.simulate_event(gpui::ScrollWheelEvent {
            position: Self::at(x, y),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(dx as f32), gpui::px(dy as f32))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        self.settle();
    }

    /// Wheels or swipes of `(dx, dy)` pixels, one after the other with no frame between them (the platform hands
    /// each over as it comes, and a frame is drawn when it is next due), with the pointer at `x`, `y`.
    fn wheels(&mut self, x: f64, y: f64, deltas: &[(f64, f64)]) {
        self.hover(x, y);
        self.visual.update(|window, cx| {
            for (dx, dy) in deltas {
                let event = gpui::ScrollWheelEvent {
                    position: Self::at(x, y),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(*dx as f32), gpui::px(*dy as f32))),
                    modifiers: gpui::Modifiers::none(),
                    touch_phase: gpui::TouchPhase::Moved,
                };
                window.dispatch_event(gpui::PlatformInput::ScrollWheel(event), cx);
            }
        });
        self.settle();
    }

    /// The keyboard goes to something else in the window, as an app-wide shortcut that opens or shows another pane does; the pane
    /// is sent no key. Keep the handle: dropping it gives the keyboard up again.
    fn focus_elsewhere(&mut self) -> gpui::FocusHandle {
        self.visual.update(|window, cx| {
            let other = cx.focus_handle();
            window.focus(&other, cx);
            other
        })
    }

    fn close(mut self) {
        self.visual.update(|window, _| window.remove_window());
    }

    /// The person sets the zoom while the diff is open: the pixel lengths scale and the mod, which reads
    /// `settings()`, renders again. One frame is drawn, none after it, so what follows is what that frame drew.
    fn set_zoom(&mut self, zoom: f32) {
        gpui_shell::set_pixel_scale(zoom);
        self.person.borrow_mut()["zoom"] = json!(zoom);
        self.zoom = zoom;
        self.visual.update(|_, cx| self.runtime.refresh(&self.root, cx).unwrap());
        self.visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// The bar's thumb as the last frame painted it: where it starts and how long it is, in window pixels
    /// (none: no bar). The bar is the only thing painted in the strip it has at the bottom of the window.
    fn thumb(&mut self) -> Option<(f64, f64)> {
        let (strip, scale) = (self.high - 8. * self.zoom - 1., self.visual.update(|window, _| window.scale_factor()));
        let found: Vec<_> = self.visual.update(|window, _| window.painted_quads()).into_iter()
            .filter(|q| !q.background.is_transparent())
            .map(|q| (q.bounds.origin.x.as_f32() / scale, q.bounds.origin.y.as_f32() / scale, q.bounds.size.width.as_f32() / scale))
            .filter(|(_, y, _)| *y >= strip)
            .collect();
        assert!(found.len() <= 1, "more than a thumb in the bar's strip: {found:?}");
        found.first().map(|(x, _, w)| (*x as f64, *w as f64))
    }

    /// Where the first row's left text starts (its first character is `first`), in window pixels, from where the
    /// pointer is over it (none: the pointer is over no text, which has all been scrolled out of sight).
    fn start_of(&mut self, first: &str) -> Option<f64> {
        let (x, y) = (100. * self.zoom as f64, 310. * self.zoom as f64);
        let before = self.over().len();
        self.hover(x, y);
        let (found, from) = self.over().split_off(before).pop()?;
        assert_eq!(found, first);
        Some(x - from)
    }

    /// As [`Self::end_gap`] for the first row's right text, which starts with `first`: from its end to its cell's.
    fn end_gap_right(&mut self, first: &str) -> Option<f64> {
        self.wheel(100., 100., -1e7, 0.);
        let x = self.width("l") + 1. + 100.;
        let before = self.over().len();
        self.hover(x, 310.);
        let (found, from) = self.over().split_off(before).pop()?;
        assert_eq!(found, first);
        Some(self.width("l") + 1. + self.width("r") - ((x - from) + self.text_width(first)))
    }

    /// The lines scrolled as far as they go, and how much room is left after the first row's left text, which
    /// starts with `first`: from its end to its cell's, in window pixels.
    fn end_gap(&mut self, first: &str) -> Option<f64> {
        self.wheel(100., 100., -1e7, 0.);
        Some(self.width("l") - (self.start_of(first)? + self.text_width(first)))
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
    assert!(!shared.borrow().contains_key("yuzora.git.diff.split"), "nothing to drag or reset: {:?}", shared.borrow());
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
    // A click below the rows gives the pane the keys without selecting anything.
    pane.click(TEXT_LEFT, 400., false);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard(), None, "the key does nothing here");
    assert!(pane.noted("toast").is_empty(), "and says nothing");
    assert_eq!(pane.noted("ancestor").len(), 1, "and is left to whatever else wants it");
    pane.click(TEXT_LEFT, row_y(1), false);
    pane.keys("cmd-c");
    assert_eq!(pane.clipboard().as_deref(), Some("\tlet a = 1;"), "with a row selected it copies");
    assert_eq!(pane.noted("ancestor").len(), 1, "and keeps the key to itself");
    pane.set_clipboard("untouched");
    pane.keys("cmd-shift-c");
    pane.keys("cmd-alt-c");
    assert_eq!(pane.clipboard().as_deref(), Some("untouched"), "Cmd+Shift+C and Cmd+Alt+C are other keys");
    assert_eq!(pane.noted("ancestor").len(), 3);
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

/// A changed row of the lines `left` and `right` (none: that side is empty).
fn text_row(left: Option<&str>, right: Option<&str>) -> Value {
    let mut r = json!({ "k": "~", "h": 0 });
    if let Some(t) = left { r["l"] = json!(1); r["li"] = json!(0); r["lt"] = json!(t); }
    if let Some(t) = right { r["r"] = json!(1); r["ri"] = json!(0); r["rt"] = json!(t); }
    r
}

/// The rows of a file that is new: each line on the right.
fn added_file(text: &str) -> Value {
    Value::Array(text.split('\n').enumerate().map(|(i, line)| json!({ "k": "~", "h": 0, "r": i + 1, "ri": i, "rt": line })).collect())
}

/// Where the first row's left text starts, from where the pointer is over it (its first character must be `first`).
fn text_start(pane: &mut DiffPane, first: &str) -> f64 {
    let before = pane.over().len();
    pane.hover(100., 310.);
    let (found, from) = pane.over().split_off(before).pop().expect("over the left text");
    assert_eq!(found, first);
    100. - from
}

// yuzora(v010): a diff with long lines opens and keeps drawing, wide characters or not. Working out how
// wide the lines are used to read every character of a line that had a wide one, in the frame, which
// ran out of its 50 ms past about 400,000 characters; the pane never got past loading, and every frame
// after the first failed. Cases: single lines of 1,000,000 and 4,000,000 characters, one of them
// with only wide ones, many lines with wide characters, and the files of this repository that
// broke it when they were opened as new ones.
#[gpui::test]
fn yuzora_diff_long_lines_with_wide_characters_open_and_keep_drawing(cx: &mut TestAppContext) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut cases: Vec<(String, Value)> = vec![
        ("1,000,000 characters and an \u{e9}".into(), json!([text_row(None, Some(&format!("{}\u{e9}", "a".repeat(1_000_000))))])),
        ("4,000,000 characters and an \u{e9}".into(), json!([text_row(None, Some(&format!("{}\u{e9}", "a".repeat(4_000_000))))])),
        ("4,000,000 wide characters".into(), json!([text_row(Some(&"\u{6f22}".repeat(4_000_000)), None)])),
        ("100 lines of 60,000 wide characters".into(), Value::Array((0..100).map(|_| text_row(Some(&"\u{6f22}".repeat(60_000)), None)).collect())),
        ("100,000 short lines".into(), Value::Array((0..100_000).map(|i| json!({"k": "~", "h": 0, "r": i + 1, "ri": i, "rt": format!("line {i} of text {}", "x".repeat(i % 60))})).collect())),
        ("10,000 lines of a whole file in context".into(), Value::Array((0..10_000).map(|i| json!({"k": "=", "l": i + 1, "r": i + 1, "t": format!("{}{}", "a".repeat(30), "b".repeat(i % 40))})).collect())),
        ("3,000 lines with wide characters".into(), Value::Array((0..3000).map(|i| text_row(Some(&format!("{} \u{6f22}\u{5b57} {i}", "word ".repeat(8 + i % 24))), Some(&format!("{} \u{1f600}", "word ".repeat(10 + i % 20))))).collect())),
    ];
    for file in ["docs/html/v0.0.16-workbench-verification-2026-09-22.html", "patches/@xterm%2Fxterm@6.0.0.patch", "docs/html/assets/herdr-e2e-20260923/native-transcripts.json"] {
        match std::fs::read_to_string(root.join(file)) {
            Ok(text) => cases.push((file.into(), added_file(&text))),
            Err(e) => eprintln!("skipped {file}: {e}"),
        }
    }
    let mut failures = Vec::new();
    for (case, rows) in cases {
        let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
        let shown = |pane: &DiffPane| pane.rendered().iter().filter(|(view, error)| view == "d1" && error.is_null()).count();
        let first = shown(&pane);
        if first == 0 {
            failures.push(format!("{case}: no frame was drawn with the diff in it: {:?}", pane.rendered().last()));
        }
        for again in 1..=2 {
            pane.redraw();
            if shown(&pane) < first + again {
                failures.push(format!("{case}: drawing it again ({again}) did not complete: {:?}", pane.rendered().last()));
            }
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a frame goes through none of the diff's rows: what the pane needs of them all is worked
// out when they arrive, so what a frame costs does not grow with the diff (it has 50 ms). Counted
// (reads of the rows' properties) rather than timed, so a slow machine does not change it.
#[gpui::test]
fn yuzora_diff_a_frame_does_not_go_through_the_diffs_rows(cx: &mut TestAppContext) {
    let side = Value::Array((0..3000).map(|i| text_row(Some(&format!("{} \u{6f22}\u{5b57} {i}", "word ".repeat(8 + i % 24))), Some(&"word ".repeat(10 + i % 20)))).collect());
    let unified = Value::Array((0..3000).map(|i| json!({"k": "+", "h": 0, "r": i + 1, "ri": i, "t": format!("{} \u{6f22}", "word ".repeat(8 + i % 24))})).collect());
    for (case, rows, props) in [("side by side", side, json!({})), ("unified", unified, json!({"layout": "unified"}))] {
        let mut pane = DiffPane::mount_with(cx, rows, 1200., 1200., &Shared::default(), props);
        pane.redraw();
        let (shown, frames) = (pane.reads("shown"), pane.reads("work"));
        let taken = shown.iter().chain(pane.reads("ranked").iter()).sum::<f64>();
        assert!(
            shown.len() == 1 && taken >= 3000.,
            "{case}: taking the diff in, and the slices after it, went through its rows: {shown:?}, {taken}"
        );
        assert!(
            frames.len() >= 2,
            "{case}: a frame was drawn with the diff in it: {frames:?}"
        );
        assert!(
            frames.iter().all(|reads| *reads < 3000.),
            "{case}: a frame went through the rows: {frames:?}"
        );
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

    let (pane, mut menu) = menu_pane(cx, side_diff(), &[(TEXT_RIGHT, 1, false)]);
    menu.click(100., menu_y(0), false);
    assert_eq!(menu.clipboard().as_deref(), Some("\tlet a = 10; // \u{6F22}\u{5B57}"), "a right-click on the right side selects on the right side");
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

/// A diff whose first row holds the longest line, 300 letters, on `side` ("l" or "r"); the other side's is short.
fn long_line_diff(side: &str) -> Diff {
    let (left, right) = if side == "l" {
        ("a".repeat(300), "b".repeat(5))
    } else {
        ("a".repeat(5), "b".repeat(300))
    };
    let rows = json!([
        {"k": "~", "h": 0, "l": 1, "li": 0, "lt": left, "r": 1, "ri": 0, "rt": right},
        {"k": "=", "l": 2, "r": 2, "t": "same"},
        {"k": "~", "h": 0, "l": 3, "li": 2, "lt": "a", "r": 3, "ri": 2, "rt": "b"},
        {"k": "=", "l": 4, "r": 4, "t": "end"},
    ]);
    let mut diff =
        Diff::of(rows).files(&[&left, "same", "a", "end"], &[&right, "same", "b", "end"]);
    diff.hunks = json!([{"index": 0, "a": [0, 1], "b": [0, 1]}]);
    diff
}

/// `rows` unchanged lines on both sides, "<word> 0", "<word> 1", ...
fn numbered_diff(rows: usize, word: &str) -> Diff {
    let texts: Vec<String> = (0..rows).map(|i| format!("{word} {i}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let lines = Value::Array(
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| json!({"k": "=", "l": i + 1, "r": i + 1, "t": t}))
            .collect(),
    );
    Diff::of(lines).files(&refs, &refs)
}

/// What the last `diffText` call was asked for.
fn asked_for(pane: &DiffPane) -> Value {
    pane.noted("text")
        .last()
        .unwrap_or_else(|| panic!("nothing was copied: {:?}", pane.notes.borrow()))[1]
        .clone()
}

/// The text a Cmd+C puts on the clipboard, the clipboard being something else before it.
fn copied(pane: &mut DiffPane) -> Option<String> {
    pane.set_clipboard("untouched");
    pane.keys("cmd-c");
    pane.clipboard().filter(|text| text != "untouched")
}

// yuzora(v010-git-copy): a selection takes no room. The lines are measured in the box `line()` draws them in, and the
// range they scroll is the widest of those, so whatever a row has around that box must not change how far the lines go:
// with rows selected on either side, by click, by Shift, by a right-click, before or after the lines were scrolled, the
// range the mod works out, the room left after the longest line's end (when it is on a selected row) and the width of the
// two sides are what they are with nothing selected, and the end of the line is in view.
#[gpui::test]
fn yuzora_diff_a_selection_takes_no_room_from_the_lines_and_the_range_they_scroll(
    cx: &mut TestAppContext,
) {
    type Do = fn(&mut DiffPane);
    let scenarios: [(&str, Do, bool); 6] = [
        ("nothing selected", |_| {}, false),
        (
            "the long row, on the left",
            |p| p.click(TEXT_LEFT, row_y(0), false),
            true,
        ),
        (
            "the long row to the third, on the left",
            |p| {
                p.click(TEXT_LEFT, row_y(0), false);
                p.click(TEXT_LEFT, row_y(2), true);
            },
            true,
        ),
        (
            "the long row, on the right",
            |p| p.click(TEXT_RIGHT, row_y(0), false),
            true,
        ),
        (
            "a right-click on the long row",
            |p| p.right_press(TEXT_LEFT, row_y(0)),
            true,
        ),
        (
            "the lines scrolled to the end, then the long row",
            |p| {
                p.wheel(100., 100., -1e7, 0.);
                p.click(TEXT_LEFT, row_y(0), false);
            },
            true,
        ),
    ];
    for side in ["l", "r"] {
        let mut found: Vec<(&str, f64, f64, (f64, f64))> = Vec::new();
        for (what, act, selects) in &scenarios {
            let mut pane = copy_pane(cx, long_line_diff(side), json!({}));
            act(&mut pane);
            assert_eq!(
                !pane.noted("shade").is_empty(),
                *selects,
                "{side} / {what}: the row is marked, or not"
            );
            let range = pane
                .ranges()
                .last()
                .copied()
                .expect("a frame reported its range");
            let cells = (pane.width("l"), pane.width("r"));
            let gap = if side == "l" {
                pane.end_gap("a")
            } else {
                pane.end_gap_right("b")
            };
            pane.close();
            found.push((
                *what,
                range,
                gap.unwrap_or_else(|| {
                    panic!("{side} / {what}: the line's end is not under the pointer")
                }),
                cells,
            ));
        }
        let (_, range, gap, cells) = found[0];
        assert!(
            (gap - left_after_the_end()).abs() <= 1.5,
            "{side}: with nothing selected {gap} is left after the line's end"
        );
        for (what, got_range, got_gap, got_cells) in &found[1..] {
            assert!(
                (got_range - range).abs() < 0.01,
                "{side} / {what}: the lines scroll {got_range}, with nothing selected {range}"
            );
            assert!(
                (got_gap - gap).abs() <= 0.5,
                "{side} / {what}: {got_gap} is left after the line's end, with nothing selected {gap}"
            );
            assert!(
                (got_cells.0 - cells.0).abs() < 0.01 && (got_cells.1 - cells.1).abs() < 0.01,
                "{side} / {what}: the sides are {got_cells:?} wide, with nothing selected {cells:?}"
            );
        }
    }
}

// yuzora(v010-git-copy): the divider and the rows beside it do not take each other's presses. Dragging the divider
// across rows (and letting go over one) changes the share and not the selection; a double press puts half back and does
// not either; clicking a row, with Shift, or right-clicking one leaves the share where it was and starts no drag; a
// press in the divider's grab area drags and selects nothing, a press just outside it selects and drags nothing.
#[gpui::test]
fn yuzora_diff_dragging_the_divider_and_selecting_rows_leave_each_other_as_they_are(
    cx: &mut TestAppContext,
) {
    const PANE: f64 = 1200.;
    let shared = Shared::default();
    let mut pane = DiffPane::mount_diff(
        cx,
        side_diff(),
        PANE as f32,
        PANE as f32,
        &shared,
        json!({"real": true}),
    );
    let half = (PANE - 1.) / 2.;
    let rows_2_and_3 = "\tlet a = 1;\n    let b = 2;  ";
    pane.click(TEXT_LEFT, row_y(1), false);
    pane.click(TEXT_LEFT, row_y(2), true);
    assert_eq!(copied(&mut pane).as_deref(), Some(rows_2_and_3));
    let runs = asked_for(&pane);

    let at = pane.width("l") + 0.5;
    pane.press(at, row_y(1), 1);
    pane.drag(at + 100., row_y(2));
    pane.drag(at + 200., row_y(3));
    near(
        pane.width("l"),
        half + 200.,
        "dragged right by 200, across rows",
    );
    pane.release(at + 200., row_y(4));
    assert!(
        kept(&shared).as_f64().is_some(),
        "the share is kept: {:?}",
        kept(&shared)
    );
    assert!(pane.noted("pick").is_empty(), "no line was picked by it");
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some(rows_2_and_3),
        "the selection is the same after the drag"
    );
    assert_eq!(asked_for(&pane), runs);

    // Clicks, Shift and a right-click leave the share and the divider where they are.
    let (share, left) = (kept(&shared), pane.width("l"));
    let beside_left = left - 10.;
    let beside_right = left + 1. + 61.;
    pane.click(beside_left, row_y(1), false);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("\tlet a = 1;"),
        "just left of the divider's grab area, a click selects the row"
    );
    pane.drag(left + 150., row_y(1));
    pane.click(beside_right, row_y(1), false);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("\tlet a = 10; // \u{6F22}\u{5B57}"),
        "and just right of it, on the right side"
    );
    pane.click(beside_right, row_y(4), true);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("\tlet a = 10; // \u{6F22}\u{5B57}\n}"),
        "Shift extends it"
    );
    pane.right_press(beside_right, row_y(1));
    assert_eq!(
        pane.noted("overlay").len(),
        1,
        "a right-click opens its menu"
    );
    near(
        pane.width("l"),
        left,
        "the divider is where it was after the clicks",
    );
    assert_eq!(kept(&shared), share, "and the share kept is the same");

    // The grab area takes the press, and the rows under it are not clicked.
    pane.click(beside_left, row_y(2), false);
    assert_eq!(copied(&mut pane).as_deref(), Some("    let b = 2;  "));
    for grab in [left - 1., left + 0.5, left + 2.] {
        pane.press(grab, row_y(4), 1);
        pane.release(grab, row_y(4));
        assert_eq!(
            copied(&mut pane).as_deref(),
            Some("    let b = 2;  "),
            "a press at {grab} is the divider's: nothing is selected by it"
        );
    }
    pane.press(left + 0.5, row_y(4), 2);
    near(pane.width("l"), half, "a double press puts half back");
    assert_eq!(kept(&shared), Value::Null);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("    let b = 2;  "),
        "and leaves the selection"
    );
    pane.close();
}

// yuzora(v010-git-copy): sideways scrolling and the selection do not touch each other: a wheel and the dragged thumb
// leave the selected rows as they are, and selecting a row, with Shift or not, leaves the lines where they were
// scrolled to and the thumb where it was.
#[gpui::test]
fn yuzora_diff_scrolling_sideways_and_selecting_rows_leave_each_other_as_they_are(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, long_line_diff("l"), json!({}));
    let long = "a".repeat(300);
    let home = left_text_start(&mut pane);
    pane.click(TEXT_LEFT, row_y(0), false);
    assert_eq!(copied(&mut pane).as_deref(), Some(long.as_str()));
    near(
        left_text_start(&mut pane),
        home,
        "selecting a row does not scroll the lines",
    );

    pane.wheel(100., 100., -200., 0.);
    near(
        left_text_start(&mut pane),
        home - 200.,
        "a wheel moves the lines",
    );
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some(long.as_str()),
        "and the selection is the same"
    );

    let bar = pane.high as f64 - 4.;
    let (from, length) = pane.thumb().expect("a bar");
    let grab = from + length / 2.;
    pane.press(grab, bar, 1);
    pane.drag(grab + 100., bar);
    pane.release(grab + 100., bar);
    let moved = left_text_start(&mut pane);
    assert!(
        moved < home - 200. - 10.,
        "the thumb moved the lines farther: {moved} from {home}"
    );
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some(long.as_str()),
        "the selection is the same after the thumb was dragged"
    );

    let thumb = pane.thumb().unwrap();
    pane.click(TEXT_LEFT, row_y(2), false);
    assert_eq!(copied(&mut pane).as_deref(), Some("a"));
    pane.click(TEXT_LEFT, row_y(3), true);
    assert_eq!(copied(&mut pane).as_deref(), Some("a\nend"));
    near(
        left_text_start(&mut pane),
        moved,
        "selecting rows leaves the lines where the thumb put them",
    );
    let after = pane.thumb().unwrap();
    assert!(
        (after.0 - thumb.0).abs() <= 1. && (after.1 - thumb.1).abs() <= 1.,
        "and the thumb: {thumb:?} then {after:?}"
    );
    pane.close();
}

// yuzora(v010-git-copy): the selection is not touched by the ranking of the rows that goes on in frames, in slices, and
// goes with the rows it named when the diff is read again while they are still being ranked. `hold` stops the ranking
// after its first slice, so the rows are on show and not all ranked; F12 goes on with it and F11 reads the diff again.
#[gpui::test]
fn yuzora_diff_a_selection_made_while_the_rows_are_ranked_stays_through_it_and_goes_when_the_diff_is_read_again(
    cx: &mut TestAppContext,
) {
    const ROWS: usize = 20_000;
    let finish = |pane: &mut DiffPane| {
        pane.keys("f12");
        for _ in 0..30 {
            if !pane.pending() {
                break;
            }
            pane.settle();
        }
        assert!(
            !pane.pending(),
            "the ranking and the laying out were not finished"
        );
    };
    let ranked_some = |pane: &mut DiffPane| {
        pane.settle_until_ranked_some();
        let done = pane.rows_ranked_in_all();
        assert!(
            pane.pending() && done > 0. && done < ROWS as f64,
            "{done} of {ROWS} rows are ranked, and the ranking is held"
        );
    };

    let mut pane = DiffPane::mount_diff(
        cx,
        numbered_diff(ROWS, "line"),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true, "hold": true}),
    );
    ranked_some(&mut pane);
    pane.click_arrives(TEXT_LEFT, row_y(3), false);
    pane.click_arrives(TEXT_LEFT, row_y(5), true);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("line 3\nline 4\nline 5"),
        "selected while the rows are being ranked"
    );
    ranked_some(&mut pane);
    finish(&mut pane);
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("line 3\nline 4\nline 5"),
        "the ranking going on, then done, leaves it"
    );
    assert_eq!(asked_for(&pane), json!([["l", 3, 6]]));
    pane.close();

    // Read again while it is held: the rows are others, and Shift reaches from nothing.
    let mut pane = DiffPane::mount_diff(
        cx,
        numbered_diff(ROWS, "line"),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true, "hold": true}),
    );
    let other = numbered_diff(ROWS, "other");
    pane.click_arrives(TEXT_LEFT, row_y(3), false);
    pane.click_arrives(TEXT_LEFT, row_y(5), true);
    assert_eq!(copied(&mut pane).as_deref(), Some("line 3\nline 4\nline 5"));
    pane.answer_next(vec![json!({"id": "d1", "rows": other.rows, "hunks": [], "stageable": true, "add": 0, "del": 0, "leftExists": true, "rightExists": true})]);
    pane.keys("f11");
    ranked_some(&mut pane);
    assert_eq!(
        copied(&mut pane).as_deref(),
        None,
        "the diff read again has nothing selected"
    );
    pane.click_arrives(TEXT_LEFT, row_y(7), true);
    copied(&mut pane);
    assert_eq!(
        asked_for(&pane),
        json!([["l", 7, 8]]),
        "Shift begins at the row, not at the one selected in the diff before"
    );
    finish(&mut pane);
    copied(&mut pane);
    assert_eq!(
        asked_for(&pane),
        json!([["l", 7, 8]]),
        "and it stays when the ranking of the new diff is done"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu): a first slice the engine cuts off, with the ranking held after it, still counts the rows it went
// through. The engine's clock is the machine's own, so on a slow one the first slice can be cut a few rows in (a `stall_slice`
// prop cuts it at 3,000 units, the same on any machine): nothing says it is back, and the held ranking does not begin another,
// so the rows it did were in no note and the ranking looked as if it had not begun.
#[gpui::test]
fn yuzora_diff_a_first_slice_the_engine_cut_off_counts_the_rows_it_went_through_while_the_ranking_is_held(
    cx: &mut TestAppContext,
) {
    const ROWS: usize = 20_000;
    let mut pane = DiffPane::mount_diff(
        cx,
        numbered_diff(ROWS, "line"),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true, "hold": true, "stall_slice": 3_000}),
    );
    pane.settle_until_ranked_some();
    let done = pane.rows_ranked_in_all();
    let cut = pane.cut("slice", "ranked");
    pane.close();
    assert!((1..=2).contains(&cut), "slices the engine cut off: one by the test, and one more if this machine is slow enough to cut another");
    assert!(
        done > 0. && done < 1_000.,
        "{done} of {ROWS} rows are ranked: the slice cut at 3,000 units went through a few"
    );
}

// yuzora(v010-git-copy): at any zoom a click and a right-click select on the side the pointer is on, and Shift reaches
// to a row on the other side without changing the side. The pointer comes back in window pixels and the sides are
// worked out in the unit the pane writes lengths in, so a point on the left side at 250 would be on the right at zoom 2
// (500 of 299.5), and one on the right side at 900 on the left at zoom 0.8 (720 of 749.5), if it were not brought in.
#[gpui::test]
fn yuzora_diff_a_click_and_a_right_click_select_on_the_side_they_are_on_at_every_zoom(
    cx: &mut TestAppContext,
) {
    if !alone("yuzora_diff_a_click_and_a_right_click_select_on_the_side_they_are_on_at_every_zoom")
    {
        return;
    }
    let rows = json!([
        {"k": "~", "h": 0, "l": 1, "li": 0, "lt": "a".repeat(300), "r": 1, "ri": 0, "rt": "b".repeat(300)},
        {"k": "~", "h": 0, "l": 2, "li": 1, "lt": "a".repeat(300), "r": 2, "ri": 1, "rt": "b".repeat(300)},
    ]);
    let mut failures = Vec::new();
    for zoom in ZOOMS {
        let z = zoom as f64;
        let mut pane = DiffPane::zoomed(cx, rows.clone(), zoom, json!({"real": true}));
        let (left, right) = (250. * z, pane.width("l") + z + 100. * z);
        let marked_side = |pane: &DiffPane| {
            pane.noted("shade")
                .last()
                .and_then(|n| n[1].as_str().map(str::to_owned))
        };
        let menu_runs = |pane: &DiffPane| {
            pane.noted("overlay")
                .last()
                .map(|n| n[1]["props"]["menu"]["runs"].clone())
        };
        pane.click(left, 38. * z, false);
        if marked_side(&pane).as_deref() != Some("l") {
            failures.push(format!(
                "zoom {zoom}: a click at {left} marked {:?}, wanted the left side",
                marked_side(&pane)
            ));
        }
        pane.click(right, 38. * z, false);
        if marked_side(&pane).as_deref() != Some("r") {
            failures.push(format!(
                "zoom {zoom}: a click at {right} marked {:?}, wanted the right side",
                marked_side(&pane)
            ));
        }
        pane.right_press(left, 58. * z);
        if menu_runs(&pane) != Some(json!([["l", 1, 2]])) {
            failures.push(format!(
                "zoom {zoom}: a right-click at {left} offered {:?}, wanted row 2 of the left side",
                menu_runs(&pane)
            ));
        }
        pane.right_press(right, 38. * z);
        if menu_runs(&pane) != Some(json!([["r", 0, 1]])) {
            failures.push(format!("zoom {zoom}: a right-click at {right} offered {:?}, wanted row 1 of the right side", menu_runs(&pane)));
        }
        pane.click(left, 38. * z, false);
        pane.click(right, 58. * z, true);
        pane.right_press(left, 58. * z);
        if menu_runs(&pane) != Some(json!([["l", 0, 2]])) {
            failures.push(format!("zoom {zoom}: Shift from the left side to a row on the right offered {:?}, wanted both rows of the left side", menu_runs(&pane)));
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010-git-diff-fu): in unified a selection takes no room either. The rows scroll in a box whose width is the longest
// line's, and a tint over a selected row must not move the text in it (then the line's end would be out of reach) nor widen the
// row or the box. Measured as the window paints it, with the lines scrolled as far as they go: the rows' backgrounds say how far
// that is and how wide the box, and a highlighted word that ends the long lines says where they end: with nothing selected,
// with a row, with rows to a Shift-click, with a right-click, and with the lines scrolled again.
#[gpui::test]
fn yuzora_diff_in_unified_a_selection_takes_no_room_from_how_far_the_lines_scroll(
    cx: &mut TestAppContext,
) {
    let long = format!("{}END", "w".repeat(297));
    let ending = json!([{"t": "w".repeat(297), "h": false}, {"t": "END", "h": true}]);
    let rows = json!([
        {"k": "-", "h": 0, "l": 1, "li": 0, "t": long, "s": ending},
        {"k": "=", "l": 2, "r": 1, "t": "same"},
        {"k": "+", "h": 0, "r": 2, "ri": 1, "t": "new"},
        {"k": "-", "h": 0, "l": 3, "li": 2, "t": "old"},
        {"k": "+", "h": 0, "r": 3, "ri": 2, "t": long, "s": ending},
    ]);
    let mut diff = Diff::of(rows).files(&[&long, "same", "old"], &["same", "new", &long]);
    diff.hunks = json!([{"index": 0, "a": [0, 1], "b": [1, 2]}]);
    let mut pane = copy_pane(cx, diff, json!({"layout": "unified"}));
    pane.wheel(300., 200., -1e7, 0.);
    let (scrolled, wide, tints, gap) = pane.unified_extent();
    assert!(
        scrolled > 1000. && wide > 2000.,
        "the long lines scroll: {scrolled} of a box {wide} wide"
    );
    assert!(
        gap.is_finite() && gap > 0. && gap < 120.,
        "the end of the long lines is in view, {gap}px from the edge"
    );
    assert_eq!(tints, 0, "nothing is selected, nothing is tinted");
    let mut failures = Vec::new();
    type Do = fn(&mut DiffPane);
    // (what is done, the rows tinted after it)
    let scenarios: [(&str, Do, usize); 4] = [
        ("a row selected", |p| p.click(300., row_y(0), false), 1),
        (
            "rows to a Shift-click",
            |p| p.click(300., row_y(4), true),
            5,
        ),
        ("a right-click", |p| p.right_press(300., row_y(2)), 5),
        (
            "the lines scrolled again",
            |p| p.wheel(300., 200., -1e7, 0.),
            5,
        ),
    ];
    for (what, act, marked) in scenarios {
        act(&mut pane);
        let (now, width, tints, room) = pane.unified_extent();
        if (now - scrolled).abs() > 0.5 || (width - wide).abs() > 0.5 {
            failures.push(format!("{what}: scrolled {now} of a box {width} wide, and with nothing selected {scrolled} of {wide}"));
        }
        if (room - gap).abs() > 0.5 {
            failures.push(format!("{what}: {room}px left after the end of the long lines, and with nothing selected {gap}px"));
        }
        if tints != marked {
            failures.push(format!("{what}: {tints} rows tinted, wanted {marked}: this measured a selection that was not there"));
        }
    }
    pane.close();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The selection the last frame drew, as `[side, anchor, head]`, or null. Nothing is pressed to ask: a key ends a drag.
fn mark_of(pane: &DiffPane) -> Value {
    pane.noted("mark")
        .last()
        .map_or(Value::Null, |n| n[1].clone())
}

/// How far the list was scrolled when the last frame was built (a frame behind a wheel or a tick, and no key to ask for it).
fn scrolled_in_frame(pane: &DiffPane) -> f64 {
    pane.noted("now")
        .last()
        .and_then(|n| n[1].as_f64())
        .expect("no frame has said how far the list is scrolled")
}

/// The runs a Cmd+C copies now (none: nothing is selected).
fn selected(pane: &mut DiffPane) -> Option<Value> {
    copied(pane)?;
    Some(asked_for(pane))
}

// yuzora(v010-git-diff-fu): holding the button down and moving across rows selects them: the selection runs from the row pressed
// to the row under the pointer, as it does by a click and a Shift-click, on the side that was pressed, and it is there while the
// button is down as well as after it is let go. Up the list as well as down it; unified copies what it shows; a Shift press
// reaches from the selection there is; the pointer coming back to the row it left from is that one row.
#[gpui::test]
fn yuzora_diff_dragging_across_rows_selects_from_the_row_pressed_to_the_row_under_the_pointer(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    pane.press(TEXT_LEFT, row_y(1), 1);
    pane.drag(TEXT_LEFT, row_y(2));
    assert_eq!(
        mark_of(&pane),
        json!(["left", 1, 2]),
        "the left side's rows 2 and 3, with the button down"
    );
    pane.release(TEXT_LEFT, row_y(2));
    pane.hover(TEXT_LEFT, row_y(4));
    assert_eq!(
        mark_of(&pane),
        json!(["left", 1, 2]),
        "and the pointer moving on with the button up selects no more"
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 3]])),
        "and it is there when it is let go"
    );
    assert_eq!(
        pane.clipboard().as_deref(),
        Some("\tlet a = 1;\n    let b = 2;  ")
    );
    // Up, on the right: the row with no line there and the fold add none. The pointer on the left side does not change the side.
    pane.press(TEXT_RIGHT, row_y(4), 1);
    pane.drag(TEXT_LEFT, row_y(2));
    pane.drag(TEXT_LEFT, row_y(1));
    pane.release(TEXT_LEFT, row_y(1));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["r", 1, 2], ["r", 5, 6]])),
        "the right side, from the last row up to the second"
    );
    // Back to where it began: that row.
    pane.press(TEXT_LEFT, row_y(1), 1);
    pane.drag(TEXT_LEFT, row_y(2));
    pane.drag(TEXT_LEFT, row_y(1));
    pane.release(TEXT_LEFT, row_y(1));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 2]])),
        "the pointer back on the row it began on: that row"
    );
    // Shift reaches from the row the selection began on, and the drag goes on moving the end.
    pane.click(TEXT_LEFT, row_y(1), false);
    pane.press_with(TEXT_LEFT, row_y(2), true);
    pane.drag(TEXT_LEFT, row_y(4));
    pane.release(TEXT_LEFT, row_y(4));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 3], ["l", 6, 7]])),
        "from row 2 to the last, the fold left out"
    );
    pane.close();

    let mut pane = copy_pane(cx, unified_diff(), json!({"layout": "unified"}));
    pane.press(300., row_y(0), 1);
    pane.drag(300., row_y(3));
    pane.drag(300., row_y(5));
    pane.drag(300., row_y(12));
    assert_eq!(
        mark_of(&pane),
        json!(["unified", 0, 5]),
        "below the last row: the last row"
    );
    pane.release(300., row_y(12));
    assert_eq!(pane.noted("text").len(), 0, "nothing was copied yet");
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("keep\n\told one\nold two\nnew one\ntail"),
        "unified copies the lines as they are shown"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu): a press that does not move, or moves inside its row, is the click it was: the row, and its hunk made
// the current one. The numbers of a diff that can stage still pick the line, and a press on them dragged across rows selects
// nothing and picks nothing; the same for a fold, the divider and the bar under the lines, which have their own drags.
#[gpui::test]
fn yuzora_diff_a_press_that_stays_in_its_row_is_a_click_and_the_numbers_the_fold_and_the_divider_start_no_selection(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, side_diff(), json!({}));
    pane.press(TEXT_LEFT, row_y(1), 1);
    pane.drag(TEXT_LEFT + 30., row_y(1) + 4.);
    pane.release(TEXT_LEFT + 30., row_y(1) + 4.);
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 2]])),
        "moved inside the row: a click on it"
    );
    // The numbers: the line is picked by a click (a stageable diff), and a drag from them picks and selects nothing.
    pane.click(TEXT_LEFT, row_y(0), false);
    pane.press(10., row_y(1), 1);
    pane.drag(10., row_y(2));
    pane.release(10., row_y(2));
    assert_eq!(
        pane.noted("pick").len(),
        0,
        "a drag across rows picks no line"
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 0, 1]])),
        "and leaves the selection there was"
    );
    // A fold: its click opens the whole file, a drag from it does nothing.
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(4));
    pane.release(TEXT_LEFT, row_y(4));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 0, 1]])),
        "a drag from a fold selects nothing"
    );
    // The divider drags the divider.
    let divider = 599.5 + 0.5;
    pane.press(divider, row_y(1), 1);
    pane.drag(divider + 40., row_y(2));
    pane.drag(divider + 40., row_y(4));
    pane.release(divider + 40., row_y(4));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 0, 1]])),
        "a drag from the divider selects nothing"
    );
    pane.close();

    let mut diff = side_diff();
    diff.stageable = false;
    let mut pane = copy_pane(cx, diff, json!({}));
    pane.press(10., row_y(1), 1);
    pane.drag(10., row_y(2));
    pane.release(10., row_y(2));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 3]])),
        "where nothing can be picked the numbers are text to select from"
    );
    pane.close();

    // The scrollbar beside the list takes the press (and the drag of its thumb): no selection begins.
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    pane.wheel(300., 300., 0., -40.);
    pane.press(1196., row_y(1), 1);
    pane.drag(1196., row_y(4));
    pane.drag(1196., row_y(6));
    pane.release(1196., row_y(6));
    assert_eq!(
        selected(&mut pane),
        None,
        "a drag from the scrollbar selects nothing"
    );
    pane.close();

    // The bar under long lines drags the lines.
    let mut pane = copy_pane(cx, long_line_diff("l"), json!({}));
    let bar = pane.high as f64 - 4.;
    let (start, long) = pane.thumb().expect("a bar");
    pane.press(start + long / 2., bar, 1);
    pane.drag(start + long / 2. + 100., row_y(1));
    pane.drag(start + long / 2. + 100., row_y(2));
    pane.release(start + long / 2. + 100., row_y(2));
    assert_eq!(
        selected(&mut pane),
        None,
        "a drag from the bar's thumb selects nothing"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu): the row under the pointer is the row the list shows there, whatever the zoom and wherever the list is
// scrolled to, a whole number of rows or not: found out by clicking, which is the list's own answer, and dragging from and to
// the same places selects the rows between (with 37 script units scrolled off the top, the pointer 55 down the list is over
// row 4, where counting from the first row drawn, row 1, gives 3).
#[gpui::test]
fn yuzora_diff_dragging_selects_the_rows_under_the_pointer_at_every_zoom_with_the_list_scrolled(
    cx: &mut TestAppContext,
) {
    if !alone("yuzora_diff_dragging_selects_the_rows_under_the_pointer_at_every_zoom_with_the_list_scrolled") {
        return;
    }
    let mut failures = Vec::new();
    for zoom in [0.8_f32, 1., 1.5, 2.] {
        let z = zoom as f64;
        gpui_shell::set_pixel_scale(zoom);
        let mut pane = DiffPane::mount_at(
            cx,
            numbered_diff(200, "line"),
            1200.,
            (1200., 500.),
            zoom,
            &Shared::default(),
            json!({"real": true}),
        );
        for scrolled in [0_f64, 37.] {
            if scrolled > 0. {
                pane.wheel(300. * z, 150. * z, 0., -37. * z);
            }
            let row_at = |pane: &mut DiffPane, down: f64| {
                pane.click_arrives(TEXT_LEFT * z, (COPY_HEADER + down) * z, false);
                match selected(pane) {
                    Some(runs) => runs[0][1].as_u64().unwrap(),
                    None => u64::MAX,
                }
            };
            let (from, to) = (row_at(&mut pane, 55.), row_at(&mut pane, 150.));
            pane.press(TEXT_LEFT * z, (COPY_HEADER + 55.) * z, 1);
            pane.drag(TEXT_LEFT * z, (COPY_HEADER + 100.) * z);
            pane.drag(TEXT_LEFT * z, (COPY_HEADER + 150.) * z);
            pane.release(TEXT_LEFT * z, (COPY_HEADER + 150.) * z);
            let got = selected(&mut pane);
            let wanted = json!([["l", from, to + 1]]);
            if got.as_ref() != Some(&wanted) {
                failures.push(format!("zoom {zoom}, {scrolled} scrolled: dragged from the row {from} to the row {to} and selected {got:?}"));
            }
            let first = ((scrolled + 55.) / 20.).floor() as u64;
            if from != first {
                failures.push(format!("zoom {zoom}, {scrolled} scrolled: the list says row {from} is 55 down, and 55 + {scrolled} over 20 is {first}"));
            }
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010-git-diff-fu): the list reports how far it is scrolled in window pixels: 0 at the top, the wheel's distance
// down it, at any zoom (a length the pane writes at zoom 1.5 is 1.5 times as many pixels, and the wheel is in pixels).
#[gpui::test]
fn yuzora_diff_the_list_says_how_far_it_is_scrolled_in_window_pixels(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_the_list_says_how_far_it_is_scrolled_in_window_pixels") {
        return;
    }
    for zoom in [1_f32, 1.5] {
        let z = zoom as f64;
        gpui_shell::set_pixel_scale(zoom);
        let mut pane = DiffPane::mount_at(
            cx,
            numbered_diff(200, "line"),
            1200.,
            (1200., 500.),
            zoom,
            &Shared::default(),
            json!({"real": true}),
        );
        let says = |pane: &mut DiffPane| {
            pane.scrolled().unwrap_or_else(|| {
                panic!("zoom {zoom}: the list cannot say how far it is scrolled")
            })
        };
        // The key that asks needs the focus, which a press in the header gives (the rows are not touched).
        pane.press(600., COPY_HEADER / 2., 1);
        pane.release(600., COPY_HEADER / 2.);
        near(says(&mut pane), 0., "at the top");
        pane.wheel(300. * z, 150. * z, 0., -37.);
        near(
            says(&mut pane),
            37.,
            &format!("zoom {zoom}: 37 pixels down"),
        );
        pane.wheel(300. * z, 150. * z, 0., 10.);
        near(says(&mut pane), 27., &format!("zoom {zoom}: 10 back up"));
        pane.wheel(300. * z, 150. * z, 0., 1e6);
        near(says(&mut pane), 0., &format!("zoom {zoom}: past the top"));
        pane.wheel(300. * z, 150. * z, 0., -1e6);
        let end = says(&mut pane);
        assert!(
            end > 100. && end < 200. * 20. * z,
            "zoom {zoom}: past the end it stops, at {end}"
        );
        pane.close();
    }
}

// yuzora(v010-git-diff-fu): a pointer held at the list's edge scrolls it, and the selection goes on: down the rows the list
// brings in, to the last, and back up. Above the list (the header) scrolls up. The pointer is not moved, so it is the clock
// that does it, and it stops with the button: no frame is built once it is let go.
#[gpui::test]
fn yuzora_diff_a_pointer_held_at_the_edge_scrolls_the_list_and_the_selection_goes_on(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let (bottom, top) = (pane.high as f64 - 8., COPY_HEADER + 6.);
    pane.press(TEXT_LEFT, row_y(5), 1);
    pane.drag(TEXT_LEFT, row_y(8));
    near(
        scrolled_in_frame(&pane),
        0.,
        "not at an edge: the list stays",
    );
    pane.drag(TEXT_LEFT, bottom);
    pane.wait(10, 50);
    let (down, mark) = (scrolled_in_frame(&pane), mark_of(&pane));
    assert!(
        down >= 100.,
        "10 ticks at the bottom edge scroll the list by rows: {down}"
    );
    assert!(
        mark[2].as_u64().unwrap_or(0) > 30,
        "the selection reaches rows that were not on screen: {mark}"
    );
    pane.wait(300, 50);
    assert_eq!(
        mark_of(&pane),
        json!(["left", 5, 199]),
        "held at the bottom: to the last row, and the list stops at its end"
    );
    let end_of_list = scrolled_in_frame(&pane);
    pane.drag(TEXT_LEFT, 10.);
    pane.wait(8, 50);
    assert!(
        end_of_list - scrolled_in_frame(&pane) >= 200.,
        "the further out the pointer is, the faster the list goes: in the header it moves two rows a tick, at the edge one"
    );
    pane.drag(TEXT_LEFT, top);
    pane.wait(300, 50);
    assert!(
        scrolled_in_frame(&pane) < end_of_list - 100.,
        "held at the top edge: the list goes back up"
    );
    assert_eq!(
        mark_of(&pane),
        json!(["left", 5, 0]),
        "to the first row, from the row pressed"
    );
    pane.drag(TEXT_LEFT, 10.);
    pane.wait(5, 50);
    assert_eq!(
        mark_of(&pane),
        json!(["left", 5, 0]),
        "above the list: the first row"
    );
    pane.release(TEXT_LEFT, 10.);
    let after = pane.script_renders();
    pane.wait(40, 50);
    assert_eq!(
        pane.script_renders(),
        after,
        "the clock builds nothing once the button is up"
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 0, 6]])),
        "and the selection stays"
    );
    pane.close();
}

/// `rows` unchanged lines, the first one `300` letters long: side by side the lines scroll sideways, and the bar under them has a track.
fn long_first_line_diff(rows: usize) -> Diff {
    let mut diff = numbered_diff(rows, "line");
    let long = "a".repeat(300);
    diff.rows[0]["t"] = json!(long);
    diff.left[0] = long.clone();
    diff.right[0] = long;
    diff
}

/// The button goes down at `x`, `y` (Shift held, or not), stays there for `ticks` of 50 ms with the pointer not moving, and comes up.
fn hold_still(pane: &mut DiffPane, x: f64, y: f64, shift: bool, ticks: usize) {
    pane.hover(x, y);
    pane.press_with(x, y, shift);
    pane.wait(ticks, 50);
    pane.release(x, y);
}

// yuzora(v010-git-diff-fu-r1): a press that stays where it landed is the click it was, however long the button is held. Not
// scrolled under it at the list's edge either, where the row the release lands on would be another than the one pressed and the
// list would report no click.
#[gpui::test]
fn yuzora_diff_a_button_held_still_on_the_last_row_in_view_is_a_click(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    let last = ((bottom - COPY_HEADER) / ROW_H) as u64;
    hold_still(&mut pane, TEXT_LEFT, bottom, false, 4);
    near(scrolled_in_frame(&pane), 0., "held 200 ms: the list stays");
    assert_eq!(
        pane.noted("clickRow"),
        vec![json!(["clickRow", last])],
        "the list reported the click"
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", last, last + 1]])),
        "the row pressed, and no other"
    );
    pane.close();
}

#[gpui::test]
fn yuzora_diff_a_shift_click_held_still_on_the_last_row_in_view_reaches_that_row_and_no_further(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    let last = ((bottom - COPY_HEADER) / ROW_H) as u64;
    pane.click(TEXT_LEFT, row_y(3), false);
    hold_still(&mut pane, TEXT_LEFT, bottom, true, 4);
    near(scrolled_in_frame(&pane), 0., "held 200 ms: the list stays");
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 3, last + 1]])),
        "from the row selected to the one clicked, and not a row further"
    );
    pane.close();
}

#[gpui::test]
fn yuzora_diff_a_button_held_still_on_the_first_row_in_view_of_a_scrolled_list_is_a_click(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    pane.wheel(300., 300., 0., -100.);
    let top = COPY_HEADER + 6.;
    let first = ((100. + 6.) / ROW_H) as u64;
    hold_still(&mut pane, TEXT_LEFT, top, false, 4);
    near(
        scrolled_in_frame(&pane),
        100.,
        "held 200 ms: the list stays where the wheel left it",
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", first, first + 1]])),
        "the row pressed"
    );
    pane.close();
}

#[gpui::test]
fn yuzora_diff_a_button_held_on_the_bars_track_selects_nothing_and_scrolls_nothing(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, long_first_line_diff(200), json!({}));
    let (start, long) = pane.thumb().expect("a bar under the long line");
    let (track, bar) = (start + long + 100., pane.high as f64 - 4.);
    assert!(
        track < 1200.,
        "a track beside the thumb: {start}..{}",
        start + long
    );
    hold_still(&mut pane, track, bar, false, 6);
    near(scrolled_in_frame(&pane), 0., "the list stays");
    assert_eq!(mark_of(&pane), Value::Null, "and no row is selected");
    assert_eq!(selected(&mut pane), None);
    pane.hover(track, bar);
    pane.press(track, bar, 1);
    pane.drag(track, bar - 200.);
    pane.release(track, bar - 200.);
    assert_eq!(
        selected(&mut pane),
        None,
        "dragged from the track up the rows: nothing is selected"
    );
    pane.close();
}

#[gpui::test]
fn yuzora_diff_unified_a_button_held_still_at_the_bottom_where_the_bar_is_scrolls_nothing(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, long_first_line_diff(200), json!({"layout": "unified"}));
    let bar = pane.high as f64 - 4.;
    hold_still(&mut pane, 600., bar, false, 6);
    near(scrolled_in_frame(&pane), 0., "the list stays");
    assert_eq!(
        mark_of(&pane),
        json!(["unified", 28, 28]),
        "and the row behind the bar is the one clicked"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): a drag begins when the pointer has gone a few pixels from where it landed or is over another row,
// and not before: a shake of the hand at the list's edge is still a click, and a pointer that has gone further and is held there
// scrolls the list.
#[gpui::test]
fn yuzora_diff_a_drag_begins_a_few_pixels_from_where_the_button_went_down(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    let last = ((bottom - COPY_HEADER) / ROW_H) as u64;
    pane.hover(TEXT_LEFT, bottom);
    pane.press(TEXT_LEFT, bottom, 1);
    pane.drag(TEXT_LEFT + 2., bottom);
    pane.wait(4, 50);
    pane.drag(TEXT_LEFT, bottom);
    pane.release(TEXT_LEFT, bottom);
    near(
        scrolled_in_frame(&pane),
        0.,
        "a shake of 2 px: the list stays",
    );
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", last, last + 1]])),
        "and it was a click"
    );

    pane.hover(TEXT_LEFT, bottom);
    pane.press(TEXT_LEFT, bottom, 1);
    pane.drag(TEXT_LEFT + 12., bottom);
    pane.wait(4, 50);
    assert!(
        scrolled_in_frame(&pane) >= 20.,
        "12 px away and held at the edge: the list scrolls"
    );
    pane.release(TEXT_LEFT + 12., bottom);
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): a pointer that goes over the next row is a drag however near it went: the list would report no
// click for a release on another row than the one pressed.
#[gpui::test]
fn yuzora_diff_a_drag_begins_when_the_pointer_is_over_another_row_however_near(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let (edge_of_row, next_row) = (row_y(1) + 9., row_y(1) + 11.);
    pane.hover(TEXT_LEFT, edge_of_row);
    pane.press(TEXT_LEFT, edge_of_row, 1);
    pane.drag(TEXT_LEFT, next_row);
    pane.release(TEXT_LEFT, next_row);
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 1, 3]])),
        "2 px away, over the next row: the two rows"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): with long lines the bar takes the bottom of the list's box: a pointer held at the bottom edge scrolls
// the list to the end of its rows, the last of which is then whole above the bar.
#[gpui::test]
fn yuzora_diff_a_pointer_held_at_the_bottom_edge_of_a_list_with_a_bar_under_it_scrolls_to_the_last_row(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, long_first_line_diff(200), json!({}));
    let list = pane.high as f64 - COPY_HEADER - 8.;
    pane.press(TEXT_LEFT, row_y(5), 1);
    pane.drag(TEXT_LEFT, row_y(8));
    pane.drag(TEXT_LEFT, COPY_HEADER + list - 18.);
    pane.wait(10, 50);
    assert!(
        scrolled_in_frame(&pane) >= 100.,
        "18 px above the list's end is in its last 20, the bar not counted"
    );
    pane.drag(TEXT_LEFT, COPY_HEADER + list - 6.);
    pane.wait(300, 50);
    near(
        scrolled_in_frame(&pane),
        200. * ROW_H - list,
        "to the end of the rows",
    );
    assert_eq!(mark_of(&pane), json!(["left", 5, 199]));
    pane.release(TEXT_LEFT, COPY_HEADER + list - 6.);
    pane.close();
}
// yuzora(v010-git-diff-fu): a drag while the rows are being ranked is not cut short by it, and does not stop it: the selection made
// is there when the ranking is done, and the ranking goes on to its end whether the button is up or down (a `hold` prop stops
// it after its first slice till F12).
#[gpui::test]
fn yuzora_diff_a_drag_while_the_rows_are_ranked_neither_ends_nor_stops_the_ranking(
    cx: &mut TestAppContext,
) {
    const ROWS: usize = 20_000;
    let mut pane = DiffPane::mount_diff(
        cx,
        numbered_diff(ROWS, "line"),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true, "hold": true}),
    );
    pane.settle_until_ranked_some();
    let held = pane.rows_ranked_in_all();
    assert!(
        pane.pending() && held > 0. && held < ROWS as f64,
        "{held} of {ROWS} rows are ranked, and the ranking is held"
    );
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(6));
    assert_eq!(
        mark_of(&pane),
        json!(["left", 3, 6]),
        "selected while the ranking is held"
    );
    // The ranking goes on with the button down, and the drag with it (a wheel lets it go: a key would end the drag).
    pane.wheel_held(TEXT_LEFT, row_y(6), 1., 0.);
    pane.drag(TEXT_LEFT, row_y(9));
    for _ in 0..30 {
        if !pane.pending() {
            break;
        }
        pane.settle();
    }
    assert!(
        !pane.pending(),
        "the ranking and the laying out were finished with the button down"
    );
    assert_eq!(
        mark_of(&pane),
        json!(["left", 3, 9]),
        "the drag went on to the row under the pointer"
    );
    pane.release(TEXT_LEFT, row_y(9));
    assert_eq!(
        selected(&mut pane),
        Some(json!([["l", 3, 10]])),
        "and it is there when the button is up"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu): the diff read again while the button is down (a file saved) ends the drag with the selection: the rows
// are others now, and the pointer moving on selects none of them.
#[gpui::test]
fn yuzora_diff_a_diff_read_again_during_a_drag_ends_it(cx: &mut TestAppContext) {
    let mut pane = DiffPane::mount_diff(
        cx,
        numbered_diff(200, "line"),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true, "reload": true}),
    );
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(5));
    assert_eq!(mark_of(&pane), json!(["left", 3, 5]));
    // A wheel makes the probe read the diff again, as a saved file would (a key would end the drag by itself).
    pane.wheel_held(TEXT_LEFT, row_y(5), 1., 0.);
    assert_eq!(
        mark_of(&pane),
        Value::Null,
        "the rows are others: nothing is selected"
    );
    pane.drag(TEXT_LEFT, row_y(9));
    pane.release(TEXT_LEFT, row_y(9));
    assert_eq!(
        mark_of(&pane),
        Value::Null,
        "and the pointer moving on selects nothing"
    );
    // Pressed on a row and not yet moved off it: the row it was pressed on is a row of the old diff.
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.wheel_held(TEXT_LEFT, row_y(3), 1., 0.);
    pane.drag(TEXT_LEFT, row_y(6));
    pane.release(TEXT_LEFT, row_y(6));
    assert_eq!(
        mark_of(&pane),
        Value::Null,
        "a press before the diff was read again begins nothing in the rows that came"
    );
    assert_eq!(selected(&mut pane), None);
    pane.close();
}

// yuzora(v010-git-diff-fu): a key pressed with the button down (Cmd+C to copy what is selected so far) copies it and ends the drag. The
// window counts the pointer as over nothing after a key until it moves, so the release may never reach the pane, and a drag
// that waited for it would go on selecting under a pointer with no button down.
#[gpui::test]
fn yuzora_diff_a_key_pressed_during_a_drag_copies_what_is_selected_and_ends_it(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(5));
    assert_eq!(
        copied(&mut pane).as_deref(),
        Some("line 3\nline 4\nline 5"),
        "copied with the button down"
    );
    pane.release(TEXT_LEFT, row_y(5));
    pane.hover(TEXT_LEFT, row_y(9));
    assert_eq!(
        mark_of(&pane),
        json!(["left", 3, 5]),
        "the pointer moving on after the key selects no more"
    );
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): the release of a button can be missed (an app-wide shortcut pressed during the drag takes the key
// away from the pane, and after a key the window counts the pointer as over nothing, so the pane's root sees no release),
// and the first move after it says the button is up. It ends the drag whatever ended the button's hold: the rows are not selected
// by a pointer moving with no button down, nor the list scrolled by one held at its edge.
#[gpui::test]
fn yuzora_diff_a_move_with_no_button_down_ends_a_drag_across_rows(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(5));
    pane.drag(TEXT_LEFT, bottom);
    pane.wait(4, 50);
    let (mark, scrolled) = (mark_of(&pane), scrolled_in_frame(&pane));
    assert!(
        scrolled >= 40.,
        "held at the edge the list scrolls: {scrolled}"
    );
    pane.hover(TEXT_LEFT, row_y(9));
    let built = pane.script_renders();
    pane.wait(40, 50);
    assert_eq!(
        mark_of(&pane),
        mark,
        "the pointer moving with no button down selects no more"
    );
    near(
        scrolled_in_frame(&pane),
        scrolled,
        "and the list it left at the edge scrolls no more",
    );
    assert_eq!(pane.script_renders(), built, "no clock builds a frame");
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): the same for the divider's drag, which a missed release left on as well.
#[gpui::test]
fn yuzora_diff_a_move_with_no_button_down_ends_the_dividers_drag_and_keeps_its_share(
    cx: &mut TestAppContext,
) {
    const PANE: f64 = 1200.;
    let shared = Shared::default();
    let mut pane = DiffPane::mount(cx, long_rows(), PANE as f32, &shared);
    let half = (PANE - 1.) / 2.;
    let at = pane.width("l") + 0.5;
    pane.press(at, 100., 1);
    pane.drag(at + 120., 100.);
    pane.hover(at - 200., 100.);
    near(
        pane.width("l"),
        half + 120.,
        "the pointer moving with no button down drags nothing",
    );
    assert!(
        (kept(&shared).as_f64().unwrap() - (half + 120.) / (PANE - 1.)).abs() < 0.001,
        "and the share dragged to is kept: {:?}",
        kept(&shared)
    );
    pane.close();
}

// yuzora(v010-git-diff-fu-r1): a drag goes on only while the pane has the keyboard it was given by the press: another pane shown
// or opened by a shortcut takes it, and the clock of a drag the pane is not shown for would otherwise build a frame every 50 ms
// for as long as the pointer is left where it was.
#[gpui::test]
fn yuzora_diff_a_drag_ends_when_the_pane_no_longer_has_the_keyboard(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    pane.press(TEXT_LEFT, row_y(5), 1);
    pane.drag(TEXT_LEFT, row_y(8));
    pane.drag(TEXT_LEFT, bottom);
    pane.wait(10, 50);
    assert!(
        scrolled_in_frame(&pane) >= 100.,
        "held at the edge the list scrolls"
    );
    let elsewhere = pane.focus_elsewhere();
    pane.wait(2, 50);
    let (scrolled, built) = (scrolled_in_frame(&pane), pane.script_renders());
    pane.wait(20, 50);
    near(
        scrolled_in_frame(&pane),
        scrolled,
        "the list scrolls no more",
    );
    assert_eq!(pane.script_renders(), built, "and no clock builds a frame");
    drop(elsewhere);
    pane.close();
}
// yuzora(v010-git-diff-fu-r1): the keyboard is only a reason to end a drag when it was the pane's and is another's now. A pane that
// never had it for the drag (here it was taken before the pointer moved) does not lose the drag to that: nothing shows in a test
// window that the pane has it in the app, and a drag that ended at its first tick for want of it would be no drag at all.
#[gpui::test]
fn yuzora_diff_a_drag_that_began_without_the_keyboard_is_not_ended_for_the_want_of_it(
    cx: &mut TestAppContext,
) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    let bottom = pane.high as f64 - 8.;
    pane.press(TEXT_LEFT, row_y(5), 1);
    let elsewhere = pane.focus_elsewhere();
    pane.drag(TEXT_LEFT, row_y(8));
    pane.drag(TEXT_LEFT, bottom);
    pane.wait(10, 50);
    assert!(
        scrolled_in_frame(&pane) >= 100.,
        "the list scrolls with the pointer held at its edge"
    );
    drop(elsewhere);
    pane.close();
}
// yuzora(v010-git-diff-fu): a drag that begins in a hunk makes it the current one, as the click on that row would: it is the hunk
// the stage and discard buttons act on, and a drag ends on another row than the one pressed, so no click does it.
#[gpui::test]
fn yuzora_diff_a_drag_makes_the_hunk_it_began_in_the_current_one(cx: &mut TestAppContext) {
    let change = |h: u64, i: u64| json!({"k": "~", "h": h, "l": i + 1, "li": i, "lt": format!("old {i}"), "r": i + 1, "ri": i, "rt": format!("new {i}")});
    let same = |i: u64| json!({"k": "=", "l": i + 1, "r": i + 1, "t": format!("same {i}")});
    let rows = Value::Array(vec![
        same(0),
        change(0, 1),
        change(0, 2),
        same(3),
        same(4),
        change(1, 5),
        change(1, 6),
        same(7),
    ]);
    let mut diff = Diff::of(rows);
    diff.hunks = json!([{}, {}]);
    let mut pane = copy_pane(cx, diff, json!({}));
    let hunk = |pane: &mut DiffPane| {
        pane.keys("f9");
        pane.noted("hunk").last().unwrap()[1].as_u64().unwrap()
    };
    pane.click(TEXT_LEFT, row_y(0), false);
    assert_eq!(hunk(&mut pane), 0, "the first hunk to begin with");
    pane.press(TEXT_LEFT, row_y(5), 1);
    pane.drag(TEXT_LEFT, row_y(7));
    pane.release(TEXT_LEFT, row_y(7));
    assert_eq!(
        hunk(&mut pane),
        1,
        "a drag from a row of the second hunk makes it the current one"
    );
    pane.press(TEXT_LEFT, row_y(1), 1);
    pane.drag(TEXT_LEFT, row_y(3));
    pane.release(TEXT_LEFT, row_y(3));
    assert_eq!(hunk(&mut pane), 0, "and one from the first, the first");
    pane.close();
}

// yuzora(v010-git-diff-fu): the pointer leaving the pane with the button down ends the drag where it was, as the divider's does (the
// release would not be seen out there): the rows selected so far stay selected, and coming back in with the button still down
// selects no more of them. The clock stops with it: no frame is drawn by a drag that is over.
#[gpui::test]
fn yuzora_diff_a_drag_across_rows_ends_where_the_pointer_leaves_the_pane(cx: &mut TestAppContext) {
    let mut pane = copy_pane(cx, numbered_diff(200, "line"), json!({}));
    pane.press(TEXT_LEFT, row_y(3), 1);
    pane.drag(TEXT_LEFT, row_y(5));
    // Past the window's edge (a `real` pane fills the window).
    pane.drag(1300., row_y(8));
    assert_eq!(
        mark_of(&pane),
        json!(["left", 3, 5]),
        "out of the pane: what was selected stays"
    );
    pane.drag(TEXT_LEFT, row_y(10));
    pane.wait(5, 50);
    assert_eq!(
        mark_of(&pane),
        json!(["left", 3, 5]),
        "back in with the button down: not a drag any more"
    );
    pane.release(TEXT_LEFT, row_y(10));
    assert_eq!(selected(&mut pane), Some(json!([["l", 3, 6]])));
    pane.close();
}

// yuzora(v010): an ordinary diff (thousands of lines, a few with wide characters) scrolls sideways
// to where its longest line ends and not past it: the room that is left for lines that were not
// laid out is what they could take at most, and for plain text that is what they take.
#[gpui::test]
fn yuzora_diff_an_ordinary_diff_scrolls_to_where_its_longest_line_ends(cx: &mut TestAppContext) {
    let mut rows = vec![diff_row(Some((200, "a")), Some((5, "b")))];
    rows.extend((0..6000).map(|i| match i % 500 {
        0 => diff_row(Some((50, "\u{6f22}")), Some((5, "b"))),
        1 => text_row(Some(&format!("{} \u{1f600}", "w".repeat(40))), Some("b")),
        _ => diff_row(
            Some((20 + i * 37 % 100, "a")),
            Some((20 + i * 11 % 100, "b")),
        ),
    }));
    let mut pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    assert!(
        pane.batches() <= 2,
        "{} batches of lines were laid out: an ordinary diff is measured with one a side, the lines that cannot be wider than the widest laid out are not laid out",
        pane.batches()
    );
    pane.wheel(100., 100., -100000., 0.);
    let end = text_start(&mut pane, "a") + pane.text_width("a");
    let cell = pane.width("l");
    assert!((cell - left_after_the_end() - 1.5 ..=cell - left_after_the_end() + 1.5).contains(&end), "scrolled as far as it goes, the longest line ends at {end}, its cell at {cell}");
    pane.close();
}

// yuzora(v010): a whole file in context (every row unchanged, the same line on both sides) is read once for both and
// measured with one round of lines laid out, and so scrolls to where its longest line ends: 10,000 lines of up to 160
// characters, the pane holding about 80 of them.
#[gpui::test]
fn yuzora_diff_a_whole_file_in_context_is_measured_in_one_round(cx: &mut TestAppContext) {
    let rows = Value::Array((0..10_000).map(|i| json!({"k": "=", "l": i + 1, "r": i + 1, "t": format!("{}{}", "a".repeat(30), "b".repeat(130 - i % 131))})).collect());
    let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
    assert!(
        pane.batches() <= 2,
        "{} batches of lines were laid out, when one a side is enough",
        pane.batches()
    );
    pane.wheel(100., 100., -100000., 0.);
    let end = text_start(&mut pane, "a") + pane.text_width("a");
    let cell = pane.width("l");
    assert!((cell - left_after_the_end() - 1.5 ..=cell - left_after_the_end() + 1.5).contains(&end), "scrolled as far as it goes, the widest line ends at {end}, its cell at {cell}");
    pane.close();
}

// yuzora(v010): a slice that fails (a `fail` prop: the first scan throws, as it would on a row that is not what the rows
// are) says so in the pane, and the diff that comes next opens, without the error: the rows are ranked after the diff is
// on show, so the failure is in the pane that shows it, not in the diff that was there.
#[gpui::test]
fn yuzora_diff_a_slice_that_fails_says_so_and_the_next_diff_works(cx: &mut TestAppContext) {
    let fine = json!({"id": "fine", "rows": [diff_row(None, Some((300, "b")))], "hunks": [{}], "stageable": true, "add": 1, "del": 0, "leftExists": true, "rightExists": true});
    let mut pane = DiffPane::mount_with(
        cx,
        json!([diff_row(None, Some((300, "b")))]),
        1200.,
        1200.,
        &Shared::default(),
        json!({"fail": 1}),
    );
    let shows = |pane: &DiffPane| pane.rendered().last().cloned().expect("a frame");
    let (shown, error) = shows(&pane);
    assert_eq!(shown, json!("d1"));
    assert!(
        error
            .as_str()
            .is_some_and(|e| e.contains("not what the rows are")),
        "the pane says so: {error}"
    );
    pane.redraw();
    assert!(
        shows(&pane).1.is_string(),
        "drawing it again keeps saying so"
    );
    pane.answer_next(vec![fine]);
    pane.press(400., 10., 1);
    assert_eq!(
        shows(&pane),
        (json!("fine"), Value::Null),
        "the next diff opens, with no error"
    );
    pane.close();
}

// yuzora(v010): the sideways offset that is kept stays within the room to scroll: a wheel over lines
// that fit keeps nothing, and an offset is brought back when the room for it shrinks. A narrower pane
// or a moved divider does not move the text with no wheel turned (the room it makes would have been
// filled by what the wheel only asked for).
#[gpui::test]
fn yuzora_diff_the_kept_sideways_offset_stays_within_the_room_to_scroll(cx: &mut TestAppContext) {
    const GUTTER: f64 = 44.;
    let home = GUTTER + 6.;
    // A line of 400px: it fits a side of a 1200px pane, and not one of 700px (112.5px short).
    let mut pane = DiffPane::mount(cx, json!([diff_row(Some((58, "a")), Some((5, "b")))]), 1200., &Shared::default());
    pane.wheel(100., 100., -400., 0.);
    near(left_text_start(&mut pane), home, "lines that fit do not move");
    pane.resize(700.);
    near(left_text_start(&mut pane), home, "a narrower pane does not move them");
    pane.resize(1200.);
    let at = pane.width("l") + 0.5;
    pane.press(at, 100., 1);
    pane.drag(at - 300., 100.);
    pane.release(at - 300., 100.);
    near(left_text_start(&mut pane), home, "nor does a divider moved to leave the left side less");
    pane.close();

    let mut pane = DiffPane::mount(cx, json!([diff_row(Some((58, "a")), Some((5, "b")))]), 700., &Shared::default());
    pane.wheel(100., 100., -100000., 0.);
    let end = left_text_start(&mut pane);
    assert!(end < home - 100., "scrolled as far as there is room: {end}");
    pane.resize(1200.);
    near(left_text_start(&mut pane), home, "with room for all of it the lines are home");
    pane.resize(700.);
    near(left_text_start(&mut pane), home, "and they stay there when the room shrinks again");
    pane.close();
}

// yuzora(v010): the bar under the lines has room of its own: the list of lines stops where it starts, so
// its last row is not under the bar's thumb (which took the clicks of that row's numbers).
#[gpui::test]
fn yuzora_diff_the_bar_under_the_lines_has_room_of_its_own(cx: &mut TestAppContext) {
    let height = |cx: &mut TestAppContext, left: usize| {
        let pane = DiffPane::mount(cx, json!([diff_row(Some((left, "a")), Some((5, "b")))]), 1200., &Shared::default());
        let height = pane.list_height();
        pane.close();
        height
    };
    let (fits, long) = (height(cx, 20), height(cx, 300));
    near(fits - long, 8., "the list is shorter by the bar when the lines need one");
}

// yuzora(v010): a click on a line's number picks it through the list's own click (the other tests'
// harness picks from the window's press): on the left or the right side's, with the bar and the divider
// over the list; the text beside them does not pick.
#[gpui::test]
fn yuzora_diff_a_click_on_a_line_number_picks_it_through_the_list(cx: &mut TestAppContext) {
    const GUTTER: f64 = 44.;
    let mut pane = DiffPane::mount_with(cx, json!([diff_row(Some((300, "a")), Some((300, "b")))]), 1200., 1200., &Shared::default(), json!({"real": true}));
    let right = pane.width("l") + 1.;
    let mut failures = Vec::new();
    for (x, want) in [(GUTTER / 2., Some("l")), (right + GUTTER / 2. + 1., Some("r")), (GUTTER + 12., None), (right + GUTTER + 12., None)] {
        let before = pane.notes.borrow().len();
        pane.press(x, 38., 1);
        pane.release(x, 38.);
        let picked: Vec<String> = pane.notes.borrow().iter().skip(before).filter(|n| n[0] == "pick").map(|n| n[1].as_str().unwrap_or("").to_owned()).collect();
        if picked.first().map(String::as_str) != want || picked.len() > 1 {
            failures.push(format!("a click at x {x} picked {picked:?}, wanted {want:?}"));
        }
    }
    pane.close();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

thread_local! {
    /// The characters a [`MeasuredFont`] laid out since a frame last took the count.
    static LAID: Cell<usize> = const { Cell::new(0) };
}

/// A font as it was measured (`scripts/yuzora-git-diff-font-widths.swift`: JetBrains Mono at 11.5px, with
/// what the system falls back to): a character is as many cells (the width of an ASCII one) as the table
/// says. The window's own test font gives every character but those past U+FFFF one cell, whatever the family.
/// This one knows two: JetBrains Mono, and any other, whose cell is a sixth wider, so a text laid out in
/// another family than the one the rows are drawn in is not as wide as they are. It counts the characters
/// it lays out ([`LAID`]).
struct MeasuredFont(std::collections::HashMap<char, f32>, gpui::NoopTextSystem);

impl MeasuredFont {
    fn new() -> Self {
        let table: Value = serde_json::from_str(include_str!("../../scripts/fixtures/yuzora-git-diff-font-widths.json")).unwrap();
        let cells = table["samples"].as_array().unwrap().iter().filter_map(|s| {
            let mut chars = s["text"].as_str()?.chars();
            let first = chars.next()?;
            chars.next().is_none().then(|| (first, s["cells"].as_f64().unwrap() as f32))
        });
        Self(cells.collect(), gpui::NoopTextSystem::new())
    }
}

impl gpui::PlatformTextSystem for MeasuredFont {
    fn add_fonts(&self, fonts: Vec<std::borrow::Cow<'static, [u8]>>) -> anyhow::Result<()> { self.1.add_fonts(fonts) }
    fn all_font_names(&self) -> Vec<String> { self.1.all_font_names() }
    fn font_id(&self, descriptor: &gpui::Font) -> anyhow::Result<gpui::FontId> { Ok(gpui::FontId(if descriptor.family.as_ref() == "JetBrains Mono" { 0 } else { 1 })) }
    fn font_metrics(&self, font_id: gpui::FontId) -> gpui::FontMetrics { self.1.font_metrics(font_id) }
    fn typographic_bounds(&self, font_id: gpui::FontId, glyph_id: gpui::GlyphId) -> anyhow::Result<gpui::Bounds<f32>> { self.1.typographic_bounds(font_id, glyph_id) }
    fn advance(&self, font_id: gpui::FontId, glyph_id: gpui::GlyphId) -> anyhow::Result<gpui::Size<f32>> { self.1.advance(font_id, glyph_id) }
    fn glyph_for_char(&self, font_id: gpui::FontId, ch: char) -> Option<gpui::GlyphId> { self.1.glyph_for_char(font_id, ch) }
    fn glyph_raster_bounds(&self, params: &gpui::RenderGlyphParams) -> anyhow::Result<gpui::Bounds<gpui::DevicePixels>> { self.1.glyph_raster_bounds(params) }
    fn rasterize_glyph(&self, params: &gpui::RenderGlyphParams, raster_bounds: gpui::Bounds<gpui::DevicePixels>) -> anyhow::Result<(gpui::Size<gpui::DevicePixels>, Vec<u8>)> { self.1.rasterize_glyph(params, raster_bounds) }
    fn recommended_rendering_mode(&self, font_id: gpui::FontId, font_size: gpui::Pixels) -> gpui::TextRenderingMode { self.1.recommended_rendering_mode(font_id, font_size) }

    fn layout_line(&self, text: &str, font_size: gpui::Pixels, runs: &[gpui::FontRun]) -> gpui::LineLayout {
        LAID.with(|laid| laid.set(laid.get() + text.chars().count()));
        let metrics = self.1.font_metrics(gpui::FontId(0));
        let font_id = runs.first().map_or(gpui::FontId(0), |run| run.font_id);
        let cell = font_size * 0.6 * if font_id == gpui::FontId(0) { 1. } else { 7. / 6. };
        let mut position = gpui::px(0.);
        let mut glyphs = Vec::new();
        for (index, c) in text.char_indices() {
            let cells = self.0.get(&c).copied().unwrap_or_else(|| {
                assert!(c.is_ascii(), "{c:?} (U+{:04X}) is not in scripts/fixtures/yuzora-git-diff-font-widths.json", c as u32);
                1.
            });
            glyphs.push(gpui::ShapedGlyph { id: gpui::GlyphId(c.len_utf16() as u32), position: gpui::point(position, gpui::px(0.)), index, is_emoji: false });
            position += cell * cells;
        }
        let width = if glyphs.is_empty() { gpui::px(0.) } else { position };
        gpui::LineLayout {
            font_size,
            width,
            ascent: font_size * (metrics.ascent / metrics.units_per_em as f32),
            descent: font_size * (metrics.descent / metrics.units_per_em as f32),
            runs: if glyphs.is_empty() { Vec::new() } else { vec![gpui::ShapedRun { font_id, glyphs }] },
            len: text.len(),
        }
    }
}

/// `test` in a window that draws with [`MeasuredFont`].
fn with_measured_font(name: &'static str, test: impl Fn(&mut TestAppContext) + std::panic::RefUnwindSafe) {
    gpui::run_test(1, &[0], 0, &mut |dispatcher, _seed| {
        let mut cx = TestAppContext::build_with_text_system(dispatcher.clone(), Some(name), std::sync::Arc::new(MeasuredFont::new()));
        let _entity_refcounts = cx.app.borrow().ref_counts_drop_handle();
        test(&mut cx);
        cx.run_until_parked();
        cx.update(|cx| { cx.background_executor().forbid_parking(); cx.quit(); });
        cx.run_until_parked();
        drop(cx);
        dispatcher.drain_tasks();
        drop(dispatcher);
    }, None);
}

// yuzora(v010): the tail of the longest line can be scrolled to in the font that draws the lines, when
// the lines that look longest in characters are not. From the Git diff review: with four lines of a
// hundred Chinese characters (11.5px each, 1.67 cells) a line of 180 ASCII ones is the widest;
// with four of 170 ASCII ones, a line of a hundred and sixty characters with a check mark (2.17 cells) is; and so with Korean (1.44 cells).
#[test]
fn yuzora_diff_the_longest_lines_tail_scrolls_into_view_in_the_font_that_draws_it() {
    with_measured_font("yuzora_diff_the_longest_lines_tail_scrolls_into_view_in_the_font_that_draws_it", |cx| {
        let cases = [
            ("180 ASCII beside 4 of 100 Chinese", "a", text_row(Some(&"a".repeat(180)), Some("b")), "\u{6f22}".repeat(100)),
            ("40 check marks beside 4 of 170 ASCII", "|", text_row(Some(&"| \u{2705} ".repeat(40)), Some("b")), "a".repeat(170)),
            ("160 ASCII beside 4 of 100 Korean", "a", text_row(Some(&"a".repeat(160)), Some("b")), "\u{d55c}".repeat(100)),
            ("100 Chinese beside 4 of 160 ASCII", "\u{6f22}", text_row(Some(&"\u{6f22}".repeat(100)), Some("b")), "a".repeat(160)),
            ("100 emoji beside 4 of 205 ASCII", "\u{1f600}", text_row(Some(&"\u{1f600}".repeat(100)), Some("b")), "a".repeat(205)),
        ];
        let mut failures = Vec::new();
        for (case, first, longest, others) in cases {
            let mut rows = vec![longest];
            rows.extend((0..4).map(|_| text_row(Some(&others), Some("b"))));
            let mut pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
            pane.wheel(100., 100., -100000., 0.);
            let end = text_start(&mut pane, first) + pane.text_width(first);
            let cell = pane.width("l");
            if !(cell - left_after_the_end() - 1.5 ..=cell - left_after_the_end() + 1.5).contains(&end) {
                failures.push(format!("{case}: scrolled as far as it goes, the longest line ends at {end}, its cell at {cell}"));
            }
            pane.close();
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

/// A test that sets the zoom runs alone. The zoom is one value for the whole process (`gpui_shell::set_pixel_scale`)
/// and the tests beside it read exact pixels at zoom 1, so this returns true in a test process of its own, and
/// elsewhere runs the test in that one, checks it passed and returns false.
fn alone(name: &str) -> bool {
    if std::env::var("YUZORA_DIFF_ALONE").as_deref() == Ok(name) {
        return true;
    }
    let out = crate::spawn::command(std::env::current_exe().unwrap().to_str().unwrap())
        .args(["--exact", &format!("mods::lifecycle_tests::{name}"), "--nocapture", "--test-threads=1"])
        .env("YUZORA_DIFF_ALONE", name)
        .output()
        .unwrap();
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && log.contains("1 passed"), "{name} failed alone:\n{log}");
    false
}

/// The zooms checked: below, at, and above 1, to the largest the person can pick but one.
const ZOOMS: [f32; 5] = [0.8, 1., 1.2, 1.5, 2.];

/// The first row's left line, `len` letters, is the longest; where `rest` is set, four lines that can take more (a
/// letter with an accent takes 1.25 cells at most, against 1) come before it in the order the lines are laid out in, and
/// are, so the longest is laid out in a frame after the first.
fn longest_first(len: usize, rest: bool) -> Value {
    let mut rows = vec![diff_row(Some((len, "a")), Some((5, "b")))];
    if rest {
        rows.extend((0..4).map(|_| diff_row(Some((len * 4 / 5 + 1, "\u{e9}")), Some((5, "b")))));
    }
    Value::Array(rows)
}

/// What is left of a side after its longest line's end, at zoom 1, when the lines are scrolled as far as they go:
/// the 18 the mod leaves less the 6 of indent before the line.
fn left_after_the_end() -> f64 {
    12.
}

/// How far from that the layout may put it: a pixel of the window, and one more for the line's edge.
fn room_for_error(_len: usize) -> f64 {
    2.
}

/// What the tests of the zoom open: (case, the letters of the longest line, whether four lines that can take more are
/// laid out before it).
const LONGEST: [(&str, usize, bool); 3] = [("a line laid out first", 300, false), ("a line behind four", 124, true), ("a long line behind four", 1600, true)];

// yuzora(v010): at any zoom the lines scroll sideways to where the longest one ends and no further, and the
// bar's thumb says so: it starts the track and ends it as the lines do, as long as the share of the lines it
// stands for. A script's lengths are multiplied by the zoom and a size or a pointer comes back in window pixels,
// so each of the two has to be turned into the other's unit before they meet.
#[gpui::test]
fn yuzora_diff_the_sideways_scroll_ends_where_the_longest_line_does_at_every_zoom(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_the_sideways_scroll_ends_where_the_longest_line_does_at_every_zoom") {
        return;
    }
    let mut failures = Vec::new();
    for zoom in ZOOMS {
        for (case, len, rest) in LONGEST {
            let mut pane = DiffPane::zoomed(cx, longest_first(len, rest), zoom, json!({}));
            let width = pane.wide as f64;
            let home = pane.start_of("a").expect("the line at home");
            let bar = pane.thumb();
            let (gap, far) = (pane.end_gap("a"), pane.start_of("a"));
            let thumb = pane.thumb();
            pane.close();
            let (z, mark) = (zoom as f64, format!("{case} at zoom {zoom}"));
            let (Some(gap), Some(far)) = (gap, far) else { failures.push(format!("{mark}: scrolled to the end, no line is left in sight")); continue };
            let travel = home - far;
            if (gap - left_after_the_end() * z).abs() > room_for_error(len) {
                failures.push(format!("{mark}: {gap} left after the end, when it is {}", left_after_the_end() * z));
            }
            let Some((start, long)) = bar else { failures.push(format!("{mark}: no bar at home")); continue };
            let Some((end_start, end_long)) = thumb else { failures.push(format!("{mark}: no bar at the end")); continue };
            if start.abs() > 1. {
                failures.push(format!("{mark}: the thumb starts at {start} at home"));
            }
            let want = width * width / (width + travel);
            if (long - want).abs() > 2. * z || (end_long - want).abs() > 2. * z {
                failures.push(format!("{mark}: the thumb is {long} long at home and {end_long} at the end; the lines go {travel} of {width}: {want}"));
            }
            if (end_start + end_long - width).abs() > 1.5 {
                failures.push(format!("{mark}: the thumb ends at {} of the {width} of the track at the end", end_start + end_long));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a zoom set while the diff is open is in the frame after it: the offset that was kept is brought
// within the room there is at the new zoom, and the thumb is where the lines are, without waiting for the sizes
// the new zoom makes to be reported. Scrolled to the end at 2, each smaller zoom has room for more of the
// line at once, so the offset is the farthest there is; each larger one has room for less, and the offset
// stays what it was, the thumb within the track.
#[gpui::test]
fn yuzora_diff_a_zoom_set_while_it_is_open_holds_from_the_next_frame(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_a_zoom_set_while_it_is_open_holds_from_the_next_frame") {
        return;
    }
    let mut failures = Vec::new();
    for (case, len, rest) in LONGEST {
        let mut pane = DiffPane::zoomed(cx, longest_first(len, rest), 2., json!({}));
        pane.wheel(100., 100., -100000., 0.);
        for (zoom, smaller) in [(1.5, true), (1., true), (0.8, true), (1.2, false), (2., false), (1.5, false)] {
            pane.set_zoom(zoom);
            let next = pane.thumb();
            pane.settle();
            let settled = pane.thumb();
            let mark = format!("{case}, set to {zoom}");
            match (next, settled) {
                (Some(a), Some(b)) if (a.0 - b.0).abs() <= 2. && (a.1 - b.1).abs() <= 2. => {}
                (a, b) => failures.push(format!("{mark}: the next frame has the thumb at {a:?}, once the sizes are reported at {b:?}")),
            }
            if settled.is_none_or(|(start, long)| start + long > 1200. + 1.5) {
                failures.push(format!("{mark}: the thumb is {settled:?} on a track of 1200"));
            }
            let gap = pane.start_of("a").map(|start| pane.width("l") - (start + pane.text_width("a")));
            let (farthest, error) = (left_after_the_end() * zoom as f64, room_for_error(len));
            match gap {
                Some(gap) if gap < farthest - error && smaller => failures.push(format!("{mark}: {gap} left after the end, and the farthest the lines go leaves {farthest}")),
                Some(gap) if gap > farthest + error => failures.push(format!("{mark}: {gap} left after the end, and the farthest the lines go leaves {farthest}")),
                None if smaller => failures.push(format!("{mark}: no line is in sight")),
                _ => {}
            }
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a zoom set while the diff is open measures its lines again: each piece of a line is rounded up to a pixel
// of the window, and what a pixel is, in the unit the pane writes lengths in, is what the zoom makes it. A line of 40
// pieces measured at zoom 2 is 20px short of what is drawn at zoom 1.
#[gpui::test]
fn yuzora_diff_a_zoom_set_while_it_is_open_measures_the_lines_again(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_a_zoom_set_while_it_is_open_measures_the_lines_again") {
        return;
    }
    let mut failures = Vec::new();
    for (from, to) in [(2., 1.), (1., 1.5), (1.5, 0.8)] {
        let mut pane = DiffPane::zoomed(cx, Value::Array(vec![row_in_pieces('a', &pieces_of(&[9], 360))]), from, json!({}));
        pane.set_zoom(to);
        pane.settle();
        let gap = pane.end_gap("a");
        pane.close();
        match gap {
            Some(gap) if (gap - left_after_the_end() * to as f64).abs() <= 1.5 => {}
            other => failures.push(format!("set from {from} to {to}: {other:?} left after the end, when it is {}", left_after_the_end() * to as f64)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): at any zoom a wheel or a swipe moves the lines as far as it went on the screen, the thumb
// follows the pointer that drags it, the divider follows the pointer that drags it and its grab area is
// where it is drawn, and a press on a line's number picks the line only on the number. Pointers and wheels
// come in window pixels, and what the pane lays out and keeps is in the unit a script writes.
#[gpui::test]
fn yuzora_diff_the_pointer_and_the_wheel_move_what_they_touch_at_every_zoom(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_the_pointer_and_the_wheel_move_what_they_touch_at_every_zoom") {
        return;
    }
    let mut failures = Vec::new();
    for zoom in ZOOMS {
        let z = zoom as f64;
        let mut pane = DiffPane::zoomed(cx, longest_first(300, false), zoom, json!({}));
        let home = pane.start_of("a").unwrap();
        pane.wheel(100., 100., -200., 0.);
        let moved = home - pane.start_of("a").unwrap_or(f64::NAN);
        if (moved - 200.).abs() > 1.5 {
            failures.push(format!("zoom {zoom}: a wheel of 200px moved the lines {moved}px"));
        }
        pane.wheel(100., 100., 100000., 0.);

        let (start, long) = pane.thumb().expect("a bar");
        let (grab, bar) = (start + long / 2., pane.high as f64 - 4. * z);
        pane.press(grab, bar, 1);
        pane.drag(grab + 100., bar);
        match pane.thumb() {
            Some((dragged, _)) if (dragged - start - 100.).abs() <= 2. => {}
            now => failures.push(format!("zoom {zoom}: the pointer dragged the thumb 100px, from {start}, and it is at {now:?}")),
        }
        pane.drag(1199., bar);
        pane.release(1199., bar);
        match pane.thumb() {
            Some((end, long)) if (end + long - 1200.).abs() <= 1.5 => {}
            now => failures.push(format!("zoom {zoom}: dragged to the end of the track the thumb is at {now:?}")),
        }
        pane.close();

        let shared = Shared::default();
        let mut pane = DiffPane::mount_at(cx, Diff::of(long_rows()), 1200., (1200., 1300.), zoom, &shared, json!({}));
        let (side, divider) = (pane.width("l"), |pane: &DiffPane| pane.width("l") + z / 2.);
        let at = divider(&pane);
        pane.press(at, 100. * z, 1);
        pane.drag(at + 120., 100. * z);
        pane.release(at + 120., 100. * z);
        if (pane.width("l") - side - 120.).abs() > 1.5 {
            failures.push(format!("zoom {zoom}: the divider dragged 120px moved {}px", pane.width("l") - side));
        }
        let kept = kept(&shared).as_f64().unwrap_or(f64::NAN);
        if (kept - pane.width("l") / (1200. - z)).abs() > 0.002 {
            failures.push(format!("zoom {zoom}: the share kept is {kept} of a left side that is {} of the {} there is to share", pane.width("l"), 1200. - z));
        }
        // The grab area is drawn 5 around the divider's 1, 2 before it: in, and out.
        for (from, grabbed) in [(-1.5 * z, true), (-3. * z, false)] {
            let (side, at) = (pane.width("l"), divider(&pane) - z / 2. + from);
            pane.press(at, 100. * z, 1);
            pane.drag(at + 50., 100. * z);
            pane.release(at + 50., 100. * z);
            let moved = pane.width("l") - side;
            if grabbed != (moved > 25.) {
                failures.push(format!("zoom {zoom}: a press {from}px from the divider's edge {} (moved {moved}px)", if grabbed { "did not grab it" } else { "grabbed it" }));
            }
        }
        pane.close();

        let mut pane = DiffPane::zoomed(cx, json!([diff_row(Some((300, "a")), Some((300, "b")))]), zoom, json!({"real": true}));
        let right = pane.width("l") + z;
        for (x, want) in [(22. * z, Some("l")), (44. * z - 3., Some("l")), (44. * z + 3., None), (right + 22. * z, Some("r")), (right + 44. * z - 3., Some("r")), (right + 44. * z + 3., None)] {
            let before = pane.notes.borrow().len();
            pane.press(x, 38. * z, 1);
            pane.release(x, 38. * z);
            let picked: Vec<String> = pane.notes.borrow().iter().skip(before).filter(|n| n[0] == "pick").map(|n| n[1].as_str().unwrap_or("").to_owned()).collect();
            if picked.first().map(String::as_str) != want || picked.len() > 1 {
                failures.push(format!("zoom {zoom}: a click at x {x} picked {picked:?}, wanted {want:?}"));
            }
        }
        pane.close();

        let rows = json!([{"k": "-", "h": 0, "l": 1, "li": 0, "t": "old"}, {"k": "+", "h": 0, "r": 1, "ri": 0, "t": "new"}]);
        let mut pane = DiffPane::zoomed(cx, rows, zoom, json!({"layout": "unified", "real": true}));
        for (x, y, want) in [(22. * z, 38., Some(("l", 0))), (88. * z - 3., 38., Some(("l", 0))), (88. * z + 3., 38., None), (88. * z - 3., 58., Some(("r", 0))), (88. * z + 3., 58., None)] {
            let before = pane.notes.borrow().len();
            pane.press(x, y * z, 1);
            pane.release(x, y * z);
            let picked: Vec<(String, i64)> = pane.notes.borrow().iter().skip(before).filter(|n| n[0] == "pick").map(|n| (n[1].as_str().unwrap().to_owned(), n[2].as_i64().unwrap())).collect();
            if picked.first().map(|(side, i)| (side.as_str(), *i)) != want || picked.len() > 1 {
                failures.push(format!("zoom {zoom}: unified, a click at x {x}, y {y}z picked {picked:?}, wanted {want:?}"));
            }
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A diff whose widest lines are the first rows, 300 characters on each side and cut into pieces, and `rows` rows after
/// them whose lines are short.
fn wide_lines_then(rows: usize) -> Value {
    let widest = |letter: char| pieces_of(&[4, 7, 3, 9, 5, 8, 6], 300).into_iter().map(|n| letter.to_string().repeat(n)).collect::<Vec<_>>();
    let cut = |letter: char| widest(letter).iter().enumerate().map(|(i, t)| json!({"t": t, "h": i % 2 == 1})).collect::<Vec<_>>();
    let mut all = vec![];
    for _ in 0..3 {
        all.push(json!({"k": "~", "h": 0, "l": 1, "li": 0, "lt": widest('a').concat(), "ls": cut('a'), "r": 1, "ri": 0, "rt": widest('b').concat(), "rs": cut('b')}));
    }
    all.extend((0..rows).map(|i| json!({"k": "~", "h": 0, "l": i + 2, "li": i + 1, "lt": "a line of text", "r": i + 2, "ri": i + 1, "rt": "another line"})));
    Value::Array(all)
}

/// What the frames of a pane cost: for each frame the diff was drawn in, the reads of the rows, what the module's scans
/// were given and the texts it built.
fn frame_costs(pane: &DiffPane) -> Vec<[f64; 3]> {
    pane.notes.borrow().iter().filter(|n| n[0] == "work").map(|n| [n[1].as_f64().unwrap(), n[2].as_f64().unwrap(), n[3].as_f64().unwrap()]).collect()
}

// yuzora(v010): what a frame costs does not depend on how many rows the diff has. Counted, not timed, and counted
// where the work is done: every read of a row and of the array that holds them, what the module's scans (`bound`, what
// ranks the lines) are asked to go through, the texts the frame builds and their characters, and the characters the
// window lays out. 2,000 rows and 60,000 have the same widest lines, so the same ones are laid out; each of their
// frames costs the same, and none of them scans a thing, while taking the diff in does go through all the rows.
#[test]
fn yuzora_diff_a_frame_costs_the_same_whatever_the_number_of_the_diffs_rows() {
    with_measured_font(
        "yuzora_diff_a_frame_costs_the_same_whatever_the_number_of_the_diffs_rows",
        |cx| {
            let mut costs = Vec::new();
            for rows in [2_000, 60_000] {
                let mut pane =
                    DiffPane::mount(cx, wide_lines_then(rows), 1200., &Shared::default());
                pane.redraw();
                let shown = pane.reads("shown");
                assert!(
                    shown.len() == 1 && shown[0] > 0. && pane.rows_ranked_in_all() >= rows as f64,
                    "{rows} rows: taking the diff in, and the slices after it, went through them: {shown:?} reads, {} ranked",
                    pane.rows_ranked_in_all()
                );
                let frames = frame_costs(&pane);
                assert!(
                    frames.len() >= 4,
                    "{rows} rows: frames were drawn with the diff in it: {frames:?}"
                );
                assert!(
                    pane.notes
                        .borrow()
                        .iter()
                        .any(|n| n[0] == "pending" && n[1].as_f64() == Some(1.)),
                    "{rows} rows: some lines were laid out out of sight"
                );
                assert!(
                    frames.iter().all(|f| f[1] == 0.),
                    "{rows} rows: a frame scanned what the diff came in with: {frames:?}"
                );
                let most = |i: usize| frames.iter().map(|f| f[i]).fold(0., f64::max);
                costs.push([
                    most(0),
                    most(1),
                    most(2),
                    pane.laid.borrow().iter().copied().max().unwrap_or(0) as f64,
                ]);
                pane.close();
            }
            let (few, many) = (costs[0], costs[1]);
            let names = ["row reads", "scans", "texts built", "characters laid out"];
            for i in 0..4 {
                assert!(
                    many[i] <= few[i] * 1.05 + 50.,
                    "a frame of 60,000 rows costs {} {}, of 2,000 {}",
                    many[i],
                    names[i],
                    few[i]
                );
            }
        },
    );
}

// yuzora(v010): the lines are laid out in the font the rows are drawn in, because it is the cell's own box that is laid
// out: in a window whose fonts differ by family (a sixth wider than JetBrains Mono's), the end of a line is
// where the range ends. Laid out in another family, it was a sixth short of it, or past it.
#[test]
fn yuzora_diff_the_lines_are_measured_in_the_font_the_rows_are_drawn_in() {
    with_measured_font("yuzora_diff_the_lines_are_measured_in_the_font_the_rows_are_drawn_in", |cx| {
        let mut failures = Vec::new();
        for (case, rows) in [
            ("a line", json!([text_row(Some(&"a".repeat(200)), Some("b"))])),
            ("a line cut into pieces", Value::Array(vec![row_in_pieces('a', &pieces_of(&[4, 7, 3, 9, 5, 8, 6], 400)), text_row(Some(&"a".repeat(380)), Some("b"))])),
            ("a line in a script the table lists", json!([text_row(Some(&"\u{6f22}".repeat(100)), Some("b"))])),
        ] {
            let first = if case.contains("script") { "\u{6f22}" } else { "a" };
            let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
            let gap = pane.end_gap(first);
            pane.close();
            match gap {
                Some(gap) if (gap - left_after_the_end()).abs() <= 1.5 => {}
                other => failures.push(format!("{case}: {other:?} left after its end, when it is {}", left_after_the_end())),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    });
}

/// A changed row whose left line is cut into pieces of `lengths` characters of `letter`, the way the word diff cuts a
/// line (every other piece is a change), each piece drawn on its own.
fn row_in_pieces(letter: char, lengths: &[usize]) -> Value {
    let pieces: Vec<Value> = lengths.iter().enumerate().map(|(i, n)| json!({"t": letter.to_string().repeat(*n), "h": i % 2 == 1})).collect();
    let text: String = pieces.iter().map(|p| p["t"].as_str().unwrap()).collect();
    json!({"k": "~", "h": 0, "l": 1, "li": 0, "lt": text, "ls": pieces, "r": 1, "ri": 0, "rt": "b"})
}

/// Pieces of the lengths in `cycle`, one after the other, until they are `total` characters.
fn pieces_of(cycle: &[usize], total: usize) -> Vec<usize> {
    let (mut out, mut sum) = (Vec::new(), 0);
    for n in cycle.iter().cycle() {
        let n = (*n).min(total - sum);
        if n == 0 {
            break;
        }
        out.push(n);
        sum += n;
    }
    out
}

// yuzora(v010): a line is scrolled to its end as it is drawn, and a line the word diff cut into pieces is drawn a
// piece at a time: each piece is as wide as its text, rounded up to a pixel. Working its width out as one piece fell
// short by up to a pixel a piece (24px for 360 characters in 40 pieces, 88px for 1,000 in 200, 38px for a line of
// JSON with 40 values changed), and the last characters of the line a change was made in could not be scrolled to.
#[gpui::test]
fn yuzora_diff_a_line_cut_into_pieces_is_scrolled_to_its_end_as_it_is_drawn(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_a_line_cut_into_pieces_is_scrolled_to_its_end_as_it_is_drawn") {
        return;
    }
    let cases = [
        ("360 letters in 40 pieces", pieces_of(&[9], 360)),
        ("360 letters in 120 pieces", pieces_of(&[3], 360)),
        ("1,000 letters in 200 pieces", pieces_of(&[5], 1000)),
        ("a line of JSON with 40 values changed", pieces_of(&[4, 7, 3, 9, 5, 8, 6], 901)),
    ];
    let mut failures = Vec::new();
    for zoom in ZOOMS {
        for (case, lengths) in &cases {
            let rows = Value::Array(vec![row_in_pieces('a', lengths), text_row(Some(&"a".repeat(120)), Some("b"))]);
            let mut pane = DiffPane::zoomed(cx, rows, zoom, json!({}));
            let gap = pane.end_gap("a");
            pane.close();
            let want = left_after_the_end() * zoom as f64;
            match gap {
                Some(gap) if (gap - want).abs() <= 1.5 => {}
                other => failures.push(format!("{case} at zoom {zoom}: {other:?} left after its end, when it is {want}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A small generator (xorshift32), so a failure is the same one every time.
struct Dice(u32);

impl Dice {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as usize % n
    }
}

// yuzora(v010): whatever the lines are cut into, the range ends where the widest one is drawn, which is not always the
// one with the most characters (a piece takes up to a pixel more than its text) nor the one with the most pieces.
// The widest drawn is the first row (the one the window can measure); the rest have more characters, more pieces, or
// both, and the room left after it is the same whichever.
#[gpui::test]
fn yuzora_diff_the_range_ends_where_the_widest_line_is_drawn_whatever_the_lines_are_cut_into(cx: &mut TestAppContext) {
    let mut dice = Dice(2463534242);
    let mut failures = Vec::new();
    for trial in 0..40 {
        let cut = |dice: &mut Dice| {
            let total = 40 + dice.below(300);
            let mut out = Vec::new();
            let mut sum = 0;
            while sum < total {
                let n = [1, 2, 3, 4, 5, 6, 7, 8, 9, 11][dice.below(10)].min(total - sum);
                out.push(n);
                sum += n;
            }
            out
        };
        // What a piece of `n` letters is drawn as wide as at zoom 1: 6.9 pixels a letter, rounded up.
        let drawn = |pieces: &[usize]| pieces.iter().map(|n| (*n as f64 * 6.9).ceil()).sum::<f64>();
        let mut lines: Vec<Vec<usize>> = (0..2 + dice.below(5)).map(|_| cut(&mut dice)).collect();
        lines.sort_by(|a, b| drawn(b).total_cmp(&drawn(a)));
        let rows = Value::Array(lines.iter().map(|pieces| row_in_pieces('a', pieces)).collect());
        let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
        let gap = pane.end_gap("a");
        pane.close();
        match gap {
            Some(gap) if (gap - left_after_the_end()).abs() <= 1.5 => {}
            other => failures.push(format!("trial {trial}, lines of {:?} letters: {other:?} left after the widest one's end", lines.iter().map(|l| l.iter().sum::<usize>()).collect::<Vec<_>>())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): lines in a script the table of widths does not list scroll to where the widest ends. They used to be
// taken at three cells a character when there were more than four lines, though the font draws them in one
// (5 lines of 150 Arabic letters: 498px to scroll, 2570px of room, and the left side empty at the end).
#[gpui::test]
fn yuzora_diff_text_in_a_script_the_table_does_not_list_scrolls_to_where_its_widest_line_ends(cx: &mut TestAppContext) {
    let many = |letter: &str, widest: usize| {
        let mut rows = vec![text_row(Some(&letter.repeat(widest)), Some("b"))];
        rows.extend((0..299).map(|i| text_row(Some(&letter.repeat(widest - 1 - i % 50)), Some("b"))));
        Value::Array(rows)
    };
    let few = |letter: &str| Value::Array((0..5).map(|_| text_row(Some(&letter.repeat(150)), Some("b"))).collect());
    let cases = [
        ("5 lines of Arabic", "\u{645}", few("\u{645}")),
        ("5 lines of Hebrew", "\u{5d0}", few("\u{5d0}")),
        ("5 lines of Thai", "\u{e01}", few("\u{e01}")),
        ("5 lines with check marks", "\u{2713}", few("\u{2713}")),
        ("300 lines of Arabic", "\u{645}", many("\u{645}", 150)),
        ("300 lines of Thai", "\u{e01}", many("\u{e01}", 150)),
    ];
    let mut failures = Vec::new();
    for (case, first, rows) in cases {
        let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
        let gap = pane.end_gap(first);
        pane.close();
        match gap {
            Some(gap) if (gap - left_after_the_end()).abs() <= 1.5 => {}
            other => failures.push(format!("{case}: {other:?} left after the widest line's end, when it is {}", left_after_the_end())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a diff read again (a mode or a context switched, a change to the file) is measured again, though its
// widest lines are the ones it had, also when it is read while its lines are being laid out: the sizes the
// measuring asks for are reported, not taken for the last measuring's. (Here the sizes the first batches are laid out
// to are lost, so they stay in flight, and the diff is read again.)
#[gpui::test]
fn yuzora_diff_a_diff_read_again_while_its_lines_are_being_laid_out_is_measured_again(cx: &mut TestAppContext) {
    let rows = Value::Array((0..5).map(|_| text_row(Some(&"\u{645}".repeat(150)), Some("b"))).collect());
    let mut pane = DiffPane::mount_with(cx, rows, 1200., 1200., &Shared::default(), json!({"drop": 2}));
    assert!(pane.pending(), "the batches are in flight");
    pane.expects_pending = false;
    pane.press(400., 10., 1);
    let gap = pane.end_gap("\u{645}");
    assert!(gap.is_some_and(|gap| (gap - left_after_the_end()).abs() <= 1.5), "{gap:?} left after the widest line's end, when it is {}", left_after_the_end());
    pane.close();
}

// yuzora(v010): sideways wheels that come before the next frame are all counted: each moves the lines from where the
// last one left them, not from where the last frame drew them (the window hands every wheel over at once, and
// a frame is drawn when it is next due; the second used to take the place of the first).
#[gpui::test]
fn yuzora_diff_sideways_wheels_that_come_before_the_next_frame_all_count(cx: &mut TestAppContext) {
    let mut pane = DiffPane::mount(cx, Value::Array(vec![diff_row(Some((300, "a")), Some((5, "b")))]), 1200., &Shared::default());
    let home = pane.start_of("a").unwrap();
    pane.wheels(100., 100., &[(-100., 0.), (-100., 0.), (-50., 0.)]);
    near(home - pane.start_of("a").unwrap(), 250., "three wheels in a frame");
    pane.wheels(100., 100., &[(80., 0.), (0., -40.), (-30., 0.), (60., 0.)]);
    near(home - pane.start_of("a").unwrap(), 250. - 80. - 60. + 30., "back, a vertical one between not counted");
    pane.wheels(100., 100., &[(-100000., 0.), (-100000., 0.)]);
    let far = home - pane.start_of("a").unwrap();
    pane.wheels(100., 100., &[(50., 0.)]);
    near(home - pane.start_of("a").unwrap(), far - 50., "two wheels past the end keep nothing beyond it");
    pane.close();
}

/// A diff of `rows` added lines of 11 to 70 characters.
fn added_lines(rows: usize) -> Value {
    Value::Array((0..rows).map(|i| json!({"k": "~", "h": 0, "r": i + 1, "ri": i, "rt": format!("line {i} of text {}", "x".repeat(i % 60))})).collect())
}

// yuzora(v010): taking a diff in goes through none of its rows, and the frames after it through a slice of them at a time:
// an event has 500 ms, a diff has up to a million rows. Counted (the rows each slice ranked, the reads of their properties),
// not timed. No slice of 60,000 rows or of 200,000 goes through more rows than a slice allows (a machine that is slow
// enough to have a slice cut off ranks fewer, which it is meant to); all the rows are ranked by the end, and the frames in
// between are drawn with the diff in them and read none of them.
#[gpui::test]
fn yuzora_diff_taking_a_diff_in_goes_through_none_of_its_rows_and_the_frames_after_it_through_a_slice_at_a_time(
    cx: &mut TestAppContext,
) {
    let mut failures = Vec::new();
    for rows in [60_000usize, 200_000] {
        let pane = DiffPane::mount(cx, added_lines(rows), 1200., &Shared::default());
        let (shown, ranked) = (pane.reads("shown"), pane.rows_ranked("ranked"));
        let longest = pane
            .notes
            .borrow()
            .iter()
            .filter(|n| n[0] == "ranked")
            .map(|n| n[4].as_f64().unwrap())
            .fold(0., f64::max);
        eprintln!(
            "{rows} rows: {} slices of at most {:?}, the longest {longest} ms",
            ranked.len(),
            ranked.iter().copied().fold(0., f64::max)
        );
        if shown.len() != 1 || shown[0] > 10. {
            failures.push(format!(
                "{rows} rows: taking the diff in read {shown:?} of their properties"
            ));
            pane.close();
            continue;
        }
        if pane.rows_ranked_in_all() < rows as f64 {
            failures.push(format!(
                "{rows} rows: {} of them were ranked in the end",
                pane.rows_ranked_in_all()
            ));
        }
        let frames = frame_costs(&pane);
        if frames.len() < 4 || frames.iter().any(|f| f[0] >= 3000.) {
            failures.push(format!(
                "{rows} rows: frames with the diff in them read its rows: {frames:?}"
            ));
        }
        if pane
            .rendered()
            .iter()
            .filter(|(view, error)| view == "d1" && error.is_null())
            .count()
            < 4
        {
            failures.push(format!(
                "{rows} rows: the diff was drawn fewer than four times: {:?}",
                pane.rendered().last()
            ));
        }
        let most = ranked.iter().copied().fold(0., f64::max);
        if most > 7_000. {
            failures.push(format!("{rows} rows: a slice ranked {most} of them, which a slice of two million units does not"));
        }
        pane.close();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): while the rows are ranked the range is what the lines ranked so far can take at most, so the bar grows
// as the rows go by and never past what it is when they are all ranked (the widest line is the last row here). Before
// the bar was cut to nothing (no lines) or to the whole of it in the frame the diff came in, which it can only be for a
// diff that is ranked in one.
#[gpui::test]
fn yuzora_diff_while_the_rows_are_ranked_the_range_grows_to_what_the_whole_diff_needs(cx: &mut TestAppContext) {
    let mut rows = match added_lines(60_000) { Value::Array(rows) => rows, _ => unreachable!() };
    rows[0] = json!({"k": "~", "h": 0, "r": 1, "ri": 0, "rt": "w".repeat(300)});
    rows.push(json!({"k": "~", "h": 0, "r": 60_001, "ri": 60_000, "rt": "w".repeat(400)}));
    let pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    let (slices, seen) = (pane.reads("ranked").len(), pane.ranges());
    let last = *seen.last().expect("a frame");
    let (first, wanted) = (300. * 6.9 + 6. + 12. - 555.5, 400. * 6.9 + 6. + 12. - 555.5);
    pane.close();
    assert!(slices >= 3, "the rows were ranked in {slices} slices after the first");
    assert!((last - wanted).abs() <= 3., "at the end the lines scroll {last}px, and the widest needs {wanted}");
    let ranking = seen.iter().filter(|reach| **reach < last - 3.).count();
    assert!(ranking >= 3, "the range was short of its end size in {ranking} frames: {seen:?}");
    let before_the_ruler = seen.iter().take_while(|reach| **reach == 0.).count();
    assert!(before_the_ruler <= 3, "{before_the_ruler} frames had no room to scroll before the first row's line was in reach: {seen:?}");
    assert!(seen.iter().skip(before_the_ruler).all(|reach| *reach >= first - 3.), "the first row's line (the widest ranked, {first}px) was out of reach in a frame: {seen:?}");
    assert!(seen.windows(2).take_while(|pair| pair[0] < last - 3.).all(|pair| pair[0] <= pair[1] + 0.5), "the range shrank while the rows were ranked: {seen:?}");
    assert!(seen.iter().all(|reach| *reach <= last + 3.), "the range went past what the whole diff needs: {seen:?}");
}

// yuzora(v010): lines in a script the table of widths does not list scroll to where the widest ends however many
// there are. They are taken at three cells a character till they are laid out, and the laying out stopped after a
// fixed amount, so 1,500 lines of 150 Hebrew letters scrolled 2,570px when 498 reach the end, and the left side was
// empty at the end; 3,000 lines of Arabic subtitles (20 to 45 letters, which fit) gave a bar that scrolled nowhere
// worth going.
#[gpui::test]
fn yuzora_diff_many_lines_in_a_script_the_table_does_not_list_scroll_to_where_the_widest_ends(cx: &mut TestAppContext) {
    let widest = |letter: &str, n: usize, shortest: usize, spread: usize| {
        Value::Array((0..n).map(|i| text_row(Some(&letter.repeat(shortest + (spread - 1 - i % spread))), Some("b"))).collect())
    };
    let mut failures = Vec::new();
    for n in [1000usize, 1500, 3000] {
        let mut pane = DiffPane::mount(cx, widest("\u{5d0}", n, 100, 51), 1200., &Shared::default());
        let (frames, reach) = (pane.asked_frames(), pane.reach());
        let gap = pane.end_gap("\u{5d0}");
        pane.close();
        let wanted = 150. * 6.9 + 6. + 12. - 555.5;
        if !gap.is_some_and(|g| (g - 12.).abs() <= 1.5) || !reach.is_some_and(|r| (r - wanted).abs() <= 3.) {
            failures.push(format!("{n} lines of up to 150 Hebrew letters: scrolls {reach:?} (wanted {wanted}), {gap:?} left after the widest line's end (wanted 12)"));
        }
        if frames > 60 {
            failures.push(format!("{n} lines of Hebrew took {frames} frames to lay out"));
        }
    }
    let mut pane = DiffPane::mount(cx, widest("\u{645}", 3000, 20, 26), 1200., &Shared::default());
    let bar = pane.thumb();
    pane.close();
    if bar.is_some() {
        failures.push(format!("3,000 subtitles of at most 45 Arabic letters fit the side, and there is a bar: {bar:?}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// yuzora(v010): a data file (every line of a deleted file, 90 to 110 characters) past what could be read of a diff in a
// slice scrolls to its widest line, not to three cells a character: 20,000 lines of at most 110 need 221.5px of a side
// of 555.5, and scrolled 1,618, the left side empty at the end.
#[gpui::test]
fn yuzora_diff_a_data_file_of_thousands_of_lines_scrolls_to_its_widest_line(cx: &mut TestAppContext) {
    let mut rows = vec![text_row(Some(&"a".repeat(110)), None)];
    rows.extend((1..20_000).map(|i| text_row(Some(&"a".repeat(90 + (i * 7) % 21)), None)));
    let mut pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    let reach = pane.reach();
    let gap = pane.end_gap("a");
    pane.close();
    assert!(reach.is_some_and(|r| (r - 221.5).abs() <= 3.) && gap.is_some_and(|g| (g - 12.).abs() <= 1.5), "lines of at most 110 characters need 221.5px: scrolls {reach:?}, {gap:?} left after the widest line's end");
}

// yuzora(v010): a single line that costs more than a frame to lay out (40,000 Hebrew letters) is laid out, alone, and
// the range is where it ends; it is not taken at its three cells a letter for good.
#[gpui::test]
fn yuzora_diff_a_line_longer_than_a_frame_is_laid_out_alone_and_scrolled_to_its_end(cx: &mut TestAppContext) {
    let rows = Value::Array(vec![text_row(Some(&"\u{5d0}".repeat(40_000)), Some("b")), text_row(Some("a short one"), Some("b"))]);
    let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
    let gap = pane.end_gap("\u{5d0}");
    pane.close();
    assert!(
        gap.is_some_and(|g| (g - 12.).abs() <= 1.5),
        "{gap:?} left after the end of a line of 40,000 letters, when it is 12"
    );
}

// yuzora(v010): a line drawn in many pieces (250 of 32 letters; each 32 * 6.9 = 220.8px, which the window rounds up to 221,
// 55,250px in all) is laid out in one frame and the range ends where it does. The line is built as 250 elements in the
// frame that lays it out, which the engine cuts off at 50 ms: this machine takes about 3 of them, and a frame that is
// cut is built again smaller, but a line cannot be made smaller than itself. (The first version of this had 1,000 pieces
// and expected 224px a piece, 7px a letter, and was 3,000px off.)
#[gpui::test]
fn yuzora_diff_a_line_in_two_hundred_and_fifty_pieces_is_laid_out_in_a_frame_and_scrolled_to_its_end(
    cx: &mut TestAppContext,
) {
    let mut rows: Vec<Value> = (0..100)
        .map(|_| text_row(Some("a short line"), Some("b")))
        .collect();
    rows.push(row_in_pieces('a', &[32; 250]));
    let pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    let (ranges, pending) = (pane.ranges(), pane.pending());
    let drawn = pane
        .rendered()
        .iter()
        .filter(|(view, error)| view == "d1" && error.is_null())
        .count();
    pane.close();
    let wanted = 250. * (32. * 6.9_f64).ceil() + 6. + 12. - 555.5;
    assert!(
        drawn >= 2 && !pending,
        "drawn {drawn} times, and lines left to lay out: {pending}"
    );
    assert!(
        ranges.last().is_some_and(|r| (r - wanted).abs() <= 1.5),
        "the lines scroll {:?}px, and the widest needs {wanted}",
        ranges.last()
    );
}

// yuzora(v010): a frame lays out at most a frame's worth of lines, and a line that costs more than that is laid out
// alone, its own side's and not the other's with it: two lines of 200,000 characters out of sight (the 201st row) were
// laid out in the first frame, 465,683 characters with the ruler. (The two differ: the window lays out a text once.)
#[test]
fn yuzora_diff_a_frame_lays_out_a_frames_worth_and_a_line_longer_than_that_alone() {
    with_measured_font("yuzora_diff_a_frame_lays_out_a_frames_worth_and_a_line_longer_than_that_alone", |cx| {
        let mut rows: Vec<Value> = (0..200).map(|_| text_row(Some("a short line"), Some("b"))).collect();
        rows.push(text_row(Some(&"\u{e9}".repeat(200_000)), Some(&"\u{17e}".repeat(200_000))));
        let pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
        let (laid, pending) = (pane.laid.borrow().clone(), pane.pending());
        pane.close();
        let most = laid.iter().copied().max().unwrap_or(0);
        assert!(!pending, "the lines were all laid out in the end");
        assert!(most <= 200_000 + 65_536 + 4096, "a frame laid out {most} characters: {laid:?}");
    });
}

// yuzora(v010): the thumb that is held stays under the pointer when the range changes under it (a pane made narrower
// here; lines being laid out do the same while the range settles): the offset follows the thumb, so the pointer
// does not lose it and the lines do not jump back. It used to keep its pixels of offset to a pixel of the pointer's
// travel as they were when it was pressed, and the thumb drew 20px from the pointer after the pane narrowed.
#[gpui::test]
fn yuzora_diff_a_held_thumb_stays_under_the_pointer_when_the_range_changes(cx: &mut TestAppContext) {
    let mut pane = DiffPane::mount(cx, json!([diff_row(Some((300, "a")), Some((5, "b")))]), 1200., &Shared::default());
    let bar = pane.high as f64 - 4.;
    let (start, long) = pane.thumb().expect("a bar");
    let grab = start + long / 2.;
    pane.press(grab, bar, 1);
    pane.drag(grab + 100., bar);
    near(pane.thumb().unwrap().0, start + 100., "the thumb follows the pointer");
    pane.resize(900.);
    let (after, narrower) = pane.thumb().expect("a bar in the narrower pane");
    assert!((narrower - long).abs() > 100., "the thumb is a different length in the narrower pane: {long} and {narrower}");
    near(after, start + 100., "the thumb stays where the pointer holds it when the pane narrows");
    pane.drag(grab + 150., bar);
    near(pane.thumb().unwrap().0, start + 150., "and goes on following the pointer");
    pane.release(grab + 150., bar);
    pane.close();
}

// yuzora(v010): the end of the right side's line is scrolled to when its line is the widest and cut into pieces (the left
// side's lines are plainer and shorter): the pieces each side's lines are cut into are their own, and the half cell
// each piece but the first may be rounded up by is counted for the right side's as for the left's.
#[gpui::test]
fn yuzora_diff_the_right_sides_line_cut_into_pieces_is_scrolled_to_its_end(cx: &mut TestAppContext) {
    let pieces: Vec<Value> = pieces_of(&[3], 360).iter().enumerate().map(|(i, n)| json!({"t": "b".repeat(*n), "h": i % 2 == 1})).collect();
    let text: String = pieces.iter().map(|p| p["t"].as_str().unwrap()).collect();
    let rows = json!([
        {"k": "~", "h": 0, "l": 1, "li": 0, "lt": "a".repeat(5), "r": 1, "ri": 0, "rt": text, "rs": pieces},
        text_row(Some("a"), Some(&"b".repeat(362))),
    ]);
    let mut pane = DiffPane::mount(cx, rows, 1200., &Shared::default());
    let gap = pane.end_gap_right("b");
    pane.close();
    assert!(gap.is_some_and(|g| (g - 12.).abs() <= 1.5), "right side: {gap:?} left after the widest line's end");
}

// yuzora(v010): the widest line is found when it comes in the middle of the batch laid out in a frame: after lines
// that can take more and before lines that can take less (the batch reports the width of its widest, not its first or
// last).
#[gpui::test]
fn yuzora_diff_the_widest_line_in_the_middle_of_a_batch_is_found(cx: &mut TestAppContext) {
    let mut rows = vec![text_row(Some(&"a".repeat(330)), Some("b"))];
    rows.extend((0..5).map(|_| text_row(Some(&"\u{e9}".repeat(300)), Some("b"))));
    rows.extend((0..10).map(|_| row_in_pieces('a', &pieces_of(&[4, 5], 300))));
    rows.extend((0..5).map(|_| text_row(Some(&"a".repeat(320)), Some("b"))));
    let mut pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    let gap = pane.end_gap("a");
    pane.close();
    assert!(gap.is_some_and(|g| (g - 12.).abs() <= 1.5), "{gap:?} left after the widest line's end");
}

// yuzora(v010): a diff read again after it settled keeps the offset it was scrolled to when it is the same, and is
// measured again when it is not: a narrower one leaves no room after its end of the last one's.
#[gpui::test]
fn yuzora_diff_a_diff_read_again_after_it_settled_keeps_its_offset_and_measures_the_next(cx: &mut TestAppContext) {
    let mut pane = DiffPane::mount(cx, json!([text_row(Some(&"a".repeat(300)), Some("b"))]), 1200., &Shared::default());
    pane.wheel(100., 100., -100000., 0.);
    let far = text_start(&mut pane, "a");
    pane.press(400., 10., 1);
    let again = text_start(&mut pane, "a");
    pane.answer_next(vec![json!({"id": "d1", "rows": [text_row(Some(&"a".repeat(150)), Some("b"))], "hunks": [{}], "stageable": true, "add": 1, "del": 0, "leftExists": true, "rightExists": true})]);
    pane.press(400., 10., 1);
    let gap = pane.end_gap("a");
    pane.close();
    near(again, far, "the same diff read again stays where it was scrolled to");
    assert!(gap.is_some_and(|g| (g - 12.).abs() <= 1.5), "a narrower diff read again: {gap:?} left after its end");
}

/// 20,000 changed rows whose left lines (20 to 59 letters) fit a side, and a last one of `widest` letters: the rows
/// are ranked in several slices, and the widest line is not in the first.
fn wide_at_the_end(widest: usize) -> Value {
    let mut rows: Vec<Value> = (0..20_000)
        .map(|i| text_row(Some(&"a".repeat(20 + i % 40)), Some("b")))
        .collect();
    rows.push(text_row(Some(&"a".repeat(widest)), Some("b")));
    Value::Array(rows)
}

/// What `diffView` answers for `rows` when the diff is read again.
fn view_of(rows: Value) -> Value {
    json!({"id": "d1", "rows": rows, "hunks": [{}], "stageable": true, "add": 1, "del": 0, "leftExists": true, "rightExists": true})
}

// yuzora(v010-git-diff-fu): a diff read again (a file saved, a mode or a context switched) keeps the offset it was scrolled to
// in every frame of the ranking that follows, also when the diff takes several slices to rank and the widest line is not in
// the first. The range while the rows are ranked is the widest ranked so far, so the first slice's was 0 and the offset was
// brought within it for as many frames as there were slices, then not given back (6 slices: 13,270px of offset, 0 for six
// frames, 0 for good). Counted in frames and pixels.
#[gpui::test]
fn yuzora_diff_a_diff_of_many_slices_read_again_keeps_its_offset_in_every_frame(
    cx: &mut TestAppContext,
) {
    let mut pane = DiffPane::mount(cx, wide_at_the_end(2000), 1200., &Shared::default());
    assert!(
        pane.reads("ranked").len() >= 3,
        "the rows are ranked in several slices: {:?}",
        pane.reads("ranked")
    );
    pane.wheel(100., 100., -1e7, 0.);
    let before = pane.drawn_since(0).pop().expect("a frame");
    let thumb = pane.thumb().expect("a bar");
    assert!(
        before.0 > 10_000. && (before.0 - before.1).abs() < 1.,
        "scrolled to the end: {before:?}"
    );
    let mark = pane.mark();
    pane.press(400., 10., 1);
    let frames = pane.drawn_since(mark);
    assert!(
        frames.len() >= 5,
        "the diff was ranked again over several frames: {frames:?}"
    );
    let short: Vec<_> = frames
        .iter()
        .filter(|(offset, _)| *offset < before.0 - 1.)
        .collect();
    assert!(
        short.is_empty(),
        "frames drew the lines short of where they were ({} px): {short:?}",
        before.0
    );
    let past: Vec<_> = frames
        .iter()
        .filter(|(offset, reach)| offset > &(reach + 0.5))
        .collect();
    assert!(
        past.is_empty(),
        "frames drew the lines past the end of their range: {past:?}"
    );
    let after = pane.thumb().expect("a bar");
    near(after.0, thumb.0, "the thumb is where it was");
    pane.close();
}

// yuzora(v010-git-diff-fu): the thumb that is held when the diff is read again stays under the pointer: the lines do not go to
// 0 for the frames in which the range is not known and come back. (A key reads the diff again with the button down: a press
// would end the drag.)
#[gpui::test]
fn yuzora_diff_a_held_thumb_does_not_take_the_lines_to_the_start_when_the_diff_is_read_again(
    cx: &mut TestAppContext,
) {
    let mut pane = DiffPane::mount_diff(
        cx,
        Diff::of(wide_at_the_end(2000)),
        1200.,
        1200.,
        &Shared::default(),
        json!({"real": true}),
    );
    pane.click(TEXT_LEFT, row_y(2), false);
    let bar = pane.high as f64 - 4.;
    let (start, long) = pane.thumb().expect("a bar");
    let grab = start + long / 2.;
    pane.press(grab, bar, 1);
    pane.drag(grab + 1100., bar);
    let before = pane.drawn_since(0).pop().expect("a frame");
    assert!(
        before.0 > 10_000.,
        "the thumb dragged to the end: {before:?}"
    );
    let mark = pane.mark();
    pane.keys("f11");
    let frames = pane.drawn_since(mark);
    assert!(
        frames.len() >= 5,
        "the diff was ranked again over several frames: {frames:?}"
    );
    let short: Vec<_> = frames
        .iter()
        .filter(|(offset, _)| *offset < before.0 * 0.99)
        .collect();
    assert!(
        short.is_empty(),
        "frames drew the lines short of where the thumb holds them ({} px): {short:?}",
        before.0
    );
    pane.release(grab + 1100., bar);
    let (end, _) = pane.thumb().expect("a bar");
    near(
        end,
        start + 1100.,
        "and the thumb is at the end of its track",
    );
    pane.close();
}

// yuzora(v010-git-diff-fu): what is read again is narrower, and the offset is brought within the new range once that is known: not
// before (while the rows are ranked the range is a floor, and the old one holds), and then for good.
#[gpui::test]
fn yuzora_diff_a_narrower_diff_read_again_brings_the_offset_within_its_range_once_it_is_known(
    cx: &mut TestAppContext,
) {
    let mut pane = DiffPane::mount(cx, wide_at_the_end(2000), 1200., &Shared::default());
    pane.wheel(100., 100., -1e7, 0.);
    let before = pane.drawn_since(0).pop().expect("a frame");
    let mark = pane.mark();
    pane.answer_next(vec![view_of(wide_at_the_end(300))]);
    pane.press(400., 10., 1);
    let frames = pane.drawn_since(mark);
    let wanted = 300. * 6.9 + 6. + 12. - 555.5;
    let past: Vec<_> = frames
        .iter()
        .filter(|(offset, reach)| offset > &(reach + 0.5))
        .collect();
    assert!(
        past.is_empty(),
        "frames drew the lines past the end of their range: {past:?}"
    );
    assert!(
        frames
            .iter()
            .any(|(offset, _)| *offset > wanted + 100. && *offset > before.0 - 1.),
        "the old offset held while the range was not known: {frames:?}"
    );
    let last = frames.last().copied().expect("a frame");
    near(last.1, wanted, "the range of the new diff");
    near(last.0, wanted, "and the offset is at its end");
    pane.close();
}

// yuzora(v010): a font set while the diff is open measures its lines again, as a zoom does: the lines are laid out in
// the font the rows are drawn in, and the other family's cell is a sixth wider.
#[test]
fn yuzora_diff_a_font_set_while_it_is_open_measures_the_lines_again() {
    with_measured_font("yuzora_diff_a_font_set_while_it_is_open_measures_the_lines_again", |cx| {
        let mut pane = DiffPane::mount(cx, json!([text_row(Some(&"a".repeat(200)), Some("b"))]), 1200., &Shared::default());
        let before = pane.end_gap("a");
        pane.person.borrow_mut()["font_family"] = json!("Other Mono");
        pane.visual.update(|_, cx| pane.runtime.refresh(&pane.root, cx).unwrap());
        pane.settle();
        let after = pane.end_gap("a");
        pane.close();
        assert!(before.is_some_and(|g| (g - 12.).abs() <= 1.5), "before: {before:?}");
        assert!(after.is_some_and(|g| (g - 12.).abs() <= 1.5), "after the font changed: {after:?} left after the end, when it is 12");
    });
}

// yuzora(v010): a zoom set and set back measures again: the ruler of the first zoom is laid out and reported a second time.
#[gpui::test]
fn yuzora_diff_a_zoom_set_and_set_back_measures_the_lines_again(cx: &mut TestAppContext) {
    if !alone("yuzora_diff_a_zoom_set_and_set_back_measures_the_lines_again") {
        return;
    }
    let mut pane = DiffPane::zoomed(cx, Value::Array(vec![row_in_pieces('a', &pieces_of(&[9], 360))]), 1., json!({}));
    for zoom in [1.5, 1.] {
        pane.set_zoom(zoom);
        pane.settle();
    }
    let gap = pane.end_gap("a");
    pane.close();
    assert!(
        gap.is_some_and(|g| (g - 12.).abs() <= 1.5),
        "{gap:?} left after the end"
    );
}

// yuzora(v010): the engine gives an event 500 ms and cuts off what runs past it, and a cut handler asks for nothing: a
// diff of 200,000 rows stopped for good when a slice of the ranking was cut, which it was on a machine a few times slower
// than a laptop. Here the machine is made slow in units, not in time (a `cut_event` prop: the ranking in an event is cut
// off, by the engine itself, past that many units), so it does not depend on this one. The slice after the cut is half
// of what the cut one got done, so there is one cut; the rows are all ranked in the end, the range ends where the widest
// line does (the last row's), and nothing builds a frame once it does.
#[gpui::test]
fn yuzora_diff_a_slice_the_engine_cuts_off_is_followed_by_a_smaller_one_and_the_rows_are_ranked_in_the_end(
    cx: &mut TestAppContext,
) {
    let mut rows = match added_lines(20_000) {
        Value::Array(rows) => rows,
        _ => unreachable!(),
    };
    rows.push(json!({"k": "~", "h": 0, "r": 20_001, "ri": 20_000, "rt": "w".repeat(400)}));
    let mut pane = DiffPane::mount_with(
        cx,
        Value::Array(rows),
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_event": 300_000}),
    );
    let built = pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("slice", "ranked"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 400. * 6.9 + 6. + 12. - 555.5;
    assert!(
        (1..=3).contains(&cuts),
        "slices the engine cut off: one by the test, and a few more if this machine is slow enough to cut another"
    );
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 1.5),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert!(
        built <= 150,
        "{built} frames were built to rank 20,001 rows"
    );
    assert_eq!(quiet, 0, "frames were built after the diff was measured");
}

// yuzora(v010): a row that the engine cuts off by itself (here a line of 60,000 characters, which reading costs 60,000
// units of an event that is cut off past 40,000) is taken at its upper bound, three cells a character, once a slice that
// began with it got nowhere; the rows after it are ranked, and the line, which is short enough to be laid out, is laid out
// and ends the range where it is drawn.
#[gpui::test]
fn yuzora_diff_a_row_the_engine_cuts_off_by_itself_is_taken_at_its_bound_and_the_rest_are_ranked(
    cx: &mut TestAppContext,
) {
    let mut rows: Vec<Value> = (0..5)
        .map(|_| text_row(Some("a short line"), Some("b")))
        .collect();
    rows.push(text_row(Some(&"a".repeat(60_000)), Some("b")));
    rows.extend((0..50).map(|_| text_row(Some("another short line"), Some("b"))));
    let mut pane = DiffPane::mount_with(
        cx,
        Value::Array(rows),
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_event": 40_000}),
    );
    pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("slice", "ranked"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 60_000. * 6.9 + 6. + 12. - 555.5;
    assert!(
        (2..=4).contains(&cuts),
        "slices the engine cut off: the one that began with the row after others, the one that began with it"
    );
    // The window's text system adds its glyphs in f32, 0.06% over at this size: the line is laid out (the range is its width,
    // 414,000px and a little), not taken at three cells a character (1,242,000px).
    assert!(
        range.is_some_and(|r| r >= wanted - 1.5 && r <= wanted + 500.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert_eq!(quiet, 0, "frames were built after the diff was measured");
}

// yuzora(v010): a frame has 50 ms, the engine cuts off a frame past it, and does not build another by itself: the
// batch of lines that a frame lays out out of sight was asked for again, the same, by nothing, and the diff stopped
// with the range of lines it had. A `cut_render` prop makes the machine cut a frame whose batch costs more than that
// many units of laying out. The batch is halved, and it is asked for again a moment after, which is what the clock ticks of
// the test are; the lines are all laid out in the end (200 lines of up to 150 Hebrew letters need 497.5px), and nothing
// builds a frame once they are.
#[gpui::test]
fn yuzora_diff_a_frame_the_engine_cuts_off_is_followed_by_a_smaller_batch_and_the_lines_are_laid_out_in_the_end(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(
        (0..200)
            .map(|i| text_row(Some(&"\u{5d0}".repeat(150 - i % 51)), Some("b")))
            .collect(),
    );
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_render": 3_000}),
    );
    let built = pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("frame", "rendered"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 150. * 6.9 + 6. + 12. - 555.5;
    assert!(
        (1..=3).contains(&cuts),
        "frames the engine cut off: one by the test, and a few more if this machine is slow enough to cut another"
    );
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 3.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert!(
        built <= 80,
        "{built} frames were built to lay out 200 lines"
    );
    assert_eq!(quiet, 0, "frames were built after the lines were laid out");
}

// yuzora(v010): a line that a frame cannot lay out even alone (a `cut_render` under the cost of any line of the left
// side: each of the four is cut, the right side's short lines are not) is passed over, at its upper bound, like a line
// that is too long to be laid out: the range is never short of a line that is drawn, and the diff does not build frames
// for ever. (v010-git-diff-fu: it is cut twice before it is passed over, not once: a frame cut once is no verdict on the line.)
#[gpui::test]
fn yuzora_diff_a_line_the_engine_cuts_off_even_alone_is_passed_over_at_its_bound(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(
        (0..4)
            .map(|_| text_row(Some(&"\u{5d0}".repeat(150)), Some("b")))
            .collect(),
    );
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_render": 100}),
    );
    let built = pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("frame", "rendered"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let drawn = 150. * 6.9 + 6. + 12. - 555.5;
    assert!(
        (8..=10).contains(&cuts),
        "frames the engine cut off: two for each line, which is asked for once more before it is passed over"
    );
    assert!(
        range.is_some_and(|r| r >= drawn - 1.5),
        "the lines scroll {range:?}px, and the widest drawn needs {drawn}"
    );
    assert!(built <= 40, "{built} frames were built for 4 lines");
    assert_eq!(
        quiet, 0,
        "frames were built after the lines were passed over"
    );
}

// yuzora(v010): a frame the engine cuts off after the last thing the diff waited for (a `cut_last` prop: the first frame in
// which nothing is left to find out) is built again. It is the frame that takes the batches out and draws the range
// the lines settled on; with the cut one the pane would stay at the range of the frame before, which is higher (five
// lines of Hebrew are taken at three cells a letter till they are laid out). A machine that stops for a moment cuts any frame.
#[gpui::test]
fn yuzora_diff_the_frame_the_engine_cuts_off_after_the_last_report_is_built_again(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(
        (0..5)
            .map(|_| text_row(Some(&"\u{5d0}".repeat(150)), Some("b")))
            .collect(),
    );
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_last": true}),
    );
    pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("frame", "rendered"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 150. * 6.9 + 6. + 12. - 555.5;
    assert!((1..=3).contains(&cuts), "frames the engine cut off");
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 3.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert_eq!(quiet, 0, "frames were built after the diff was measured");
}

// yuzora(v010-git-diff-fu): a frame the engine cuts off once is not the verdict on the line it was laying out. The first frame
// that lays lines out holds one line, so a machine that stops for a moment in it cut that line, and it was taken at its upper
// bound for good: five lines of 150 Hebrew letters scrolled 2,569px, and 497.5 reach their end (a `cut_first` prop: the first
// frame with a batch is cut, once). The line is laid out again, and is passed over only if it is cut again alone.
#[gpui::test]
fn yuzora_diff_a_frame_cut_off_once_does_not_leave_its_line_at_its_upper_bound(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(
        (0..5)
            .map(|_| text_row(Some(&"\u{5d0}".repeat(150)), Some("b")))
            .collect(),
    );
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_first": true}),
    );
    pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("frame", "rendered"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    let gap = pane.end_gap("\u{5d0}");
    pane.close();
    let wanted = 150. * 6.9 + 6. + 12. - 555.5;
    assert!((1..=2).contains(&cuts), "frames the engine cut off: one by the test, and one more if this machine is slow enough to cut another");
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 3.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert!(
        gap.is_some_and(|g| (g - left_after_the_end()).abs() <= 1.5),
        "{gap:?} left after the widest line's end, when it is {}",
        left_after_the_end()
    );
    assert_eq!(quiet, 0, "frames were built after the lines were laid out");
}

// yuzora(v010-git-diff-fu): a frame is cut by the side whose batch costs more, and the other side's line is not given up with
// it. Here the left line (300 letters, 380 to lay out, with a `cut_render` of 300) is cut each time it is asked for, the
// right one (150 Hebrew letters, 230) is not the cause. The right was taken at its upper bound with the left (2,569px of range,
// when the left line needs 1,532.5).
#[gpui::test]
fn yuzora_diff_a_frame_is_cut_by_the_costlier_side_and_the_other_sides_line_is_still_laid_out(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(vec![text_row(
        Some(&"a".repeat(300)),
        Some(&"\u{5d0}".repeat(150)),
    )]);
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_render": 300}),
    );
    pane.settle_through_cuts(400);
    let (range, quiet) = (pane.ranges().last().copied(), pane.builds_in(10));
    pane.close();
    let wanted = 300. * 6.9 + 6. + 12. - 555.5;
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 3.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert_eq!(
        quiet, 0,
        "frames were built after the lines were passed over"
    );
}

// yuzora(v010-git-diff-fu): a slice the engine cuts off once does not make every slice after it small. One that stopped at 3,000
// units (a machine that stops for a moment) left the slice at 1,500 for good: 20,000 rows took more than 4,000 slices, a frame
// each. The slice grows back after a slice that is not cut, to half what was asked for when it was cut and not past it, so
// a size that was cut is not asked for again (a `stall_slice` prop cuts the first slice that goes 3,000 units in).
#[gpui::test]
fn yuzora_diff_a_slice_cut_off_once_grows_back_but_not_to_the_size_that_was_cut(
    cx: &mut TestAppContext,
) {
    let mut rows = match added_lines(20_000) {
        Value::Array(rows) => rows,
        _ => unreachable!(),
    };
    rows.push(json!({"k": "~", "h": 0, "r": 20_001, "ri": 20_000, "rt": "w".repeat(400)}));
    let mut pane = DiffPane::mount_with(
        cx,
        Value::Array(rows),
        1200.,
        1200.,
        &Shared::default(),
        json!({"stall_slice": 3_000}),
    );
    pane.settle_through_cuts(120);
    let (cuts, slices, range, quiet) = (
        pane.cut("slice", "ranked"),
        pane.rows_ranked("ranked"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let most = slices.iter().copied().fold(0., f64::max);
    let wanted = 400. * 6.9 + 6. + 12. - 555.5;
    assert!((1..=2).contains(&cuts), "slices the engine cut off: one by the test, and one more if this machine is slow enough to cut another");
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 1.5),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert!(
        slices.len() <= 40,
        "{} slices to rank 20,001 rows, when 4 do without a cut",
        slices.len()
    );
    assert!(
        most >= 1_000.,
        "no slice grew back: the largest ranked {most} rows"
    );
    assert!(most <= 3_200., "a slice ranked {most} rows, which is past half of what was asked for when one was cut (2,900 or so)");
    assert_eq!(quiet, 0, "frames were built after the diff was measured");
}

// yuzora(v010-git-diff-fu): a row that a slice is cut off at before it has done a row (a `stall_slice` of one unit: the machine
// stops for a moment at the first row) is read again, not taken at its upper bound for good. The row is 300,000 letters, past
// what is laid out, so the range is its bound: 3 cells a letter when it was given up on, 1 when it is read.
#[gpui::test]
fn yuzora_diff_a_row_cut_off_alone_once_is_read_again_and_not_taken_at_its_upper_bound(
    cx: &mut TestAppContext,
) {
    let mut rows = vec![text_row(Some(&"a".repeat(300_000)), Some("b"))];
    rows.extend((0..5).map(|_| text_row(Some("a short line"), Some("b"))));
    let mut pane = DiffPane::mount_with(
        cx,
        Value::Array(rows),
        1200.,
        1200.,
        &Shared::default(),
        json!({"stall_slice": 1}),
    );
    pane.settle_through_cuts(400);
    let (cuts, range, quiet) = (
        pane.cut("slice", "ranked"),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 300_000. * 6.9 + 6. + 12. - 555.5;
    assert!((1..=2).contains(&cuts), "slices the engine cut off: one by the test, and one more if this machine is slow enough to cut another");
    // The window's text system adds glyphs in f32, 0.06% over at this size: the bound is 1 cell a letter, not 3.
    assert!(range.is_some_and(|r| r >= wanted - 1.5 && r <= wanted * 1.002), "the lines scroll {range:?}px, and the line needs {wanted} (three times that when it is taken at three cells a letter)");
    assert_eq!(quiet, 0, "frames were built after the diff was measured");
}

// yuzora(v010-git-diff-fu): the same for a batch of lines laid out in a frame: after one cut once (a `cut_big` prop: the first
// frame whose batch holds more than 8 lines) the batches grow back, not to the cost that was cut. 2,000 lines of Hebrew, all
// of which are laid out, were 9 to a frame for good, 220 frames.
#[gpui::test]
fn yuzora_diff_a_batch_cut_off_once_grows_back_and_the_lines_are_laid_out_in_a_few_frames(
    cx: &mut TestAppContext,
) {
    let rows = Value::Array(
        (0..2000)
            .map(|i| text_row(Some(&"\u{5d0}".repeat(150 - i % 51)), Some("b")))
            .collect(),
    );
    let mut pane = DiffPane::mount_with(
        cx,
        rows,
        1200.,
        1200.,
        &Shared::default(),
        json!({"cut_big": 8}),
    );
    let built = pane.settle_through_cuts(400);
    let (cuts, batches, range, quiet) = (
        pane.cut("frame", "rendered"),
        pane.batches(),
        pane.ranges().last().copied(),
        pane.builds_in(10),
    );
    pane.close();
    let wanted = 150. * 6.9 + 6. + 12. - 555.5;
    assert!((1..=2).contains(&cuts), "frames the engine cut off: one by the test, and one more if this machine is slow enough to cut another");
    assert!(
        range.is_some_and(|r| (r - wanted).abs() <= 3.),
        "the lines scroll {range:?}px, and the widest needs {wanted}"
    );
    assert!(batches <= 60, "{batches} batches to lay out 2,000 lines");
    assert!(
        built <= 100,
        "{built} frames were built to lay out 2,000 lines"
    );
    assert_eq!(quiet, 0, "frames were built after the lines were laid out");
}

// Not a test: how long taking a diff in takes, and each slice of ranking after it, for the numbers of rows in
// YUZORA_DIFF_ROWS (comma apart), laid out in three ways and side by side or unified (YUZORA_DIFF_LAYOUT=unified).
// `took` is the mod's own clock around `show()` (side by side it ranks nothing; unified it goes through the rows); a slice
// is the same around `ranked()`. The engine gives an event 500 ms.
//   YUZORA_DIFF_ROWS=100000,1000000 cargo test --release --bin herdr-app yuzora_diff_how_long -- --ignored --nocapture
#[gpui::test]
#[ignore = "a timing, run by hand"]
fn yuzora_diff_how_long_taking_a_diff_in_takes(cx: &mut TestAppContext) {
    type Rows = Box<dyn Fn(usize) -> Value>;
    let kinds: Vec<(&str, Rows)> = vec![
        ("an added file", Box::new(added_lines)),
        ("a whole file in context", Box::new(|n| Value::Array((0..n).map(|i| json!({"k": "=", "l": i + 1, "r": i + 1, "t": format!("{}{}", "a".repeat(30), "b".repeat(i % 40))})).collect()))),
        ("changed rows", Box::new(|n| Value::Array((0..n).map(|i| json!({"k": "~", "h": 0, "l": i + 1, "li": i, "lt": format!("let value_{i} = compute({i}, {});", "x".repeat(i % 30)), "r": i + 1, "ri": i, "rt": format!("let value_{i} = compute({i}, {}) + 1;", "y".repeat(i % 30))})).collect()))),
    ];
    let sizes: Vec<usize> = std::env::var("YUZORA_DIFF_ROWS").unwrap_or("10000,100000".into()).split(',').map(|n| n.parse().unwrap()).collect();
    let layout = std::env::var("YUZORA_DIFF_LAYOUT").unwrap_or("side".into());
    for (kind, make) in &kinds {
        for &n in &sizes {
            let pane = DiffPane::mount_with(cx, make(n), 1200., 1200., &Shared::default(), json!({"timed": true, "layout": layout}));
            let took = pane.notes.borrow().iter().filter(|n| n[0] == "took").map(|n| n[1].as_f64().unwrap()).next();
            let slices: Vec<f64> = pane.notes.borrow().iter().filter(|n| n[0] == "ranked").map(|n| n[4].as_f64().unwrap()).collect();
            let drawn = pane.rendered().iter().filter(|(view, error)| view == "d1" && error.is_null()).count();
            eprintln!("TIMING {layout} {kind}, {n} rows: show() {took:?} ms, {} slices after it, the longest {} ms, frames with the diff {drawn}", slices.len(), slices.iter().copied().fold(0., f64::max));
            pane.close();
        }
    }
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
        // yuzora(v0.1.0 E1 r1): which repos were read, for the test of an arrival for a place left.
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
