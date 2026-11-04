//! Shipped JS descriptions and native rendering regressions.
use super::*;
use gpui::TestAppContext;
use gpui_shell::{ShellRuntime, plugin::DescriptionSession, policy::Policy};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};

struct Staged(PathBuf);
impl Staged {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/var/tmp/herdr-render-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let staged = Self(path);
        for (folder, files) in [("mods", DEFAULT_MODS), ("frame", FRAME_MODS)] {
            for (name, bytes) in files {
                let path = staged.0.join(folder).join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        }
        staged
    }
    fn entries(&self) -> Vec<String> {
        let mut entries = Vec::new();
        for folder in ["mods", "frame"] {
            for entry in std::fs::read_dir(self.0.join(folder)).unwrap() {
                let entry = entry.unwrap();
                assert!(entry.path().join("main.js").is_file(), "missing main.js: {:?}", entry.path());
                entries.push(format!("{folder}/{}", entry.file_name().to_str().unwrap()));
            }
        }
        entries.sort();
        entries
    }
    fn wrapper(&self, entry: &str, setup: &str) -> PathBuf {
        let dir = self.0.clone();
        std::fs::write(dir.join("main.js"), format!(
            "import Real from './{entry}/main.js'; export default class Scenario extends Real {{ init(props, cx) {{ super.init(props, cx); {setup} }} }}"
        )).unwrap();
        dir
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Default)]
struct Calls {
    unexpected: Vec<String>,
    effects: Vec<(String, Vec<Value>)>,
    shared: BTreeMap<String, Value>,
    // yuzora(L1): the commands nextCommand() hands out, in order; with none left it never returns.
    commands: Vec<String>,
}

fn updates(app: &str, core: &str) -> Value {
    json!({
        "app": {"version":"0.1.0", "updates":app != "source", "ready": (app == "ready").then_some("0.1.1")},
        "core": {"version":"0.1.5", "ready":(core == "ready").then_some("0.1.6"), "managed":(core == "managed").then_some("homebrew"), "error":(core == "error").then_some("offline"), "updating":core == "updating"},
        "checked":1700000000, "checks":app != "source", "checking":false
    })
}

fn fixture(populated: bool, props: Value, update: Value) -> BTreeMap<String, Value> {
    let program =
        json!({"key":"pi", "name":"Pi", "icon":null, "tint":true, "tile":"#334455", "color":"#aabbcc", "shell":false, "agent":true});
    let mut values = BTreeMap::from([
        ("props".into(), props),
        ("machines".into(), json!([{ "name": null, "layout": "ready" }])),
        ("spaces".into(), json!([])), ("agents".into(), json!([])), ("terminals".into(), json!([])), ("panes".into(), json!([])), ("focused".into(), Value::Null),
        ("layout".into(), serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap()),
        ("layouts".into(), json!(crate::window::shipped_layouts())),
        ("theme".into(), serde_json::to_value(&*crate::settings::theme()).unwrap()),
        ("settings".into(), serde_json::to_value(crate::settings::Settings::default()).unwrap()),
        ("themes".into(), json!([{"name":"graphite", "colors": &*crate::settings::theme()}])),  ("fonts".into(), json!(["Monospace", "Sans"])),
        ("updates".into(), update),
        ("telemetry".into(), json!({"enabled":true,"setting":true,"sends":true,"off_because":null,"endpoint":"https://invalid.example", "draft":{},"queued":[],"last_sent":null})),
        ("window".into(), json!({"platform":"linux","titlebar":"none","buttons":true,"maximized":false,"fullscreen":false,"active":true})),
        ("placement".into(), Value::Null),
        ("home".into(), json!("/fixture")), ("appHome".into(), json!("/fixture/app")),
        ("keys".into(), Value::Array(serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>(include_str!("../../defaults/keys.json")).unwrap().into_iter().flat_map(|(section, bindings)| bindings.into_iter().map(move |(keys, action)| json!({"section":section,"keys":keys,"action":action}))).collect())),
        ("commands".into(), Value::Array(crate::commands::COMMANDS.iter().map(|c| json!({"name":c.name,"about":c.about,"args":c.args.iter().map(|a| json!({"name":a.name,"required":a.required})).collect::<Vec<_>>()})).collect())),
    ]);
    if populated {
        values.insert("spaces".into(), json!([
            {"id":"s1","index":1,"name":"Local","folder":"/fixture/project","machine":null,"branch":"main","active":true,"status":"working","tabs":[{"id":"tab1","index":1,"name":"Build","active":true,"status":"working","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":1700000000000i64,"working_since":1700000000000i64}]},
            {"id":"s2","index":2,"name":"Remote","folder":"/remote/project","machine":"remote","branch":null,"active":false,"status":"done","tabs":[{"id":"tab2","index":1,"name":"Done","active":true,"status":"done","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":null,"working_since":null}]}
        ]));
        values.insert("panes".into(), json!([{"pane":"p1","terminal":"t1","tab":"tab1","title":"Build","cwd":"/fixture/project","branch":"main","status":"working","app":"pi","program":program,"focused":true,"zoomed":false}, {"pane":"p2","terminal":"t2","tab":"tab2","title":"Remote task","cwd":"/remote/project","branch":null,"status":"done","app":"pi","program":program,"focused":false,"zoomed":false}]));
        values.insert("terminals".into(), json!([{"address":"t1","name":"Build","title":"Build","argv":["pi"],"parent":null,"labels":{},"state":"running","status":"working","report":{"app":"pi"},"program":program,"cwd":"/fixture/project"}, {"address":"t2","name":"Remote task","title":"Remote task","argv":["pi"],"parent":null,"labels":{},"state":"running","status":"done","report":{"app":"pi"},"program":program,"cwd":"/remote/project"}]));
        values.insert("agents".into(), json!([{"address":"t1","pane":"p1","space":"s1","space_name":"Local","tab":"tab1","tab_index":1,"tab_name":"Build","status":"working","app":"pi","program":program,"title":"Build","seq":1,"focused":true}, {"address":"t2","pane":"p2","space":"s2","space_name":"Remote","tab":"tab2","tab_index":1,"tab_name":"Done","status":"done","app":"pi","program":program,"title":"Remote task","seq":2,"focused":false}]));
        values.insert(
            "focused".into(),
            json!({"space":"s1","tab":"tab1","pane":"p1","terminal":"t1","agent":"t1","shown":null,"cwd":"/fixture/project"}),
        );
    }
    values
}

