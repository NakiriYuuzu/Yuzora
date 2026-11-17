//! Settings > Mods & Templates (docs/beta2/settings.md): one page with the Mods, Layouts and Templates tabs, and
//! Install from link beside Install from folder. The old Templates page is its Templates tab now: asking for it by
//! its section or mod name opens that tab.
use super::*;

/// The page with `props`, nothing of the person's read: the mods listing and the app status are swapped out
/// (a call of `list` counts in `t.listed`), and the Templates tab's reading records what it was made with in
/// `t.made` instead of asking hc. `setup` runs at the end of init.
fn page(cx: &mut TestAppContext, staged: &Staged, props: Value, setup: &str) -> DockCopy {
    scenario(staged, setup);
    dock_copy(cx, staged, "", props, &Value::Null, Value::Null, &[], &[], None)
}

fn scenario(staged: &Staged, setup: &str) {
    std::fs::write(staged.0.join("main.js"), format!(
        "import Real from './mods/yuzora-settings-mods/main.js'; \
         import {{ Templates }} from './mods/yuzora-settings-mods/templates.js'; \
         import {{ InputState }} from 'gpui-base'; \
         import {{ shared, setShared }} from 'herdr'; \
         Templates.prototype.reload = function () {{ \
           setShared('t.made', [...(shared('t.made') ?? []), {{ machine: this.machine, edit: this.pendingEdit }}]); \
           this.list = {{ dir: '/x/templates', templates: [] }}; }}; \
         export default class Scenario extends Real {{ init(props, cx) {{ \
           this.refresh = () => {{}}; this.list = () => setShared('t.listed', (shared('t.listed') ?? 0) + 1); \
           super.init(props, cx); {setup} }} }}"
    )).unwrap();
}

/// The page as `page` makes it with no props, yuzora:settings' planFetch and installFolder stood in for as
/// `module` stands in for herdr: planFetch answers `answer`, installFolder `{ok, plan: answer.plan}`, and every
/// call is kept. What it drew and the calls so far, once the answers have come and again a second later.
fn answered(cx: &mut TestAppContext, staged: &Staged, answer: Value, setup: &str) -> [(String, Vec<Value>); 2] {
    scenario(staged, setup);
    let calls: Rc<RefCell<Vec<Value>>> = Rc::default();
    let (fetches, installs, installed) = (calls.clone(), calls.clone(), json!({"ok": true, "plan": answer["plan"]}));
    let settings = yuzora_settings::host_module()
        .async_function("planFetch", move |a| {
            fetches.borrow_mut().push(json!(["planFetch", a.string(0)?]));
            Ok(std::future::ready(Ok(to_host(&answer))))
        })
        .async_function("installFolder", move |a| {
            installs.borrow_mut().push(json!(["installFolder", a.string(0)?]));
            Ok(std::future::ready(Ok(to_host(&installed))))
        });
    let herdr = Rc::new(RefCell::new(Calls::default()));
    let policy = Policy::new()
        .with_storage_path(staged.0.join("store-answered.json"))
        .with_capabilities(Capabilities::new().storage(true).read_roots([staged.0.clone()]))
        .with_host_module(module(fixture(true, json!({}), updates("current", "current")), herdr.clone()))
        .unwrap();
    let policy = crate::yuzora::with_modules(policy, "").with_host_module(settings).unwrap();
    let window = cx.add_window(|_, _| Empty);
    let mut session = window.update(cx, |_, window, cx| DescriptionSession::load(ShellRuntime::new_isolated().unwrap(), &staged.0, policy, window, cx).unwrap()).unwrap();
    let mut seen = Vec::new();
    for wait in [0, 1000] {
        cx.executor().advance_clock(std::time::Duration::from_millis(wait));
        cx.run_until_parked();
        let tree = window.update(cx, |_, window, cx| session.describe(window, cx).unwrap()).unwrap();
        cx.run_until_parked();
        seen.push((tree, calls.borrow().clone()));
    }
    drop(session);
    window.update(cx, |_, window, _| window.remove_window()).unwrap();
    cx.run_until_parked();
    assert!(herdr.borrow().unexpected.is_empty(), "{:?}", herdr.borrow().unexpected);
    seen.try_into().unwrap()
}

