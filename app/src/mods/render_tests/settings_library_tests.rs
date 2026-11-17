//! Settings > Mods & Templates (docs/beta2/settings.md): one page with the Mods, Layouts and Templates tabs, and
//! Install from link beside Install from folder. The old Templates page is its Templates tab now: asking for it by
//! its section or mod name opens that tab.
use super::*;

/// The page with `props`, nothing of the person's read: the mods listing and the app status are swapped out
/// (a call of `list` counts in `t.listed`), and the Templates tab's reading records what it was made with in
/// `t.made` instead of asking hc. `setup` runs at the end of init.
fn page(cx: &mut TestAppContext, staged: &Staged, props: Value, setup: &str) -> DockCopy {
    std::fs::write(staged.0.join("main.js"), format!(
        "import Real from './mods/yuzora-settings-mods/main.js'; \
         import {{ Templates }} from './mods/yuzora-settings-mods/templates.js'; \
         import {{ shared, setShared }} from 'herdr'; \
         Templates.prototype.reload = function () {{ \
           setShared('t.made', [...(shared('t.made') ?? []), {{ machine: this.machine, edit: this.pendingEdit }}]); \
           this.list = {{ dir: '/x/templates', templates: [] }}; }}; \
         export default class Scenario extends Real {{ init(props, cx) {{ \
           this.refresh = () => {{}}; this.list = () => setShared('t.listed', (shared('t.listed') ?? 0) + 1); \
           super.init(props, cx); {setup} }} }}"
    )).unwrap();
    dock_copy(cx, staged, "", props, &Value::Null, Value::Null, &[], &[], None)
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
    assert!(fetching.tree.contains("install_fetching"), "{}", fetching.tree);

    let plan = "{ kind: 'mod', source: 'https://github.com/acme/demo', commit: '1a2b3c4', mods: [{ from: '/tmp/t/demo', as: 'demo' }], \
                layouts: [], keys: false, reaches: ['network'], files: 1, bytes: 9, token: 't1' }";
    let shown = page(cx, &staged, json!({}), &format!("this.install = {{ link: 'acme/demo', plan: {plan} }};"));
    for part in ["trust_title", "install_from https://github.com/acme/demo", "install_commit", "in-go"] {
        assert!(shown.tree.contains(part), "{part}: {}", shown.tree);
    }

    let failed = page(cx, &staged, json!({}), "this.install = { link: 'acme/demo', error: 'fetch_failed', message: 'https://github.com/acme/demo: HTTP 404' };");
    for part in ["install_errors.fetch_failed", "https://github.com/acme/demo: HTTP 404"] {
        assert!(failed.tree.contains(part), "{part}: {}", failed.tree);
    }
    assert!(!failed.tree.contains("in-go"));
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