fn module(values: BTreeMap<String, Value>, calls: Rc<RefCell<Calls>>) -> HostModule {
    let mut module = HostModule::new("herdr");
    for (name, value) in values {
        module = module.function(name, move |_| Ok(to_host(&value)));
    }
    module = module.function("feedbackPreview", |a| {
        Ok(to_host(
            &json!({"available":true,"body":{"message":a.string(0)?,"email":a.string(1)?}}),
        ))
    });
    let shared = calls.clone();
    module = module.function("shared", move |a| {
        Ok(shared.borrow().shared.get(a.string(0)?).map(to_host).unwrap_or(HostValue::Null))
    });
    let shared = calls.clone();
    module = module.function("setShared", move |a| {
        shared.borrow_mut().shared.insert(a.string(0)?.into(), from_host(a.value(1)?));
        Ok(HostValue::Null)
    });
    for name in ["run", "playSound", "setAgentOrder", "openOverlay", "closeOverlay", "notify", "presentUpdateNotes"] {
        let calls = calls.clone();
        module = module.function(name, move |a| {
            if name == "run" {
                let command = a.string(0)?;
                let command = crate::commands::find(command).unwrap_or_else(|| panic!("unknown shipped mod command {command}"));
                let args = a.get(1).map(from_host).unwrap_or(Value::Null);
                crate::commands::validate_args(command, &args).unwrap();
            }
            calls
                .borrow_mut()
                .effects
                .push((name.into(), (0..2).filter_map(|i| a.get(i).map(from_host)).collect()));
            Ok(if name == "openOverlay" || name == "notify" {
                "fixture-overlay".into()
            } else {
                HostValue::Null
            })
        });
    }
    for name in ["feedbackPasteImage", "feedbackClearImages", "feedbackAttachImages"] {
        module = module.function(name, |_| Ok(HostValue::Null));
    }
    module = module.async_function("feedbackPickImages", |_| Ok(async { Ok(HostValue::Array(vec![])) }));
    module = module.async_function("knownAgents", |_| Ok(async { Ok(HostValue::Array(vec![])) }));
    module = module.async_function("pickFolder", |_| Ok(async { Ok(HostValue::Null) }));
    module = module.async_function("watchFolder", |_| Ok(std::future::pending::<gpui_shell::HostResult>()));
    // yuzora(L1): nextCommand() hands out Calls.commands (upstream: it never returned).
    let queued = calls.clone();
    module = module.async_function("nextCommand", move |_| {
        let next = {
            let mut calls = queued.borrow_mut();
            (!calls.commands.is_empty()).then(|| calls.commands.remove(0))
        };
        Ok(async move {
            match next {
                Some(command) => Ok(HostValue::from(command)),
                None => std::future::pending::<gpui_shell::HostResult>().await,
            }
        })
    });
    for name in ["exec", "hc", "readTextFile", "writeTextFile", "sendFeedback"] {
        let calls = calls.clone();
        module = module.async_function(name, move |a| {
            let first = a.get(0).map(from_host).unwrap_or(Value::Null);
            let reply = match name {
                "readTextFile" if first == "/fixture/app/settings.json" || first == "/fixture/.ssh/config" => Some(Value::Null),
                "readTextFile" => first
                    .as_str()
                    .and_then(|p| p.strip_prefix("/fixture/app/layouts/"))
                    .and_then(|p| p.strip_suffix(".json"))
                    .filter(|n| crate::window::shipped_layouts().contains(n))
                    .map(|_| json!("{}")),
                "hc" if first == json!(["machine", "ls", "--json"]) => {
                    Some(json!({"code":0,"stdout":"{\"machines\":[{\"name\":\"remote\",\"state\":\"online\"}]}","stderr":""}))
                }
                "hc" if first.as_array().and_then(|a| a.first()).and_then(Value::as_str) == Some("run") => {
                    Some(json!({"code":0,"stdout":"{\"address\":\"t-setup\"}","stderr":""}))
                }
                // The files panel lists a folder with sh: kind and name pairs.
                "exec" if first == "sh" && a.get(1).map(from_host).and_then(|v| v.get(2).cloned()) == Some(json!("files")) => {
                    Some(json!({"code":0,"stdout":"d\u{0}src\u{0}f\u{0}README.md\u{0}","stderr":""}))
                }
                "exec"
                    if first == "git"
                        && [
                            json!(["rev-parse", "--show-toplevel"]),
                            json!(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
                            json!(["status", "--porcelain=v2", "--branch"]),
                            json!(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch"]),
                            json!(["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"]),
                            json!(["rev-parse", "--abbrev-ref", "HEAD"]),
                        ]
                        .contains(&a.get(1).map(from_host).unwrap_or(Value::Null)) =>
                {
                    Some(json!({"code":128,"stdout":"","stderr":"fixture: not a git repository"}))
                }
                _ => None,
            };
            let result = if let Some(reply) = reply {
                Ok(to_host(&reply))
            } else {
                let message = format!("unexpected {name}: {first}, {:?}", a.get(1));
                calls.borrow_mut().unexpected.push(message.clone());
                Err(HostError::new(message))
            };
            Ok(std::future::ready(result))
        });
    }
    module.declarations(DECLARATIONS)
}

struct Empty;
impl gpui::Render for Empty {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

fn describe(
    cx: &mut TestAppContext,
    staged: &Staged,
    entry: &str,
    populated: bool,
    props: Value,
    update: Value,
    setup: Option<&str>,
) -> anyhow::Result<String> {
    describe_with_values(cx, staged, entry, fixture(populated, props, update), setup)
}

fn describe_with_values(
    cx: &mut TestAppContext,
    staged: &Staged,
    entry: &str,
    values: BTreeMap<String, Value>,
    setup: Option<&str>,
) -> anyhow::Result<String> {
    let calls = Rc::new(RefCell::new(Calls::default()));
    static NEXT_STORE: AtomicU64 = AtomicU64::new(0);
    let policy = Policy::new()
        .with_storage_path(staged.0.join(format!("store-{}.json", NEXT_STORE.fetch_add(1, Ordering::Relaxed))))
        .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
        .with_host_module(module(values, calls.clone()))?;
    let policy = crate::yuzora::with_modules(policy, ""); // yuzora: shipped yuzora-* mods import yuzora:*
    let runtime = ShellRuntime::new_isolated()?;
    let root = setup.map(|s| staged.wrapper(entry, s)).unwrap_or_else(|| staged.0.join(entry));
    let window = cx.add_window(|_, _| Empty);
    let result = window.update(cx, |_, window, cx| {
        let mut session = DescriptionSession::load(runtime, root, policy, window, cx)?;
        let first = session.describe(window, cx)?;
        let second = session.describe(window, cx)?;
        assert!(!first.is_empty() && !second.is_empty(), "empty description: {entry}");
        Ok(second)
    })?;
    window.update(cx, |_, window, _| window.remove_window())?;
    assert!(calls.borrow().unexpected.is_empty(), "{entry}: {:?}", calls.borrow().unexpected);
    result
}

fn props() -> Value {
    json!({"overlay":"o1","pane":"p1","terminal":"t1","space":"s1","tab":"pane","target":{"pane":"p1","terminal":"t1","id":"s1","space":"s1"},"title":"Fixture","value":"Name","message":"A prefilled message","command":{"name":"space.rename","args":{"id":"s1"}},"notification":{"title":"Finished","body":"Build done","action":{"name":"app.help"}},"action":"new"})
}

#[gpui::test]
fn shipped_initial_descriptions(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    let entries = staged.entries();
    let mut count = 0;
    for entry in &entries {
        for populated in [false, true] {
            describe(cx, &staged, entry, populated, props(), updates("current", "current"), None)
                .unwrap_or_else(|e| panic!("{entry}/{}: {e:#}", if populated { "populated" } else { "empty" }));
            count += 1;
        }
    }
    eprintln!(
        "{count} initial-description cases ({} entries, each twice): {:?}",
        entries.len(),
        start.elapsed()
    );
}

#[gpui::test]
fn brand_leaves_room_for_macos_traffic_lights_only_outside_full_screen(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (fullscreen, inset) in [(false, true), (true, false)] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("window".into(), json!({"platform":"macos","titlebar":"none","fullscreen":fullscreen,"buttons":false}));
        let tree = describe_with_values(cx, &staged, "mods/workspace-chrome", values, None).unwrap();
        let root = tree.lines().next().unwrap();
        assert_eq!(root.contains(".pl[Number(76.0)]"), inset, "fullscreen={fullscreen}: {tree}");
    }
}

#[gpui::test]
fn tabs_leave_room_for_macos_traffic_lights(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (platform, titlebar, fullscreen, width, lights, zoom, expected) in [
        ("macos", "none", false, 44, true, 1.0, 32),
        ("macos", "none", false, 44, true, 0.5, 108),
        ("macos", "none", false, 44, true, 2.0, 10),
        ("macos", "none", false, 60, true, 1.0, 16),
        ("macos", "none", false, 76, true, 1.0, 10),
        ("macos", "none", false, 268, true, 1.0, 10),
        ("macos", "none", true, 44, true, 1.0, 10),
        ("macos", "system", false, 44, true, 1.0, 10),
        ("macos", "none", false, 44, false, 1.0, 10),
        ("linux", "none", false, 44, true, 1.0, 10),
        ("windows", "none", false, 44, true, 1.0, 10),
    ] {
        let mut values = fixture(true, props(), updates("current", "current"));
        values.insert("window".into(), json!({"platform":platform,"titlebar":titlebar,"fullscreen":fullscreen,"buttons":false}));
        values.get_mut("settings").unwrap()["zoom"] = json!(zoom);
        let layout = values.get_mut("layout").unwrap();
        layout["children"][0]["size"]["px"] = json!(width);
        if !lights {
            layout.as_object_mut().unwrap().remove("traffic_lights");
        }
        let tree = describe_with_values(cx, &staged, "mods/tabs", values, None).unwrap();
        let root = tree.lines().next().unwrap();
        assert!(root.contains(&format!(".pl[Number({expected}.0)]")), "{platform}/{titlebar}/{fullscreen}/{width}/{lights}: {tree}");
        assert!(tree.contains("tab-tab1"), "first tab must remain visible: {tree}");
    }
}

#[gpui::test]
fn windows_tab_buttons_use_matching_vector_icons(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for platform in ["windows", "linux"] {
        for maximized in [false, true] {
            let mut values = fixture(true, props(), updates("current", "current"));
            values.insert("window".into(), json!({"platform":platform,"buttons":true,"maximized":maximized}));
            let tree = describe_with_values(cx, &staged, "mods/tabs", values, None).unwrap();
            if platform == "windows" {
                for icon in ["win-min.svg", if maximized { "win-restore.svg" } else { "win-max.svg" }, "win-close.svg"] {
                    assert!(tree.contains(icon), "missing {icon}: {tree}");
                }
            } else {
                assert!(!tree.contains("mods/tabs/win-"), "{tree}");
                assert!(tree.contains("mods/pane/close.svg"), "{tree}");
            }
        }
    }
}

#[gpui::test]
fn font_pickers_contain_scroll_on_the_wrapper(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for which in ["font", "ui_font"] {
        let mut p = props();
        p["section"] = json!("look");
        let tree = describe(
            cx, &staged, "frame/settings", true, p, updates("current", "current"),
            Some(&format!("this.picking = '{which}';")),
        ).unwrap();
        let scroll = tree.lines().find(|line| line.contains(&format!("st-fl-{which}"))).unwrap();
        assert!(scroll.contains("overflow_y_scrollbar"), "{tree}");
        assert!(!scroll.contains("on_scroll_wheel"), "the native scroller must receive the event: {tree}");
        assert_eq!(tree.matches("on_scroll_wheel").count(), 1, "the wrapper must stop scroll chaining: {tree}");
    }
}

#[gpui::test]
fn check_materializes_retained_elements_in_a_rendered_view(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for body in [
        "v_flex().h(400).child(v_virtual_list('rows', 0, 28, (i) => 'a', () => { throw new Error('deferred rows ran'); }))",
        "v_flex().h(400).child(v_virtual_list('rows', 1, 28, (i) => 'a', () => { throw new Error('deferred rows ran'); }))",
        "div().w(100).h(50).left(10).transition('left', 200)",
        "div().w(100).h(50).transition('width', 200)",
        "div().w(100).h(50).left(10).spring('left')",
        "div().opacity(0.5).transition('opacity', 200)",
    ] {
        std::fs::write(staged.0.join("main.js"), format!(
            "import {{ View, div }} from 'gpui-kit'; import {{ v_flex, v_virtual_list }} from 'gpui-base'; export default class Demo extends View {{ render() {{ return {body}; }} }}"
        )).unwrap();
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|_, _| Empty);
        let description = window.update(cx, |_, window, cx| runtime.check(&staged.0, window, cx)).unwrap().unwrap();
        assert!(!description.is_empty(), "{body}");
        gpui::AnyWindowHandle::from(window).update(cx, |_, window, _| window.remove_window()).unwrap();
    }
}

#[gpui::test]
fn rebuilt_machine_offers_connect_only_for_identity_mismatch(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (state, why, offered) in [
        ("unreachable", "a different core answers there (\"new-id\")", true),
        ("unreachable", "connection refused", false),
        ("unreachable", "a different core answers there", false),
        ("connected", "a different core answers there (\"new-id\")", false),
    ] {
        let setup = format!(
            "this.machines = [{{name:'cloud', state:{}, why:{}, target:'host'}}]; this.folded.add('cloud');",
            json!(state), json!(why)
        );
        let tree = describe(cx, &staged, "mods/spaces", false, props(), updates("current", "current"), Some(&setup)).unwrap();
        assert_eq!(tree.contains("machine-connect-cloud"), offered, "{state}/{why}: {tree}");
        assert_eq!(tree.contains("rebuilt ·"), offered, "{tree}");
        assert_eq!(tree.contains("This machine was rebuilt or replaced. Click to connect to the new one."), offered, "{tree}");
        assert!(!tree.contains("machine-reconnect-"), "reconnect belongs in the machine row: {tree}");
    }
}

#[gpui::test]
fn machine_picker_keeps_long_lists_scrollable(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = describe(
        cx, &staged, "mods/machine-add", false, props(), updates("current", "current"),
        Some("this.hosts = Array.from({length: 30}, (_, i) => ({alias: `host-${i}`, aliases: []}));"),
    ).unwrap();
    let scroll = tree.lines().find(|line| line.contains("ma-hosts")).unwrap();
    assert!(scroll.contains("overflow_y_scroll") && !scroll.contains("overflow_y_scrollbar"), "{tree}");
    assert!(scroll.contains(".min_h[Number(0.0)]"), "{tree}");
    assert!(tree.contains("ma-row-host-29"), "{tree}");
    assert_eq!(tree.matches(".flex_none").count(), 60, "rows must not shrink into the viewport: {tree}");
}

#[gpui::test]
fn machine_setup_prompt_stacks_above_the_picker(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let calls = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new().with_host_module(module(fixture(false, props(), updates("current", "current")), calls.clone())).unwrap();
    let root = staged.wrapper("mods/machine-add", "cx.spawn(async () => { await this.inTerminal('host'); });");
    let window = cx.add_window(|_, _| Empty);
    let mut session = window.update(cx, |_, window, cx| {
        DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap()
    }).unwrap();
    cx.run_until_parked();
    window.update(cx, |_, window, cx| {
        session.describe(window, cx).unwrap();
        window.remove_window();
    }).unwrap();
    let calls = calls.borrow();
    assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
    let spec = &calls.effects.iter().find(|(name, _)| name == "openOverlay").unwrap().1[0];
    assert_eq!(spec["kind"], "dialog");
    assert_eq!(spec["layout"]["children"][1]["content"]["terminal"], "t-setup");
    assert!(!calls.effects.iter().any(|(name, _)| name == "closeOverlay"), "the picker must keep monitoring setup");
}

#[gpui::test]
fn option_key_settings_are_only_shown_on_macos(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for platform in ["linux", "windows", "macos"] {
        let mut p = props();
        p["section"] = json!("keyboard");
        let mut values = fixture(true, p, updates("current", "current"));
        values.insert("window".into(), json!({"platform":platform}));
        let tree = describe_with_values(cx, &staged, "frame/settings", values, None).unwrap();
        for value in ["auto", "on", "off", "left", "right"] {
            assert_eq!(tree.contains(&format!("st-option-{value}")), platform == "macos", "{platform}: {tree}");
        }
        assert_eq!(tree.contains("st-s-keyboard"), platform == "macos", "{platform}: {tree}");
    }
}

#[gpui::test]
fn settings_sections_and_updates(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    for section in ["look", "alerts", "privacy", "updates"] {
        let mut p = props();
        p["section"] = json!(section);
        describe(cx, &staged, "frame/settings", true, p, updates("current", "current"), None).unwrap();
    }
    for app in ["source", "current", "ready"] {
        for core in ["current", "ready", "error", "managed", "updating"] {
            let mut p = props();
            p["section"] = json!("updates");
            let tree = describe(cx, &staged, "frame/settings", true, p, updates(app, core), None)
                .unwrap_or_else(|e| panic!("updates/{app}/{core}: {e:#}"));
            if app == "current" && core == "current" {
                assert!(
                    tree.contains("check-app") && tree.contains("check-core"),
                    "both update buttons must exist: {tree}"
                );
            }
        }
    }
    for (checking, checked, asked) in [
        (false, None, false), (false, None, true),
        (true, None, false), (true, None, true),
        (true, Some(1700000000u64), false), (true, Some(1700000000), true),
        (false, Some(1700000000), false), (false, Some(1700000000), true),
    ] {
        let mut p = props();
        p["section"] = json!("updates");
        let mut u = updates("current", "current");
        u["checked"] = json!(checked);
        u["checking"] = json!(checking);
        let setup = asked.then_some("this.asked = Date.now();");
        let tree = describe(cx, &staged, "frame/settings", true, p, u, setup).unwrap();
        let succeeded = !checking && checked.is_some();
        assert_eq!(tree.contains("Up to date"), succeeded, "success requires a completed, successful check: {tree}");
        if !checking && checked.is_none() {
            assert!(tree.contains("check-app") && tree.contains("check-core"), "failed checks must remain retryable: {tree}");
        }
        assert!(tree.contains(if checking { "Checking" } else if succeeded { "Up to date" } else { "Check your connection" }), "check status missing: {tree}");
    }
    eprintln!("27 settings cases, each twice: {:?}", start.elapsed());
}

#[gpui::test]
fn animated_images_advance_with_explicit_and_implicit_ids(cx: &mut TestAppContext) {
    use gpui::{IntoElement as _, http_client::{AsyncBody, FakeHttpClient, Response}};
    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};
    use std::{sync::Arc, time::Duration};

    cx.update(gpui_shell::init);
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder.encode_frame(Frame::from_parts(RgbaImage::from_pixel(2, 2, Rgba(color)), 0, 0, Delay::from_numer_denom_ms(20, 1))).unwrap();
        }
    }
    let client = FakeHttpClient::create(move |_| {
        let bytes = bytes.clone();
        async move { Ok(Response::builder().status(200).body(AsyncBody::from(bytes)).unwrap()) }
    });
    cx.update(|cx| cx.set_http_client(client));
    let staged = Staged::new();
    for explicit in [false, true] {
        let runtime = ShellRuntime::new_isolated().unwrap();
        let url = format!("https://images.example/animation-{explicit}.gif");
        let source = format!("import {{ View, image }} from 'gpui-kit'; export default class Demo extends View {{ render() {{ return image('{url}'){}.size(20); }} }}", if explicit { ".id('clip')" } else { "" });
        std::fs::write(staged.0.join("main.js"), source).unwrap();
        let context = cx.add_empty_window();
        let view = context.update(|window, cx| {
            window.activate_window();
            let root = runtime.try_load(&staged.0, window, cx).unwrap();
            root.read(cx).content().clone()
        });
        let draw = |context: &mut gpui::VisualTestContext| {
            let view = view.clone();
            context.draw(gpui::Point::default(), gpui::size(gpui::px(40.), gpui::px(40.)), move |_, _| view.into_any_element());
        };
        draw(context);
        context.run_until_parked();
        draw(context);
        assert!(context.update(|window, _| window.is_window_active()));
        let resource = gpui::Resource::Uri(url.into());
        let data: Arc<gpui::RenderImage> = context.update(|_, cx| cx.fetch_asset::<gpui::ImgResourceLoader>(&resource).unwrap().unwrap());
        assert_eq!(data.frame_count(), 2);
        assert!(!context.update(|window, _| window.has_image_atlas_entry(&data)));
        // GPUI's image clock is wall time, not the test executor's clock.
        std::thread::sleep(Duration::from_millis(25));
        draw(context);
        assert!(context.update(|window, _| window.has_image_atlas_entry(&data)), "both animation frames must have painted (explicit id: {explicit})");
    }
}