fn made(r: &DockCopy) -> Vec<Value> {
    r.shared.get("t.made").and_then(|m| m.as_array().cloned()).unwrap_or_default()
}

#[gpui::test]
fn the_page_opens_on_the_tab_its_props_name(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    for (props, tab) in [(json!({}), "mods"), (json!({"tab": "pane"}), "mods"), (json!({"tab": "layouts"}), "layouts"), (json!({"tab": "templates"}), "templates")] {
        let r = page(cx, &staged, props.clone(), "setShared('t.tab', this.tab);");
        assert_eq!(r.shared["t.tab"], tab, "{props}");
        for each in ["mods", "layouts", "templates"] {
            assert!(r.tree.contains(&format!("md-tab-{each}")), "{props}: the {each} tab: {}", r.tree);
        }
        let templates = tab == "templates";
        assert_eq!(r.tree.contains("tp-new"), templates, "{props}: the templates' buttons: {}", r.tree);
        assert_eq!(r.tree.contains("md-install-link"), !templates, "{props}: the mods' buttons: {}", r.tree);
        // Templates are read only once their tab shows.
        assert_eq!(made(&r).len(), usize::from(templates), "{props}");
    }
}

#[gpui::test]
fn the_templates_tab_takes_the_machine_and_template_it_was_asked_for(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    let r = page(cx, &staged, json!({"tab": "templates", "machine": "remote", "edit": "reviewer"}), "");
    assert_eq!(made(&r), [json!({"machine": "remote", "edit": "reviewer"})]);
    let r = page(cx, &staged, json!({"tab": "templates"}), "");
    assert_eq!(made(&r), [json!({"machine": null, "edit": null})]);
}

#[gpui::test]
fn switching_tabs_keeps_the_templates_tab_and_reads_the_mods_again(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    let r = page(cx, &staged, json!({"tab": "templates"}), "this.showTab('layouts', cx); this.showTab('templates', cx); this.showTab('mods', cx);");
    assert_eq!(made(&r).len(), 1, "made once, kept while the other tabs show");
    assert_eq!(r.shared["t.listed"], 2, "each mods or layouts tab shown reads the home's mods");
}

#[gpui::test]
fn install_from_link_asks_for_a_link_and_shows_where_the_files_come_from(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    let closed = page(cx, &staged, json!({}), "");
    assert!(!closed.tree.contains("md-link-go"));
    let open = page(cx, &staged, json!({}), "this.linking = true;");
    assert!(open.tree.contains("md-link-go") && open.tree.contains("md-link-cancel"), "{}", open.tree);

    let fetching = page(cx, &staged, json!({}), "this.install = { link: 'acme/demo', busy: true };");
    assert!(fetching.tree.contains("install_fetching") && fetching.tree.contains("in-cancel"), "{}", fetching.tree);

    let plan = "{ kind: 'mod', source: 'https://github.com/acme/demo', commit: '1a2b3c4', mods: [{ from: '/tmp/t/demo', as: 'demo' }], \
                layouts: [], keys: false, reaches: ['network'], files: 1, bytes: 9, token: 't1' }";
    let shown = page(cx, &staged, json!({}), &format!("this.install = {{ link: 'acme/demo', plan: {plan} }};"));
    for part in ["trust_title", "install_from https://github.com/acme/demo", "install_commit", "in-go"] {
        assert!(shown.tree.contains(part), "{part}: {}", shown.tree);
    }

    let failed = page(cx, &staged, json!({}), "this.install = { link: 'acme/demo', github: { repo: 'acme/demo', ref: null, folder: '' }, error: 'fetch_failed', message: 'HTTP 404' };");
    for part in ["install_errors.fetch_failed", "HTTP 404", "github_repo acme/demo", "github_ref github_default", "github_folder github_root"] {
        assert!(failed.tree.contains(part), "{part}: {}", failed.tree);
    }
    assert!(!failed.tree.contains("in-go"));
    // An error with nothing more to say than its code shows its sentence once.
    let bare = page(cx, &staged, json!({}), "this.install = { link: 'acme/plain', error: 'not_a_mod', message: 'not_a_mod' };");
    assert!(bare.tree.contains("text \"install_errors.not_a_mod\""), "{}", bare.tree);
    assert!(!bare.tree.contains("text \"not_a_mod\""), "{}", bare.tree);
}

