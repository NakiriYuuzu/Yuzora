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

// yuzora(v010): the shipped Git diff in a window, its herdr and yuzora:git stubbed. A list's rows
// hold no handlers and report no sizes, so `Probed` lays the first row out once more below the
// list: its cells report their widths (`cell`), its texts their width and what the pointer is
// over (`text` and `over`: the text's first character, and the pointer's distance from where the
// text starts), and a press picks the row it landed on, as the list's own click would, in the
// body's text size (a `real` prop leaves that out: the list's own click picks the row). A press in the
// header's empty part loads the view again (`relayout`). `rendered` is noted when a frame was
// drawn whole, `shown` how many times the rows' properties were read in taking the diff in and `work`
// in a frame (the rows are proxies that count), and `list` how tall the rows' list is. A `pane` prop
// puts the diff in a box that wide, so the pointer can leave it.
//
// What a frame costs is counted beyond the rows: the module's built-in objects are frozen, so the string and
// array methods cannot be metered, and what `Probed` counts is every read of the rows and of the array that
// holds them (`reads`), what goes through the module's own scans (`bound` and what ranks the lines: the characters
// and lines they are given, `scans`) and the texts a frame builds and their characters (`built`, in `work`), which
// `shown` and `work` note in that order. The window's text system counts the characters it lays out in each frame
// (`MeasuredFont`). `pending` is noted with each frame: whether the diff still has lines to lay out out of sight, so
// a test draws until it has none.
const DIFF_PROBE: &str = r#"
import { note } from "fixture";
const scanned = { n: 0 }, built = { n: 0 };
if (typeof bound === "function") { const f = bound; bound = (s, ...rest) => { scanned.n += s.length; return f(s, ...rest); }; }
for (const name of ["rank", "widest"]) {
  const f = { rank: typeof rank === "function" && rank, widest: typeof widest === "function" && widest }[name];
  if (f) { const wrap = (rows, ...rest) => { scanned.n += rows.length; return f(rows, ...rest); }; if (name === "rank") rank = wrap; else widest = wrap; }
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
  segments(segs, text, ...rest) {
    built.n += 1 + text.length;
    const el = super.segments(segs, text, ...rest), first = String.fromCodePoint(text.codePointAt(0));
    return this.probing ? el.id(`probe-text-${first}`).on_size_change((s) => note("text", first, s.width)).on_mouse_move((e) => note("over", first, e.local_position.x)) : el;
  }
  rowList(...rest) {
    return div().size_full().on_size_change((s) => note("list", s.height)).child(super.rowList(...rest));
  }
  show(view) {
    const touched = this.touched = { n: 0 };
    const counting = { get(t, k, rc) { touched.n++; return Reflect.get(t, k, rc); } };
    const rows = new Proxy(view.rows.map((r) => new Proxy(r, counting)), counting);
    const before = scanned.n;
    super.show({ ...view, rows });
    note("shown", touched.n, scanned.n - before);
    this.scanned = scanned.n;
  }
  batch(name) {
    const batch = super.batch(name);
    if (batch.lines.length) this.asked = true;
    return batch;
  }
  measured(...rest) {
    if ((this.dropped ?? 0) < (props().drop ?? 0)) { this.dropped = (this.dropped ?? 0) + 1; return; }
    super.measured(...rest);
  }
  render(cx) {
    const before = this.touched?.n ?? 0, scan = this.scanned ?? scanned.n, made = built.n;
    this.asked = false;
    let root = super.render(cx);
    note("work", (this.touched?.n ?? 0) - before, scanned.n - scan, built.n - made);
    this.scanned = scanned.n;
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
    note("pending", this.asked || (this.gauge && this.unit?.key !== this.gauge.key) ? 1 : 0);
    return pane ? div().w(pane).h_full().child(root) : root;
  }
}
"#;

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
    /// The characters the window laid out in each frame drawn by [`Self::settle`] (only in a [`MeasuredFont`] window).
    laid: Vec<usize>,
    /// Whether the diff is meant to have lines to lay out that its frames keep asking for (a `drop` prop loses the
    /// first sizes it is told, so a batch stays in flight).
    expects_pending: bool,
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
        Self::mount_at(cx, rows, pane, (wide, 600.), 1., shared, props)
    }

    /// The diff of `rows` in a 1200px window as tall as it takes to show the probe row at `zoom` (a test that
    /// sets the zoom runs alone, in a process of its own: see [`alone`]).
    fn zoomed(cx: &mut TestAppContext, rows: Value, zoom: f32, props: Value) -> Self {
        gpui_shell::set_pixel_scale(zoom);
        Self::mount_at(cx, rows, 1200., (1200., 1300.), zoom, &Shared::default(), props)
    }

    /// As [`Self::mount_with`], in a window `(wide, high)` big, the zoom set to `zoom` (see [`Self::zoomed`]).
    fn mount_at(cx: &mut TestAppContext, rows: Value, pane: f32, (wide, high): (f32, f32), zoom: f32, shared: &Shared, props: Value) -> Self {
        use gpui::{px, size};
        let mut source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods/yuzora-git-diff/main.js")).unwrap();
        let class = "export default class GitDiff extends View {";
        assert_eq!(source.matches(class).count(), 1, "the diff mod no longer has: {class}");
        source = source.replacen(class, "class GitDiff extends View {", 1) + DIFF_PROBE;
        let fixture = Fixture::new(&source);
        let notes = Rc::new(RefCell::new(Vec::<Value>::new()));
        let hunks = usize::from(rows.as_array().is_some_and(|r| r.iter().any(|r| r.get("h").is_some())));
        let theme = json!({"background": "#101010", "foreground": "#e0e0e0", "border": "#303030", "raised": "#202020", "dim": "#808080", "muted": "#a0a0a0", "accent": "#4080ff", "selected": "#304050", "hover": "#282828", "ansi1": "#f08a8a", "ansi2": "#86d17a"});
        let view = json!({"id": "d1", "rows": rows, "hunks": vec![json!({}); hunks], "stageable": true, "add": 3, "del": 0, "leftExists": true, "rightExists": true});
        let mut herdr = HostModule::new("herdr");
        let opened_drops = props.get("drop").is_some();
        let mut opened = json!({"cwd": "/r", "on": null, "path": "wide-new.ts", "mode": "unstaged", "at": 1});
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
        for name in ["run", "panes", "focused", "closeOverlay"] {
            herdr = herdr.function(name, |_| Ok(HostValue::Null));
        }
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
        let core = HostModule::new("yuzora:core")
            .function("t", |a| Ok(a.string(0)?.into()))
            .function("session", |_| Ok("fixture-run".into()))
            .function("tokens", |_| Ok(to_host(&json!({"diff": {"added": "#86d17a", "removed": "#f08a8a", "added_bg": "#103010", "removed_bg": "#301010"}}))));
        let calls = notes.clone();
        let fixture_module = HostModule::new("fixture").function("note", move |a| {
            calls.borrow_mut().push(Value::Array((0..4).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(HostValue::Null)
        });
        let mut policy = Policy::new();
        for module in [herdr, git, core, fixture_module] {
            policy = policy.with_host_module(module).unwrap();
        }
        gpui_shell::policy::set_default(policy);
        let runtime = ShellRuntime::new_isolated().unwrap();
        let mut mounted = None;
        let window = cx.add_window(|window, cx| {
            let root = runtime.try_load(&fixture.0, window, cx).unwrap();
            mounted = Some(root.clone());
            Mounted(root)
        });
        let visual = VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(size(px(wide), px(high)));
        let mut view = Self { visual, notes, views, wide, high, zoom, person, runtime, root: mounted.unwrap(), extra: 0, laid: Vec::new(), expects_pending: opened_drops, _fixture: fixture };
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
            pane.laid.push(LAID.with(Cell::take));
        };
        for _ in 0..3 {
            draw(self);
        }
        self.extra = 0;
        let limit = if self.expects_pending { 10 } else { 400 };
        while self.pending() && self.extra < limit {
            draw(self);
            self.extra += 1;
        }
        assert!(self.expects_pending || !self.pending(), "the diff still had lines to lay out after {} frames", 3 + self.extra);
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

    /// The lines scrolled as far as they go, and how much room is left after the first row's left text, which
    /// starts with `first`: from its end to its cell's, in window pixels.
    fn end_gap(&mut self, first: &str) -> Option<f64> {
        self.wheel(100., 100., -100000., 0.);
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
        assert!(shown.len() == 1 && shown[0] >= 3000., "{case}: taking the diff in went through its rows: {shown:?}");
        assert!(frames.len() >= 2, "{case}: a frame was drawn with the diff in it: {frames:?}");
        assert!(frames.iter().all(|reads| *reads < 3000.), "{case}: a frame went through the rows: {frames:?}");
        pane.close();
    }
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
        _ => diff_row(Some((20 + i * 37 % 100, "a")), Some((20 + i * 11 % 100, "b"))),
    }));
    let mut pane = DiffPane::mount(cx, Value::Array(rows), 1200., &Shared::default());
    pane.wheel(100., 100., -100000., 0.);
    let end = text_start(&mut pane, "a") + pane.text_width("a");
    let cell = pane.width("l");
    assert!((cell - 30. ..=cell).contains(&end), "scrolled as far as it goes, the longest line ends at {end}, its cell at {cell}");
    pane.close();
}