#[gpui::test]
fn settings_keep_notes_in_the_whats_new_window(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let notes = json!([
        {"version":"1.8.0","date":"2026-10-26","items":[{"headline":"Newest headline","detail":"Newest details","fixed":false,"media":[]},{"headline":"","detail":"Newest fix","fixed":true,"media":[]}],"spotlight":true},
        {"version":"1.7.0","date":"2026-10-25","items":[{"headline":"Older headline","detail":"Older details","fixed":false,"media":[]},{"headline":"Another older headline","detail":"Hidden detail","fixed":false,"media":[]}],"spotlight":true}
    ]);
    let mut update = updates("ready", "current");
    update["notes"] = notes.clone();
    let mut p = props();
    p["section"] = json!("updates");
    let tree = describe(cx, &staged, "frame/settings", true, p, update.clone(), None).unwrap();
    assert!(tree.contains("st-whats-new") && tree.contains("st-app-update") && tree.contains("st-check-core"));
    assert!(!tree.contains("Newest details") && !tree.contains("Older headline"));
    let mut p = props();
    p["notes"] = notes;
    let tree = describe(cx, &staged, "frame/whatsnew", true, p, update, None).unwrap();
    assert!(tree.contains("Newest details") && tree.contains("Older details"));
    assert!(tree.contains("Also fixed") && tree.contains("Newest fix"));
    assert!(tree.contains("Oct 26") && !tree.contains("2026-10-26"));
}