// Read, shown, and installed only when "I trust it: install" is pressed (its handler, doInstall, a second in).
#[gpui::test]
fn a_link_is_installed_only_once_the_person_trusts_its_plan(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    let link = "https://github.com/acme/kit/tree/feature/x/side";
    let plan = json!({"kind": "mod", "source": link, "commit": "1a2b3c4", "mods": [{"from": "/tmp/t/side", "as": "side"}], "layouts": [],
                      "keys": false, "reaches": [], "files": 1, "bytes": 9, "token": "t1"});
    let answer = json!({"ok": true, "plan": plan, "github": {"repo": "acme/kit", "ref": "feature", "folder": "x/side"}});
    let typed = format!("this.linkInput = InputState.new({{ value: '{link}' }}); this.fetchLink(cx); cx.timer.after(500, (cx) => this.doInstall(cx));");
    let [(shown, read), (done, installed)] = answered(cx, &staged, answer, &typed);
    assert_eq!(read, [json!(["planFetch", link])], "the link is read, and nothing is installed");
    for part in ["trust_title", "github_repo acme/kit", "github_ref feature", "github_folder x/side", "install_commit 1a2b3c4", "in-go"] {
        assert!(shown.contains(part), "{part}: {shown}");
    }
    assert_eq!(installed, [json!(["planFetch", link]), json!(["installFolder", "t1"])], "pressed, the plan it showed is installed");
    assert!(done.contains("installed"), "{done}");
}

// The settings frame: app.settings {section: "templates"} and the old mod name both open the merged page's
// Templates tab, unless a page of its own still claims them (an edited copy of the old page left in the home).
#[gpui::test]
fn asking_for_the_old_templates_page_opens_the_templates_tab(cx: &mut TestAppContext) {
    shell_init(cx);
    let staged = Staged::new();
    std::fs::write(staged.0.join("main.js"),
        "import { View, div } from 'gpui-kit'; import { setShared } from 'herdr'; \
         import { find, open } from './frame/settings/yuzora.js'; \
         const merged = { mod: 'yuzora-settings-mods', section: 'mods', label: 'Mods & Templates' }; \
         const edited = { mod: 'yuzora-settings-templates', section: 'templates', label: 'Templates' }; \
         export default class Probe extends View { \
           init() { \
             setShared('t.found', ['templates', 'yuzora-settings-templates', 'mods', 'git'].map((a) => find(a, [merged]))); \
             setShared('t.edited', find('templates', [merged, edited])); \
             open(find('templates', [merged]), null, 0); \
             open(merged, null, 0); \
           } \
           render() { return div(); } \
         }").unwrap();
    let r = dock_copy(cx, &staged, "", json!({}), &Value::Null, Value::Null, &[], &[], None);
    let tab = json!({"mod": "yuzora-settings-mods", "section": "mods", "label": "Mods & Templates", "tab": "templates"});
    let merged = json!({"mod": "yuzora-settings-mods", "section": "mods", "label": "Mods & Templates"});
    assert_eq!(r.shared["t.found"], json!([tab, tab, merged, null]));
    assert_eq!(r.shared["t.edited"]["mod"], "yuzora-settings-templates");
    let page_props = |o: &Value| o["layout"]["children"][1]["content"]["props"].clone();
    assert_eq!(r.overlays.iter().map(page_props).collect::<Vec<_>>(), [json!({"section": "mods", "tab": "templates"}), json!({"section": "mods"})]);
}
