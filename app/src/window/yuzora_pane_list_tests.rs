// yuzora(v0.1.0 files-open): included into `window::tests`, in a file of its own so upstream's new tests in
// window.rs never meet these in a merge.

#[test]
fn a_mod_pane_in_a_tab_not_shown_yet_is_listed_with_its_props_but_has_not_started() {
    in_own_process("pane_props_listed");
}

// yuzora(v0.1.0 files-open): a restored layout starts only the tab shown, so the mods of its other tabs'
// panes have no copy to ask: `panes()` is the one place that says which file such a pane holds.
#[gpui::test]
fn pane_props_listed(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("pane_props_listed") {
        return;
    }
    let probe = r#"import { View, div } from "gpui-kit";
import { panes, props, shared, setShared } from "herdr";

export default class Probe extends View {
  init() {
    if (props()?.path) setShared("probe.started", [...(shared("probe.started") ?? []), props().path]);
  }
  render() {
    if (!props()?.path) setShared("probe.panes", panes());
    return div();
  }
}
"#;
    let layout = json!({ "id": "root", "type": "row", "children": [
        { "id": "tabs", "type": "block", "content": { "tab": "active" } },
        { "id": "probe", "type": "block", "content": { "mod": "pane-probe" } }
    ] });
    let (view, cx) = open_window(cx, "pane-probe", probe, layout);
    let props = json!({ "path": "/srv/a.rs", "on": "lab", "opened": 1 });
    let settle = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
    };
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
        let pane = crate::layout::Node::block("lab/pa175", crate::layout::Content::Mod { name: "pane-probe".into(), props: props.clone() });
        this.model.add_tab("sp1", crate::model::Tab::with_pane("tb2", "a.rs", pane));
        this.model.select_tab("tb1");
        this.sync(window, cx);
    }));
    settle(cx);
    let listed = |id: &str| crate::mods::shared("probe.panes").and_then(|v| v.as_array()?.iter().find(|p| p["pane"] == id).cloned());
    let pane = listed("lab/pa175").expect("the pane of the tab not shown is listed");
    assert_eq!((&pane["mod"], &pane["props"]), (&json!("pane-probe"), &props));
    assert_eq!(pane["started"], json!(false), "no copy of its mod runs: nothing has opened its file");
    assert!(listed("p1").expect("the terminal is listed").get("props").is_none());
    assert!(listed("p1").unwrap().get("started").is_none(), "a terminal has no copy of a mod");
    assert_eq!(crate::mods::shared("probe.started"), None, "its tab has not been shown: no copy of the mod has started");
    assert!(!view.read_with(cx, |this, _| this.copies.contains_key("lab/pa175")));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.model.select_tab("tb2");
        this.sync(window, cx);
    }));
    settle(cx);
    assert_eq!(crate::mods::shared("probe.started"), Some(json!(["/srv/a.rs"])), "shown, it starts with the props it is listed with");
    assert_eq!(listed("lab/pa175").unwrap()["started"], json!(true), "and is listed as started");
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.model.select_tab("tb1");
        this.sync(window, cx);
    }));
    settle(cx);
    assert_eq!(listed("lab/pa175").unwrap()["started"], json!(true), "its copy keeps running when its tab is put away");
}

#[test]
fn a_pane_not_started_is_replaced_in_place_and_its_tab_shown_without_the_old_one_starting() {
    in_own_process("pane_replaced_in_place");
}

// yuzora(v0.1.0 files-open): what Files asks for a file whose pane waits in a tab not shown yet: a pane beside it with
// other props, the old one closed, the tab selected. The old copy never starts, the new one does, with the tab's focus.
#[gpui::test]
fn pane_replaced_in_place(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("pane_replaced_in_place") {
        return;
    }
    let probe = r#"import { View, div } from "gpui-kit";
import { props, shared, setShared } from "herdr";

export default class Probe extends View {
  init() {
    if (props()?.path) setShared("probe.started", [...(shared("probe.started") ?? []), JSON.stringify(props())]);
  }
  render() { return div(); }
}
"#;
    let layout = json!({ "id": "root", "type": "row", "children": [
        { "id": "tabs", "type": "block", "content": { "tab": "active" } },
        { "id": "probe", "type": "block", "content": { "mod": "pane-probe" } }
    ] });
    let (view, cx) = open_window(cx, "pane-probe", probe, layout);
    let settle = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
    };
    let old = json!({ "path": "/srv/a.rs", "on": null, "opened": 1 });
    let new = json!({ "path": "/srv/a.rs", "on": null, "opened": 2, "line": 7, "column": 2 });
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
        let pane = crate::layout::Node::block("pf1", crate::layout::Content::Mod { name: "pane-probe".into(), props: old.clone() });
        this.model.add_tab("sp1", crate::model::Tab::with_pane("tb2", "a.rs", pane));
        this.model.select_tab("tb1");
        this.sync(window, cx);
    }));
    settle(cx);
    cx.update(|window, cx| view.update(cx, |this, cx| {
        // The layout document of this machine is read: pane commands wait for it.
        this.documents.get_mut("").expect("this machine's slot").available = true;
        this.run("pane.split", json!({ "direction": "right", "pane": "pf1", "mod": "pane-probe", "props": new.clone() }), window, cx);
        this.run("pane.close", json!({ "pane": "pf1" }), window, cx);
        this.run("tab.select", json!({ "id": "tb2" }), window, cx);
    }));
    settle(cx);
    let (shown, panes, focused) = view.read_with(cx, |this, _| {
        let tab = this.model.tab("tb2").expect("the tab is still there");
        let ids: Vec<String> = tab.root.panes().iter().map(|n| n.id.clone()).collect();
        (this.model.active_tab().map(|t| t.id.clone()), ids, this.model.focused_pane())
    });
    assert_eq!(shown.as_deref(), Some("tb2"), "the tab with the new pane is shown");
    assert_eq!(panes.len(), 1, "the old pane is gone: {panes:?}");
    assert_ne!(panes[0], "pf1");
    assert_eq!(focused.as_deref(), Some(panes[0].as_str()), "the new pane has the tab's focus");
    assert_eq!(
        crate::mods::shared("probe.started"),
        Some(json!([new.to_string()])),
        "only the new pane started, with the props it was given"
    );
}