#[gpui::test]
fn local_state_and_overlay_initial_descriptions(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let start = Instant::now();
    let staged = Staged::new();
    for page in 1..=4 {
        let tree = describe(
            cx,
            &staged,
            "frame/welcome",
            true,
            props(),
            updates("current", "current"),
            Some(&format!("this.page = {page};")),
        )
        .unwrap();
        // yuzora(L1): the welcome says Yuzora.
        let expected = ["Yuzora runs your terminals in the background", "Add your other machines", "Choose your panels", "Make it yours"][page - 1];
        assert!(tree.contains(expected), "welcome page {page}: {tree}");
    }
    for setup in ["", "this.sent = true;", "this.error = 'Fixture error';"] {
        let tree = describe(
            cx,
            &staged,
            "frame/feedback",
            true,
            props(),
            updates("current", "current"),
            Some(setup),
        )
        .unwrap();
        assert!(tree.contains("feedback"), "{tree}");
        if setup.is_empty() {
            assert!(
                tree.contains("Input #") && tree.contains("Textarea #") && tree.contains("A prefilled message"),
                "{tree}"
            );
        } else {
            assert!(
                tree.contains(if setup.contains("sent") { "Sent. Thanks" } else { "Fixture error" }),
                "{tree}"
            );
        }
    }
    for query in ["settings", "no-such-command"] {
        let tree = describe(
            cx,
            &staged,
            "frame/palette",
            true,
            props(),
            updates("current", "current"),
            Some(&format!("this.query = {query:?};")),
        )
        .unwrap();
        // The query sits in a native text field; what the list shows tells it apart.
        let expected = if query == "settings" { "Settings" } else { "Nothing matches" };
        assert!(tree.contains(expected), "palette query {query}: {tree}");
    }
    // yuzora(L1): the palette has no "Next layout" or any other entry for a bare layout.switch
    // (it lands in Herdr's own layout from the yuzora one); "Layout: <name>" for a named one stays.
    for (query, listed) in [("next layout", false), ("switch layout", false), ("layout: herdr", true)] {
        let tree = describe(cx, &staged, "frame/palette", true, props(), updates("current", "current"), Some(&format!("this.query = {query:?};"))).unwrap();
        assert_eq!(!tree.contains("Nothing matches"), listed, "palette query {query}: {tree}");
    }
    // Reviewed overlay contract table, not extracted from arbitrary JS call sites.
    let scenarios = [
        ("mods/menu", json!({"tab":"pane","target":{"pane":"p1","terminal":"t1"}})),
        ("mods/menu", json!({"tab":"tab","target":{"id":"tab1","space":"s1"}})),
        ("mods/menu", json!({"tab":"space","target":{"id":"s1"}})),
        ("mods/prompt", json!({"machines":true})),
        (
            "mods/confirm",
            json!({"title":"Close space?","command":{"name":"space.close","args":{"id":"s1"}}}),
        ),
        (
            "mods/toast",
            json!({"title":"Done","body":"Build complete","level":"success","action":{"name":"app.help"}}),
        ),
        ("mods/peek", json!({"terminal":"t1"})),
        ("mods/peek", json!({"terminal":"missing"})),
        ("mods/worktree", json!({"space":"s1","action":"new"})),
        ("mods/worktree", json!({"space":"s1","action":"remove"})),
    ];
    let entries = staged.entries();
    for (entry, patch) in scenarios {
        assert!(entries.iter().any(|e| e == entry), "orphan scenario: {entry}");
        let mut p = props();
        p.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        describe(cx, &staged, entry, true, p, updates("current", "current"), None).unwrap();
    }
    eprintln!("18 local-state/overlay cases, each twice: {:?}", start.elapsed());
}

#[gpui::test]
fn duplicate_element_negative_control(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let dir = staged.0.join("negative");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("main.js"), "import { View, div } from 'gpui-kit'; export default class Bad extends View { render() { const child = div().child('reused'); return div().child(div().child(child)).child(div().child(child)); } }").unwrap();
    let error = describe(cx, &staged, "negative", false, props(), updates("current", "current"), None).unwrap_err();
    assert!(format!("{error:#}").contains("already added to a parent"), "{error:#}");
}

#[test]
fn shipped_layouts_and_keys() {
    let staged = Staged::new();
    let mods: BTreeSet<_> = staged
        .entries()
        .into_iter()
        .filter_map(|p| p.strip_prefix("mods/").map(str::to_owned))
        .collect();
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/layouts");
    let mut layouts = BTreeSet::new();
    fn references(v: &Value, mods: &BTreeSet<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::String(name)) = o.get("mod") {
                    assert!(mods.contains(name), "unknown layout mod {name}");
                }
                for v in o.values() {
                    references(v, mods);
                }
            }
            Value::Array(a) => {
                for v in a {
                    references(v, mods);
                }
            }
            _ => {}
        }
    }
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str::<Node>(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        references(&serde_json::from_str::<Value>(&text).unwrap(), &mods);
        layouts.insert(path.file_stem().unwrap().to_str().unwrap().to_owned());
    }
    assert_eq!(layouts, crate::window::shipped_layouts().into_iter().map(str::to_owned).collect());
    let mod_commands = ["ribbon.next"];
    for text in [
        include_str!("../../defaults/keys.json"),
        include_str!("../../defaults/keys.macos.json"),
    ] {
        crate::keys::parse(text).unwrap();
        let sections: BTreeMap<String, BTreeMap<String, String>> = serde_json::from_str(text).unwrap();
        for bindings in sections.values() {
            for action in bindings.values() {
                let name = action.split(':').next().unwrap();
                if let Some(command) = crate::commands::find(name) {
                    let args = match action.split_once(':') {
                        Some((_, value)) => {
                            assert!(!command.args.is_empty(), "unexpected shipped key argument {action}");
                            crate::commands::shorthand(command, value)
                        }
                        None => Value::Null,
                    };
                    crate::commands::validate_args(command, &args).unwrap();
                    assert!(crate::commands::missing(command, &args).is_none(), "missing shipped key argument {action}");
                }
                assert!(
                    action == "terminal"
                        || action.starts_with("send:")
                        || crate::commands::COMMANDS.iter().any(|c| c.name == name)
                        || (mod_commands.contains(&name) && mods.contains(name.split('.').next().unwrap())),
                    "unknown shipped key action {action}"
                );
            }
        }
    }
}

#[gpui::test]
fn picker_scrolls_the_selection_into_view(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut values = fixture(true, props(), updates("current", "current"));
    let tabs: Vec<Value> = (1..=12)
        .map(|i| json!({"id":format!("tab{i}"),"index":i,"name":format!("Task {i}"),"active":i==1,"status":"done","panes":1,"zoomed":false,"settled":false,"settled_at":null,"changed_at":null,"working_since":null}))
        .collect();
    values.insert("spaces".into(), json!([
        {"id":"s1","index":1,"name":"Local","folder":"/fixture/project","machine":null,"branch":"main","active":true,"status":"working","tabs":tabs}
    ]));
    // Selecting the last row (with the arrows) must scroll it into the visible 320px.
    let tree = describe_with_values(cx, &staged, "mods/picker", values, Some("this.at = 999; cx.notify();")).unwrap();
    let top = tree.lines()
        .find(|l| l.contains("pk-space-here"))
        .unwrap_or_else(|| panic!("selected row missing: {tree}"));
    let px: f64 = top.split(".top[Number(").nth(1).unwrap_or_else(|| panic!("selected row must be placed inside the scrolled list: {tree}")).split(")").next().unwrap().parse().unwrap();
    assert!(px <= 320.0 - 32.0, "selected row at {px}px is below the visible list: {tree}");
}

#[gpui::test]
fn status_bar_shows_the_real_palette_key(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut bound = fixture(true, props(), updates("current", "current"));
    bound.insert("keys".into(), json!([{"section":"global","keys":"ctrl-p","action":"app.palette"}]));
    let tree = describe_with_values(cx, &staged, "mods/workspace-status", bound, None).unwrap();
    assert!(tree.contains("Ctrl+P"), "hint must show the real binding: {tree}");
    assert!(!tree.contains("Shift P"), "stale hardcoded hint: {tree}");

    let mut unbound = fixture(true, props(), updates("current", "current"));
    unbound.insert("keys".into(), json!([{"section":"global","keys":"ctrl-k","action":"app.help"}]));
    let tree = describe_with_values(cx, &staged, "mods/workspace-status", unbound, None).unwrap();
    assert!(tree.contains("commands"), "unbound hint still offers commands: {tree}");
    assert!(!tree.contains("Shift P"), "unbound hint must not invent a key: {tree}");
}