// yuzora(v010): a diff whose lines cannot be measured stays as it was, and the next one that can be
// opens: what it worked out is kept with the diff only once all of it is. (Here the second answer
// has a line that is not text.)
#[gpui::test]
fn yuzora_diff_a_measure_that_fails_leaves_the_diff_on_show_and_the_next_works(cx: &mut TestAppContext) {
    let view = |id: &str, text: Value| json!({"id": id, "rows": [{"k": "~", "h": 0, "r": 1, "ri": 0, "rt": text}], "hunks": [{}], "stageable": true, "add": 1, "del": 0, "leftExists": true, "rightExists": true});
    let mut pane = DiffPane::mount(cx, json!([diff_row(None, Some((300, "b")))]), 1200., &Shared::default());
    let shows = |pane: &DiffPane| pane.rendered().last().cloned().expect("a frame");
    assert_eq!(shows(&pane), (json!("d1"), Value::Null));
    pane.answer_next(vec![view("broken", json!(["not text"])), view("fine", json!("b".repeat(300)))]);
    pane.press(400., 10., 1);
    let (shown, error) = shows(&pane);
    assert_eq!(shown, json!("d1"), "a diff that cannot be measured does not replace the one on show");
    assert!(error.is_string(), "and says so: {error}");
    pane.redraw();
    assert_eq!(shows(&pane).0, json!("d1"), "drawing it again works");
    pane.press(400., 10., 1);
    assert_eq!(shows(&pane).0, json!("fine"), "the next one opens");
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
            if !(cell - 30. ..=cell + 1.).contains(&end) {
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

/// The first row's left line, `len` letters, is the longest; where `rest` is set, four lines that look longer (a
/// letter with an accent takes more than a cell at most) are laid out instead of it, so its width is the room for
/// what was not.
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

/// How far from that the layout may put it: each length is rounded to a pixel, and a cell is worked out from the width
/// of a run of 256, whose own is rounded up to one, for the lines that were not laid out.
fn room_for_error(len: usize) -> f64 {
    3. + len as f64 / 256.
}

/// What the tests of the zoom open: (case, the letters of the longest line, whether it is laid out).
const LONGEST: [(&str, usize, bool); 3] = [("a line laid out", 300, false), ("a line that was not", 124, true), ("a long line that was not", 1600, true)];

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
        let mut pane = DiffPane::mount_at(cx, long_rows(), 1200., (1200., 1300.), zoom, &shared, json!({}));
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
    with_measured_font("yuzora_diff_a_frame_costs_the_same_whatever_the_number_of_the_diffs_rows", |cx| {
        let mut costs = Vec::new();
        for rows in [2_000, 60_000] {
            let mut pane = DiffPane::mount(cx, wide_lines_then(rows), 1200., &Shared::default());
            pane.redraw();
            let (shown, scans) = (pane.reads("shown"), pane.scans("shown"));
            assert!(shown.len() == 1 && shown[0] >= rows as f64 && scans[0] >= rows as f64, "{rows} rows: taking the diff in went through them: {shown:?} reads, {scans:?} scanned");
            let frames = frame_costs(&pane);
            assert!(frames.len() >= 4, "{rows} rows: frames were drawn with the diff in it: {frames:?}");
            assert!(pane.notes.borrow().iter().any(|n| n[0] == "pending" && n[1].as_f64() == Some(1.)), "{rows} rows: some lines were laid out out of sight");
            assert!(frames.iter().all(|f| f[1] == 0.), "{rows} rows: a frame scanned what the diff came in with: {frames:?}");
            let most = |i: usize| frames.iter().map(|f| f[i]).fold(0., f64::max);
            costs.push([most(0), most(1), most(2), pane.laid.iter().copied().max().unwrap_or(0) as f64]);
            pane.close();
        }
        let (few, many) = (costs[0], costs[1]);
        let names = ["row reads", "scans", "texts built", "characters laid out"];
        for i in 0..4 {
            assert!(many[i] <= few[i] * 1.05 + 50., "a frame of 60,000 rows costs {} {}, of 2,000 {}", many[i], names[i], few[i]);
        }
    });
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