// yuzora(L1): Yuzora's new-pane menu opens as an overlay without a height of its own (a dialog
// from the command), so it is as tall as its content. Its list must not wait for a height to fill: with flex_1 and overflow_y_scrollbar (a scroll area
// that fills its parent) it drew nothing under the place pills.
#[gpui::test]
fn yuzora_new_pane_menu_shows_its_list_in_an_overlay(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let menu = json!({"menu":"pane","overlay":"o1"});
    let line = |tree: &str, id: &str| tree.lines().find(|l| l.contains(&format!("\"{id}\""))).unwrap_or_else(|| panic!("{id} missing: {tree}")).to_owned();
    let agents = "this.agents = Array.from({length: 30}, (_, i) => ({key: `a${i}`, name: `Agent ${i}`, icon: null, tint: true, installed: true, run: [`a${i}`]})); this.listed = true;";
    let tree = describe(cx, &staged, "mods/yuzora-shell-tabs", true, menu.clone(), updates("current", "current"), Some(agents)).unwrap();
    let root = line(&tree, "pane-menu");
    assert!(!root.contains(".size_full") && !root.contains(".h_full"), "the menu takes its rows' height: {root}");
    assert!(root.contains(".max_h[Number(520.0)]") && root.contains(".min_h[Number(0.0)]"), "{root}");
    let list = line(&tree, "pane-menu-list");
    assert!(!list.contains(".flex_1") && !list.contains(".size_full") && !list.contains("overflow_y_scrollbar"), "a list that waits to fill is empty in a popup: {list}");
    assert!(list.contains(".min_h[Number(0.0)]") && list.contains("overflow_y_scroll"), "a long list scrolls: {list}");
    for id in ["terminal", "agent-a0", "agent-a29"] {
        assert!(line(&tree, id).contains(".flex_none"), "rows keep their height: {tree}");
    }
    // The chosen place is unmistakable: the accent, the other one plain.
    let theme = crate::settings::theme();
    let (accent, ink) = (theme["accent"].as_str(), theme["accent_foreground"].as_str());
    let (right, down) = (line(&tree, "dir-right"), line(&tree, "dir-down"));
    assert!(right.contains(&format!(".bg[Str(\"{accent}\")]")) && right.contains(&format!(".text_color[Str(\"{ink}\")]")), "{right}");
    assert!(!down.contains(&format!(".bg[Str(\"{accent}\")]")), "{down}");

    // Nothing to offer still says so.
    let tree = describe(cx, &staged, "mods/yuzora-shell-tabs", true, menu.clone(), updates("current", "current"),
        Some("this.tools = () => []; this.agents = []; this.listed = true;")).unwrap();
    assert!(tree.contains("menu.no_tools") && tree.contains("menu.no_agents") && !tree.contains("menu.checking"), "{tree}");
    let tree = describe(cx, &staged, "mods/yuzora-shell-tabs", true, menu, updates("current", "current"),
        Some("this.agents = []; this.failed = true; this.listed = true;")).unwrap();
    assert!(tree.contains("menu.failed") && !tree.contains("menu.no_agents"), "{tree}");
}

// yuzora(L1): the tab bar lists the tabs of the current Space only, as Herdr's own does (the
// sidebar is where Spaces are told apart); switching Space swaps the tabs, none active shows none.
#[gpui::test]
fn yuzora_tab_bar_lists_only_the_current_space(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tab = |id: &str, active: bool, settled: bool| json!({"id":id,"index":1,"name":id,"active":active,"status":"idle","panes":1,"zoomed":false,"settled":settled,"settled_at":null,"changed_at":null,"working_since":null});
    let spaces = |active: Option<&str>| json!([
        {"id":"s1","index":1,"name":"Local","folder":"/a","machine":null,"branch":null,"active":active==Some("s1"),"status":"idle","tabs":[tab("a-one", true, false), tab("a-two", false, false), tab("a-old", false, true)]},
        {"id":"s2","index":2,"name":"Remote","folder":"/b","machine":"remote","branch":null,"active":active==Some("s2"),"status":"idle","tabs":[tab("b-one", true, true)]},
    ]);
    let shown = |cx: &mut TestAppContext, active: Option<&str>| {
        let mut values = fixture(true, json!({}), updates("current", "current"));
        values.insert("spaces".into(), spaces(active));
        let tree = describe_with_values(cx, &staged, "mods/yuzora-shell-tabs", values, None).unwrap();
        let ids = ["a-one", "a-two", "a-old", "b-one"].map(|id| tree.contains(&format!("\"tab-{id}\"")));
        (ids, tree)
    };
    let (ids, tree) = shown(cx, Some("s1"));
    assert_eq!(ids, [true, true, false, false], "{tree}");
    let (ids, tree) = shown(cx, Some("s2"));
    assert_eq!(ids, [false, false, false, true], "{tree}");
    let (ids, tree) = shown(cx, None);
    assert_eq!(ids, [false; 4], "{tree}");
}

// yuzora(L1): the sidebar's Herdr tools menu has no "next layout" row: pressed in the yuzora
// layout it lands in Herdr's own `main`, where none of Yuzora's features exist (2026-11-04).
#[gpui::test]
fn yuzora_tools_menu_has_no_next_layout_row(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let tree = describe(cx, &staged, "mods/yuzora-sidebar-workspace", true, json!({"menu":"tools","overlay":"o1"}), updates("current", "current"), None).unwrap();
    assert!(tree.contains("\"m-palette\"") && tree.contains("\"m-help\""), "the other rows stay: {tree}");
    assert!(!tree.contains("\"m-layout\""), "no row switches layout: {tree}");
}

// yuzora(L1): the tab bar has no new-pane or split buttons (the user asked for them gone,
// 2026-11-04), with or without a focused pane; "+" and the window's tabs stay.
#[gpui::test]
fn yuzora_tab_bar_has_no_new_pane_or_split_buttons(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for populated in [true, false] {
        let tree = describe(cx, &staged, "mods/yuzora-shell-tabs", populated, json!({}), updates("current", "current"), None).unwrap();
        for id in ["pane-add", "split-right", "split-down"] {
            assert!(!tree.contains(&format!("\"{id}\"")), "{id} must not be in the tab bar: {tree}");
        }
        assert!(tree.contains("\"tab-new\""), "the + button stays: {tree}");
    }
}

// yuzora(L1): with the buttons gone the command is the only way to the new-pane menu: it opens
// the menu as a centered dialog of this mod.
#[gpui::test]
fn yuzora_new_pane_command_opens_the_menu(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let open = |cx: &mut TestAppContext, command: &str| {
        let calls = Rc::new(RefCell::new(Calls::default()));
        calls.borrow_mut().commands.push(command.into());
        let values = fixture(true, json!({}), updates("current", "current"));
        let policy = Policy::new()
            .with_storage_path(staged.0.join("store-command.json"))
            .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
            .with_host_module(module(values, calls.clone()))
            .unwrap();
        let policy = crate::yuzora::with_modules(policy, "");
        let root = staged.0.join("mods/yuzora-shell-tabs");
        let window = cx.add_window(|_, _| Empty);
        let session = window.update(cx, |_, window, cx| {
            let mut session = DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap();
            session.describe(window, cx).unwrap();
            session
        }).unwrap();
        cx.run_until_parked();
        drop(session);
        window.update(cx, |_, window, _| window.remove_window()).unwrap();
        let calls = calls.borrow();
        assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
        calls.effects.iter().filter(|(n, _)| n == "openOverlay").map(|(_, a)| json!(a)).collect::<Vec<Value>>()
    };
    assert_eq!(
        open(cx, "new-pane"),
        [json!([{"mod": "yuzora-shell-tabs", "kind": "dialog", "width": 280, "props": {"menu": "pane"}}])]
    );
    assert!(open(cx, "something-else").is_empty(), "other commands open nothing");
}

// yuzora(L1): a picked row closes the menu and goes where the pills say: beside the focused
// pane, or a new tab when nothing is focused.
#[gpui::test]
fn yuzora_new_pane_menu_puts_a_pick_where_the_pills_say(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut picks = |populated: bool, setup: &str| {
        let calls = Rc::new(RefCell::new(Calls::default()));
        let values = fixture(populated, json!({"menu":"pane","overlay":"o1"}), updates("current", "current"));
        let policy = Policy::new()
            .with_storage_path(staged.0.join("store-pick.json"))
            .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
            .with_host_module(module(values, calls.clone()))
            .unwrap();
        let policy = crate::yuzora::with_modules(policy, "");
        let root = staged.wrapper("mods/yuzora-shell-tabs", setup);
        let window = cx.add_window(|_, _| Empty);
        window.update(cx, |_, window, cx| {
            let mut session = DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap();
            session.describe(window, cx).unwrap();
            window.remove_window();
        }).unwrap();
        let calls = calls.borrow();
        assert!(calls.unexpected.is_empty(), "{:?}", calls.unexpected);
        let runs: Vec<Value> = calls.effects.iter().filter(|(n, _)| n == "run").map(|(_, a)| json!(a)).collect();
        assert!(calls.effects.iter().any(|(n, a)| n == "closeOverlay" && a == &[json!("o1")]), "the menu closes");
        (runs, calls.shared.get("yuzora.shell.split").cloned())
    };
    let (runs, _) = picks(true, "this.choose({kind: 'terminal'});");
    assert_eq!(runs, [json!(["pane.split", {"direction": "right", "pane": "p1"}])]);
    let (runs, split) = picks(true, "this.place('down', cx); this.choose({kind: 'agent', name: 'Pi', run: ['pi']});");
    assert_eq!(runs, [json!(["terminal.open", {"argv": ["pi"], "place": "down"}])]);
    assert_eq!(split, Some(json!("down")), "the next menu starts below too");
    let (runs, _) = picks(true, "this.choose({kind: 'tool', mod: 'yuzora-db-query'});");
    assert_eq!(runs, [json!(["pane.split", {"direction": "right", "pane": "p1", "mod": "yuzora-db-query", "props": {}}])]);
    let (runs, _) = picks(false, "this.choose({kind: 'terminal'});");
    assert_eq!(runs, [json!(["tab.new", {}])], "nothing focused: a new tab");
}

// yuzora(L1): Herdr's own Files and Changes panels start off in Yuzora (its sidebar has Files and
// Git): no saved choice means none, a saved one stays, and Settings > General turns them on and off.
#[gpui::test]
fn yuzora_herdr_panels_start_off(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let shown: Value = serde_json::from_str(include_str!("../../defaults/layouts/yuzora.json")).unwrap();
    let mut session = |entry: &str, layout: &Value, saved: Option<Value>, setup: Option<&str>| {
        let calls = Rc::new(RefCell::new(Calls::default()));
        if let Some(saved) = saved {
            calls.borrow_mut().shared.insert("workspace.panels".into(), saved);
        }
        let mut values = fixture(true, json!({}), updates("current", "current"));
        values.insert("layout".into(), layout.clone());
        let policy = Policy::new()
            .with_storage_path(staged.0.join("store-panels.json"))
            .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
            .with_host_module(module(values, calls.clone()))
            .unwrap();
        let policy = crate::yuzora::with_modules(policy, "");
        let root = setup.map(|s| staged.wrapper(entry, s)).unwrap_or_else(|| staged.0.join(entry));
        let window = cx.add_window(|_, _| Empty);
        let tree = window.update(cx, |_, window, cx| {
            let mut session = DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), root, policy, window, cx).unwrap();
            session.describe(window, cx).unwrap();
            let tree = session.describe(window, cx).unwrap();
            window.remove_window();
            tree
        }).unwrap();
        let calls = calls.borrow();
        assert!(calls.unexpected.is_empty(), "{entry}: {:?}", calls.unexpected);
        let runs: Vec<Value> = calls.effects.iter().filter(|(n, _)| n == "run").map(|(_, a)| json!(a)).collect();
        (tree, calls.shared.get("workspace.panels").cloned(), runs)
    };

    // The default itself.
    std::fs::write(staged.0.join("main.js"), "import { View, div } from 'gpui-kit'; import { panelsOn } from './mods/yuzora-shell-tabs/main.js'; \
        export default class Check extends View { render() { return div().child(JSON.stringify([undefined, null, [], ['files'], 'files'].map((v) => panelsOn(v)))); } }").unwrap();
    let (tree, ..) = session("", &shown, None, None);
    assert!(tree.contains(r#"[[],[],[],[\"files\"],[]]"#), "{tree}");

    // The tab bar shows a button only for a panel that's on, and never writes the choice.
    let button = |tree: &str, id: &str| tree.contains(&format!("\"panel-{id}\""));
    for (saved, files, changes) in [
        (None, false, false),
        (Some(Value::Null), false, false),
        (Some(json!([])), false, false),
        (Some(json!(["files"])), true, false),
        (Some(json!(["files", "changes"])), true, true),
    ] {
        let (tree, kept, _) = session("mods/yuzora-shell-tabs", &shown, saved.clone(), None);
        assert_eq!((button(&tree, "files"), button(&tree, "changes")), (files, changes), "{saved:?}: {tree}");
        assert_eq!(kept, saved, "the tab bar keeps the saved choice");
    }

    // Settings > General shows a switch for each and turns them on and off.
    let accent = crate::settings::theme()["accent"].clone();
    let switch = |tree: &str, id: &str| tree.lines().find(|l| l.contains(&format!("\"gn-panel-{id}\""))).unwrap_or_else(|| panic!("{id}: {tree}")).contains(&format!(".bg[Str({accent:?})]"));
    let (tree, kept, _) = session("mods/yuzora-settings-general", &shown, Some(json!(["changes"])), None);
    assert_eq!((switch(&tree, "files"), switch(&tree, "changes")), (false, true), "{tree}");
    assert_eq!(kept, Some(json!(["changes"])), "showing the page changes nothing");
    let (_, kept, runs) = session("mods/yuzora-settings-general", &shown, None, Some("this.setPanel('files', true);"));
    assert_eq!((kept, runs), (Some(json!(["files"])), vec![]));
    // Turning off the panel that's showing closes it.
    let mut open = shown.clone();
    fn show(node: &mut Value) {
        if node["id"] == "panel" {
            node["hidden"] = json!(false);
        }
        for child in node.get_mut("children").and_then(Value::as_array_mut).into_iter().flatten() {
            show(child);
        }
    }
    show(&mut open);
    let (_, kept, runs) = session("mods/yuzora-settings-general", &open, Some(json!(["files", "changes"])), Some("this.setPanel('files', false);"));
    assert_eq!((kept, runs), (Some(json!(["changes"])), vec![json!(["layout.hide", {"id": "panel"}])]));
}

// yuzora(L2): Git UI layout. The description tree has no sizes, so these mount a scenario in a
// window, lay it out and send it real mouse events. A scenario lays out one of the L2 mods' own
// pieces (a row, the commit detail panel) in a box of a known size and reports through `probe`.
struct Mounted(gpui::Entity<gpui_shell::ShellRoot>);
impl gpui::Render for Mounted {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        self.0.clone()
    }
}

struct Scenario {
    visual: gpui::VisualTestContext,
    notes: Rc<RefCell<Vec<Vec<Value>>>>,
    _calls: Rc<RefCell<Calls>>,
}

impl Scenario {
    /// `source` replaces the staged main.js; it may `import { note } from "probe"`.
    fn mount(cx: &mut TestAppContext, staged: &Staged, source: &str, width: f32, height: f32) -> Self {
        use std::ops::Deref;
        let calls = Rc::new(RefCell::new(Calls::default()));
        let notes: Rc<RefCell<Vec<Vec<Value>>>> = Rc::default();
        let sink = notes.clone();
        let policy = Policy::new()
            .with_storage_path(staged.0.join("store-scenario.json"))
            .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
            .with_host_module(module(fixture(false, props(), updates("current", "current")), calls.clone()))
            .unwrap();
        let policy = crate::yuzora::with_modules(policy, "")
            .with_host_module(HostModule::new("probe").function("note", move |a| {
                sink.borrow_mut().push((0..8).map_while(|i| a.get(i)).map(from_host).collect());
                Ok(HostValue::Null)
            }))
            .unwrap();
        gpui_shell::policy::set_default(policy);
        std::fs::write(staged.0.join("main.js"), source).unwrap();
        let runtime = ShellRuntime::new_isolated().unwrap();
        let window = cx.add_window(|window, cx| Mounted(runtime.try_load(&staged.0, window, cx).unwrap()));
        let visual = gpui::VisualTestContext::from_window(*window.deref(), cx);
        visual.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        let mut scenario = Self { visual, notes, _calls: calls };
        scenario.settle();
        scenario
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.visual.update(|window, cx| window.draw(cx).clear(cx));
            self.visual.run_until_parked();
        }
    }

    fn wheel(&mut self, x: f64, y: f64, dy: f64) {
        self.visual.simulate_event(gpui::MouseMoveEvent { position: gpui::point(gpui::px(x as f32), gpui::px(y as f32)), modifiers: gpui::Modifiers::default(), pressed_button: None });
        self.settle();
        self.visual.simulate_event(
            gpui::ScrollWheelEvent {
                position: gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy as f32))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            },
        );
        self.settle();
    }

    fn mouse(&mut self, kind: &str, x: f64, y: f64, clicks: usize) {
        let position = gpui::point(gpui::px(x as f32), gpui::px(y as f32));
        let modifiers = gpui::Modifiers::default();
        match kind {
            "down" => self.visual.simulate_event(gpui::MouseDownEvent { position, modifiers, button: gpui::MouseButton::Left, click_count: clicks, first_mouse: false }),
            "up" => self.visual.simulate_event(gpui::MouseUpEvent { position, modifiers, button: gpui::MouseButton::Left, click_count: clicks }),
            _ => self.visual.simulate_event(gpui::MouseMoveEvent { position, modifiers, pressed_button: Some(gpui::MouseButton::Left) }),
        }
        self.settle();
    }

    fn hover(&mut self, x: f64, y: f64) {
        self.visual.simulate_event(gpui::MouseMoveEvent { position: gpui::point(gpui::px(x as f32), gpui::px(y as f32)), modifiers: gpui::Modifiers::default(), pressed_button: None });
    }

    fn click(&mut self, x: f64, y: f64) {
        self.mouse("down", x, y, 1);
        self.mouse("up", x, y, 1);
    }

    /// The last `note(name, …)`.
    fn last(&self, name: &str) -> Option<Vec<Value>> {
        self.notes.borrow().iter().rev().find(|n| n.first() == Some(&json!(name))).cloned()
    }
}

/// The commit detail panel of the Git Log in a `width`×`height` box, dragged by its `dragging()`.
/// A press with three clicks swaps in another commit's details (the height must stay).
fn detail_scenario(lines: usize, side: bool, width: f32, height: f32, files: usize) -> String {
    format!(
        r#"
import Real from "./mods/yuzora-git-log/main.js";
import {{ v_flex }} from "gpui-base";
import {{ theme }} from "herdr";
import {{ note }} from "probe";
const make = (hash, n) => ({{ hash, subject: "Subject " + hash, body: Array.from({{ length: n }}, (_, i) => "line " + i).join("\n"), author: "A", email: "a@x",
  authorTime: 1700000000, committer: "A", committerEmail: "a@x", commitTime: 1700000000, parents: [], signature: "N", signer: "",
  files: Array.from({{ length: {files} }}, (_, i) => ({{ path: "src/f" + i + ".rs", kind: "M" }})) }});
export default class Scenario extends Real {{
  init(p, cx) {{
    super.init(p, cx); this.w = 900; this.showDetail = true; this.sel = {{ hash: "a" }}; this.detail = {{ hash: "a", data: make("a", {lines}) }};
    this.probe = (slot) => (e) => note(slot, e.local_position.y);
  }}
  render(cx) {{
    note("state", this.infoH ?? null, this.infoNow ?? null, this.bodyH ?? null);
    return this.dragging(v_flex().id("scenario").w({width}).h({height})
      .on_mouse_down("left", (e, cx) => {{ if (e.click_count === 3) {{ this.detail = {{ hash: "b", data: make("b", 5) }}; cx.notify(); }} }})
      .child(this.detailPanel(theme(), {side}, "mono", {{}})));
  }}
}}
"#
    )
}

fn state(s: &Scenario) -> (Option<f64>, f64, f64) {
    let n = s.last("state").expect("a render");
    (n[1].as_f64(), n[2].as_f64().expect("info measured"), n[3].as_f64().expect("body measured"))
}

// yuzora(L2): the commit message scrolls. `overflow_y_scrollbar` with a `max_h` did not move.
#[gpui::test]
fn git_log_commit_message_scrolls_and_the_files_stay(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (side, width, height) in [(true, 340.0, 600.0), (false, 700.0, 260.0)] {
        let mut s = Scenario::mount(cx, &staged, &detail_scenario(300, side, width, height, 40), width, height);
        let (_, visible, body) = state(&s);
        assert!(visible <= if side { 300.0 } else { 110.0 } + 0.5, "side={side}: the info is as tall as its limit, not its 300 lines: {visible}");
        assert!(body - visible - 7.0 - 24.0 >= 72.0, "side={side}: the files keep room: body {body}, info {visible}");
        s.mouse("move", 100.0, 40.0, 0);
        let before = s.last("info").expect("content under the pointer")[1].as_f64().unwrap();
        s.wheel(100.0, 40.0, -30.0);
        s.mouse("move", 100.0, 40.0, 0);
        let after = s.last("info").unwrap()[1].as_f64().unwrap();
        assert!(after > before + 10.0, "side={side}: the wheel moves the message ({before} -> {after})");
    }
}

// yuzora(L2): drag the bar between the info and the files; the heights follow and stop at the minimums.
#[gpui::test]
fn git_log_detail_divider_drags_and_keeps_minimums(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (side, width, height) in [(true, 340.0, 600.0), (false, 700.0, 300.0)] {
        let mut s = Scenario::mount(cx, &staged, &detail_scenario(300, side, width, height, 40), width, height);
        let (set, visible, body) = state(&s);
        assert_eq!(set, None, "side={side}: untouched, the default");
        let bar = 28.0 + visible + 3.0; // the panel's header, the info, the middle of the bar
        let limit = body - 24.0 - 7.0 - 72.0;
        s.mouse("down", 100.0, bar, 1);
        s.mouse("move", 100.0, bar - 20.0, 0);
        assert_eq!(state(&s).1, visible - 20.0, "side={side}: the info follows the pointer up");
        s.mouse("move", 100.0, height as f64 - 1.0, 0);
        assert_eq!(state(&s).1, limit, "side={side}: the files keep 72px");
        s.mouse("move", 100.0, 1.0, 0);
        assert_eq!(state(&s).1, 48.0, "side={side}: the info keeps 48px");
        s.mouse("move", 100.0, bar - 40.0, 0);
        s.mouse("up", 5.0, 5.0, 1); // released away from the bar: the drag is over
        let dragged = state(&s).1;
        s.mouse("move", 100.0, bar + 100.0, 0);
        assert_eq!(state(&s).1, dragged, "side={side}: moves after the release change nothing");
        s.mouse("down", 100.0, 10.0, 3); // another commit
        assert_eq!(state(&s).1, dragged, "side={side}: the height stays on another commit");
        let bar = 28.0 + dragged + 3.0;
        s.mouse("down", 100.0, bar, 2);
        assert_eq!(state(&s).0, None, "side={side}: a double press puts the default back");
    }
}

// yuzora(L2): a pane too short for the default height still shows the divider and 72px of files.
#[gpui::test]
fn git_log_detail_default_height_leaves_room_for_the_files(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (side, width, height) in [(true, 340.0, 300.0), (true, 340.0, 200.0), (false, 700.0, 200.0)] {
        let s = Scenario::mount(cx, &staged, &detail_scenario(300, side, width, height, 40), width, height);
        let (set, visible, body) = state(&s);
        assert_eq!(set, None);
        assert!(visible + 7.0 + 24.0 + 72.0 <= body.max(48.0 + 7.0 + 24.0 + 72.0) + 0.5, "{side}/{height}: info {visible} in a body of {body}");
        assert!(28.0 + visible + 7.0 <= height as f64, "{side}/{height}: the divider is on screen");
    }
}

// yuzora(L2): leaving the pane with the button down ends the drag (its release would not be seen).
#[gpui::test]
fn git_log_detail_divider_drag_ends_when_the_pointer_leaves_the_pane(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let mut s = Scenario::mount(cx, &staged, &detail_scenario(300, true, 340.0, 600.0, 40), 700.0, 600.0);
    let (_, visible, _) = state(&s);
    let bar = 28.0 + visible + 3.0;
    s.mouse("down", 100.0, bar, 1);
    s.mouse("move", 100.0, bar - 30.0, 0);
    let dragged = state(&s).1;
    assert_eq!(dragged, visible - 30.0);
    s.mouse("move", 600.0, bar - 30.0, 0); // out of the pane, still in the window
    s.mouse("move", 100.0, bar - 80.0, 0); // back in, button no longer counted
    assert_eq!(state(&s).1, dragged, "the drag ended when the pointer left");
}

// yuzora(L2): a long message shows in full, up to 2000 lines and then says what is left out.
#[gpui::test]
fn git_log_commit_message_is_shown_whole_up_to_a_cap(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    for (lines, cut) in [(100, false), (2500, true)] {
        let setup = format!(
            "this.repo = {{}}; this.showDetail = true; this.sel = {{hash:'a'}}; this.detail = {{hash:'a', data: {{hash:'a', subject:'S', body: Array.from({{length:{lines}}}, (_, i) => 'line ' + i).join('\\n'), \
             author:'A', email:'a@x', authorTime:1, committer:'A', committerEmail:'a@x', commitTime:1, parents:[], signature:'N', signer:'', files:[]}}}};"
        );
        let tree = describe(cx, &staged, "mods/yuzora-git-log", false, props(), updates("current", "current"), Some(&setup)).unwrap();
        assert!(tree.contains(&format!("line {}\"", lines.min(2000) - 1)), "{lines} lines: the last one shows");
        assert!(!tree.contains("line 2000\""), "{lines} lines: the cap");
        assert_eq!(tree.contains("text \"detail.truncated\""), cut, "{lines} lines: the notice");
    }
}

/// The repo picker in the frame an overlay gives it (`render_overlays`): 340 wide, at most 360 high
/// (it is as tall as its rows: a popup, or a dialog without a height), a border, clipped, a row
/// holding `render_slot`'s `size_full` div and, in it, the shell root of a `fit` overlay. Reports the
/// frame's and the picker's size; a row picked says `pick`.
fn picker_in_overlay(n: usize, loaded: bool) -> String {
    format!(
        r#"
import Real from "./mods/yuzora-sidebar-git/main.js";
import {{ div }} from "gpui-kit";
import {{ note }} from "probe";
export default class Scenario extends Real {{
  init(p, cx) {{
    this.handle = cx.focus_handle(); this.handle.focus();
    this.picker = {{ space: "s1", folder: "/sp", on: null, cwds: [], current: "/sp/r0", pinned: false }};
    this.found = {loaded} ? {{ repos: Array.from({{ length: {n} }}, (_, i) => ({{ root: "/sp/r" + i, name: "r" + i, rel: "r" + i, trusted: false }})), truncated: false }} : null;
  }}
  pick(root) {{ note("pick", root); }}
  render(cx) {{
    const picker = this.renderPicker().on_size_change((s) => note("picker", s.width, s.height));
    return div().id("scenario").size_full().child(
      div().id("ov-frame").w(340).max_h(360).border_1().overflow_hidden().flex().on_size_change((s) => note("frame", s.width, s.height))
        .child(div().size_full().child(div().relative().flex().flex_col().min_h(0).child(picker))));
  }}
}}
"#
    )
}

// yuzora(L2): the repo picker opens as a popup (or a dialog) without a height of its own, so it is as
// tall as its rows, up to the frame's 360. With flex_1 and overflow_y_scrollbar (a scroll area that
// fills its parent) its list was 0 high and the frame 39: only "follow the focus" showed. Laid out in
// the overlay's frame: the rows show and can be clicked (the last one too, scrolled to), loading and an
// empty list leave room for their text, and a click anywhere in the frame leaves the keys working.
#[gpui::test]
fn yuzora_git_repo_picker_fills_its_overlay_frame(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let size = |s: &Scenario, k: &str| s.last(k).map(|v| v[2].as_f64().unwrap()).unwrap_or_else(|| panic!("{k} not measured"));
    let picked = |s: &mut Scenario, y: f64| {
        let before = s.notes.borrow().len();
        s.click(100.0, y);
        s.notes.borrow()[before..].iter().rev().find(|n| n[0] == json!("pick")).map(|n| n[1].as_str().unwrap_or("follow").to_owned())
    };
    // Rows of 26 under 1 + 4 px: the follow row, 1 + 2 + 2 for the line, then the repos.
    let staged = Staged::new();
    let mut s = Scenario::mount(cx, &staged, &picker_in_overlay(2, true), 600.0, 800.0);
    assert_eq!(size(&s, "frame"), 3.0 * 26.0 + 5.0 + 8.0 + 2.0, "2 repos: the frame takes the rows' height");
    assert_eq!(picked(&mut s, 49.0).as_deref(), Some("/sp/r0"));
    assert_eq!(picked(&mut s, 75.0).as_deref(), Some("/sp/r1"));
    for (n, loaded) in [(0, false), (0, true)] {
        let s = Scenario::mount(cx, &staged, &picker_in_overlay(n, loaded), 600.0, 800.0);
        assert!(size(&s, "frame") > 39.0 + 20.0, "loaded {loaded}: room for the text under the follow row: {}", size(&s, "frame"));
    }
    let mut s = Scenario::mount(cx, &staged, &picker_in_overlay(50, true), 600.0, 800.0);
    assert_eq!(size(&s, "frame"), 360.0, "50 repos: the frame is at its limit");
    s.wheel(100.0, 200.0, -5000.0);
    assert_eq!(picked(&mut s, 345.0).as_deref(), Some("/sp/r49"), "scrolled to the end, the last one is there");
    // The keys: ↓ Enter picks the first repo; a click on the line or a row inside the frame keeps them.
    for y in [None, Some(33.0), Some(18.0)] {
        let mut s = Scenario::mount(cx, &staged, &picker_in_overlay(2, true), 600.0, 800.0);
        if let Some(y) = y {
            s.click(100.0, y);
        }
        let before = s.notes.borrow().len();
        s.visual.simulate_keystrokes("down enter");
        s.settle();
        let keyed = s.notes.borrow()[before..].iter().rev().find(|n| n[0] == json!("pick")).map(|n| n[1].as_str().unwrap_or("follow").to_owned());
        assert!(keyed.is_some(), "after a click at {y:?}, ↓ Enter picked nothing");
    }
}

/// A sidebar file row in a list of `width` (padded and measured like the real list): its parts report
/// their size (`name`, `dir`, `add`, `del`, `action:<label>`) and, under the pointer, where they sit
/// (`at:<part>`, x and width). A click goes through the real `clickRow` and says what it did (`result`).
fn row_scenario(section: &str, path: &str, add: u64, del: u64, width: f32) -> String {
    format!(
        r#"
import Real from "./mods/yuzora-sidebar-git/main.js";
import {{ div }} from "gpui-kit";
import {{ theme }} from "herdr";
import {{ note }} from "probe";
const file = {{ path: "{path}", kind: "M", add: {add}, del: {del} }};
export default class Scenario extends Real {{
  init(p, cx) {{
    super.init(p, cx);
    this.st = {{ conflicts: [], staged: [], unstaged: [], untracked: [], counts: {{ total: 1 }} }};
    this.st["{section}" === "unstaged" ? "unstaged" : "{section}"].push(file);
  }}
  act() {{ this.acted = true; }}
  openDiff() {{ this.diffed = true; }}
  render(cx) {{
    const probe = (slot, kind) => (kind ? (e) => note("at:" + slot, e.bounds.x, e.bounds.width) : (size) => note(slot, size.width));
    const key = "{section}:{path}";
    return div().id("list").w({width}).px(6)
      .on_size_change((size) => {{ this.listWidth = size.width; }})
      .on_mouse_down("left", (e) => {{ this.downX = e.local_position.x; }})
      .on_click((_e, cx) => {{
        this.acted = false; this.diffed = false; this.confirmDiscard = null;
        this.clickRow(key, cx);
        note("result", this.confirmDiscard ? "discard" : this.acted ? "act" : this.diffed ? "diff" : "none");
      }})
      .child(this.fileRow({{ section: "{section}", f: file }}, theme(), probe));
  }}
}}
"#
    )
}

fn number(n: &Value) -> f64 {
    n.as_f64().unwrap()
}

// yuzora(L2): in a narrow sidebar the name gives way before +N −M and the buttons, and +N −M before
// the buttons: they stay at the right edge, wherever clickRow counts from.
#[gpui::test]
fn sidebar_git_row_keeps_counts_and_buttons_clear_of_the_name(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let long = format!("deep/er/{}/{}.rs", "folder".repeat(10), "a-very-long-file-name".repeat(4));
    for (section, buttons) in [("unstaged", 2usize), ("staged", 1), ("conflicts", 1)] {
        for (add, del) in [(123, 45), (150000, 150000)] {
            for width in [100.0, 140.0, 180.0, 248.0] {
                let s = Scenario::mount(cx, &staged, &row_scenario(section, &long, add, del, width), width, 40.0);
                let w = |slot: &str| s.last(slot).unwrap_or_else(|| panic!("{slot}"))[1].as_f64().unwrap();
                let (row, text, name, dir) = (w("row"), w("text"), w("name"), w("dir"));
                let (a, d) = (w("add"), w("del"));
                let who = format!("{section}/{width}/{add}: row {row} text {text} name {name} dir {dir} +{a} -{d}");
                assert_eq!(row, width as f64 - 12.0, "{who}");
                assert!(name <= text + 0.5 && name + dir <= text + 0.5, "the name and folder stay in their box: {who}");
                let labels: Vec<String> = s.notes.borrow().iter().filter_map(|n| n[0].as_str().filter(|k| k.starts_with("action:")).map(String::from)).collect();
                assert_eq!(labels.len(), buttons, "{who}");
                // Left to right: 6 + 12 + 6 + text + 6 + +N + 6 + -M + 6 + buttons; no part sticks out of the row.
                let fixed = 6.0 + 12.0 + 6.0 + 6.0 + 6.0 + 6.0 + 22.0 * buttons as f64;
                assert!(fixed + text + a + d <= row + 0.5, "nothing sticks out of the row: {who}");
                if width >= 180.0 {
                    assert!(a > 0.0 && d > 0.0, "{who}");
                    if add == 123 {
                        assert!(a > 20.0 && d > 15.0, "short numbers are written out: {who}");
                    }
                }
            }
        }
    }
}

// yuzora(L2): what is drawn under the pointer is what a click does. The row is the real `fileRow` in a list padded
// and measured like the real one, clicks go through the real `clickRow` (the virtual list is not laid out in a test window).
#[gpui::test]
fn sidebar_git_row_buttons_are_where_clickrow_counts_them(cx: &mut TestAppContext) {
    cx.update(gpui_shell::init);
    let staged = Staged::new();
    let long = format!("deep/{}.rs", "a-long-file-name".repeat(3));
    for (section, expect) in [("unstaged", vec![("action:↺", "discard"), ("action:+", "act")]), ("staged", vec![("action:−", "act")])] {
        for (add, del) in [(123, 45), (150000, 150000)] {
            for width in [140.0, 180.0, 248.0] {
                let mut s = Scenario::mount(cx, &staged, &row_scenario(section, &long, add, del, width), width, 40.0);
                // Hover along the row to learn where each part is drawn.
                for x in 0..width as i32 {
                    s.hover(x as f64 + 0.5, 12.0);
                }
                let pos: BTreeMap<String, (f64, f64)> = s.notes.borrow().iter()
                    .filter_map(|n| n[0].as_str()?.strip_prefix("at:").map(|k| (k.to_string(), (number(&n[1]), number(&n[2]))))).collect();
                let at = |_: &Scenario, part: &str| *pos.get(part).unwrap_or_else(|| panic!("{section}/{width}: {part} never under the pointer: {pos:?}"));
                let who = format!("{section}/{width}/{add}");
                // Left to right, in the order they are drawn.
                let labels: Vec<&str> = expect.iter().map(|e| e.0).collect();
                let xs: Vec<f64> = labels.iter().map(|l| at(&s, l).0).collect();
                assert!(xs.windows(2).all(|p| p[0] < p[1]), "{who}: the buttons are in the order {labels:?}: {xs:?}");
                let (row_x, row_w) = at(&s, "row");
                for (label, result) in &expect {
                    let (x, w) = at(&s, label);
                    for click in [x + w / 2.0, x + 2.0, x + w - 2.0] {
                        s.notes.borrow_mut().clear();
                        s.click(click, 12.0);
                        assert_eq!(s.last("result").map(|n| n[1].clone()), Some(json!(result)), "{who}: {label} at {click} (button {x}..{})", x + w);
                    }
                }
                // The buttons end at the row's right edge.
                let (lx, lw) = at(&s, expect.last().unwrap().0);
                assert!((lx + lw - (row_x + row_w)).abs() < 0.6, "{who}: the last button ends at the row's edge ({} vs {})", lx + lw, row_x + row_w);
                for part in ["add", "del", "name"] {
                    if part == "name" && width < 248.0 {
                        continue; // a name this narrow can be under no pointer at all
                    }
                    let (x, w) = at(&s, part);
                    s.notes.borrow_mut().clear();
                    s.click(x + w / 2.0, 12.0);
                    assert_eq!(s.last("result").map(|n| n[1].clone()), Some(json!("diff")), "{who}: {part} opens the diff");
                }
            }
        }
    }
}
