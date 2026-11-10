// yuzora(v0.1.0 files-open): included into `window::tests`. Files replaces a pane with `pane.split` and `pane.close`
// (open.js `replace`). A mod is not told the id of the pane the split made, so this pins what the commands keep: in the
// tab shown, where the new pane is the focused one and `pane.swap` and `pane.zoom` default to the focused pane, all of
// it; in a tab not shown, only the pane's place. docs/beta2/files.md ("換 pane 不是原地替換") is checked against it.

#[test]
fn what_a_pane_replaced_by_split_and_close_keeps_in_the_tab_shown_and_in_one_not_shown() {
    in_own_process("pane_replace_limits");
}

#[gpui::test]
fn pane_replace_limits(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("pane_replace_limits") {
        return;
    }
    let probe = "import { View, div } from \"gpui-kit\";\nexport default class Probe extends View { render() { return div(); } }\n";
    let layout = json!({ "id": "root", "type": "row", "children": [
        { "id": "tabs", "type": "block", "content": { "tab": "active" } },
        { "id": "probe", "type": "block", "content": { "mod": "pane-probe" } }
    ] });
    let (view, cx) = open_window(cx, "pane-probe", probe, layout);
    let settle = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
    };
    let props = |n: u32| json!({ "path": format!("/srv/{n}.rs"), "on": null, "opened": n });
    let mk = |id: &str| crate::layout::Node::block(id, crate::layout::Content::Mod { name: "pane-probe".into(), props: props(1) });
    let weights = |this: &super::AppWindow, tab: &str| -> Vec<f32> {
        this.model
            .tab(tab)
            .unwrap()
            .root
            .children()
            .iter()
            .map(|n| match n.size {
                Some(crate::layout::Size::Weight { weight }) => weight,
                other => panic!("a weight was expected, not {other:?}"),
            })
            .collect()
    };
    let panes = |this: &super::AppWindow, tab: &str| -> Vec<String> { this.model.tab(tab).unwrap().root.panes().iter().map(|n| n.id.clone()).collect() };
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
            for (tab, pane) in [("size", "a1"), ("first", "b1"), ("swap", "c1"), ("zoom", "d1"), ("away", "e1"), ("shown", "f1"), ("hides", "g1"), ("zoomed", "h1")] {
                this.model.add_tab("sp1", crate::model::Tab::with_pane(tab, tab, mk(pane)));
            }
            this.model.select_tab("tb1");
            this.sync(window, cx);
            this.documents.get_mut("").expect("this machine's slot").available = true;
        })
    });
    settle(cx);

    // Three panes in a row, 1/2, 1/4 and 1/4 of it: `replace` is `pane.split right` of the old pane, then `pane.close` of it.
    let three = |this: &mut super::AppWindow, tab: &str, first: &str, window: &mut gpui::Window, cx: &mut gpui::Context<super::AppWindow>| {
        this.run("pane.split", json!({ "direction": "right", "pane": first, "mod": "pane-probe", "props": props(2) }), window, cx);
        let second = panes(this, tab)[1].clone();
        this.run("pane.split", json!({ "direction": "right", "pane": second, "mod": "pane-probe", "props": props(3) }), window, cx);
        assert_eq!(weights(this, tab), [0.5, 0.25, 0.25]);
        panes(this, tab)
    };
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            let ids = three(this, "size", "a1", window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": ids[1], "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.close", json!({ "pane": ids[1] }), window, cx);
            assert_eq!(
                weights(this, "size"),
                [0.625, 0.125, 0.25],
                "the new pane has half of the share the old one had, and the pane before it took the other half"
            );

            let ids = three(this, "first", "b1", window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": ids[0], "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.close", json!({ "pane": ids[0] }), window, cx);
            assert_eq!(weights(this, "first"), [0.5, 0.25, 0.25], "the first of the row has nothing before it: the new pane takes the share back");

            let ids = three(this, "swap", "c1", window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": ids[1], "mod": "pane-probe", "props": props(9) }), window, cx);
            let new = panes(this, "swap").into_iter().find(|p| !ids.contains(p)).expect("the new pane");
            this.run("pane.swap", json!({ "pane": ids[1], "target": new }), window, cx);
            this.run("pane.close", json!({ "pane": ids[1] }), window, cx);
            assert_eq!(
                weights(this, "swap"),
                [0.5, 0.25, 0.25],
                "with the new pane's id, a swap before the close keeps the share: a mod is not told that id of a pane in a tab not shown"
            );
        })
    });
    settle(cx);

    // A zoomed tab: the pane the zoom hides is replaced.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.run("pane.split", json!({ "direction": "right", "pane": "d1", "mod": "pane-probe", "props": props(2) }), window, cx);
            let hidden = panes(this, "zoom")[1].clone();
            this.model.tab_mut("zoom").unwrap().focused = Some("d1".into());
            this.run("pane.zoom", json!({ "pane": "d1" }), window, cx);
            assert_eq!(this.model.tab("zoom").unwrap().zoomed.as_deref(), Some("d1"));
            this.run("pane.split", json!({ "direction": "right", "pane": hidden, "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.close", json!({ "pane": hidden }), window, cx);
            let tab = this.model.tab("zoom").unwrap();
            assert_eq!(tab.zoomed, None, "the split put the zoom away");
            let new = panes(this, "zoom").into_iter().find(|p| p != "d1").unwrap();
            assert_eq!(tab.focused.as_deref(), Some(new.as_str()), "and the new pane has the tab's focus, not the zoomed one");
            this.run("pane.zoom", json!({ "pane": "d1" }), window, cx);
            let tab = this.model.tab("zoom").unwrap();
            assert_eq!(
                (tab.zoomed.as_deref(), tab.focused.as_deref()),
                (Some("d1"), Some(new.as_str())),
                "zooming it again, which needs the pane's id too, leaves the focus under the zoom"
            );
        })
    });
    settle(cx);

    // A tab not shown, where the pane replaced is not the one the tab remembers as focused.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.run("pane.split", json!({ "direction": "right", "pane": "e1", "mod": "pane-probe", "props": props(2) }), window, cx);
            let ids = panes(this, "away");
            this.model.tab_mut("away").unwrap().focused = Some("e1".into());
            this.run("pane.split", json!({ "direction": "right", "pane": ids[1], "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.close", json!({ "pane": ids[1] }), window, cx);
            let new = panes(this, "away").into_iter().find(|p| p != "e1").unwrap();
            assert_eq!(this.model.tab("away").unwrap().focused.as_deref(), Some(new.as_str()), "the tab now remembers the new pane");
            let shown = this.model.active_tab().map(|t| t.id.clone());
            assert_eq!(shown.as_deref(), Some("tb1"), "no tab was shown by it");
            assert!(!this.copies.contains_key(&new), "and the new pane has not started");

            let swapped = this.run_as("pane.swap", json!({ "pane": new, "direction": "left" }), false, window, cx);
            assert!(swapped["error"].as_str().is_some_and(|e| e.contains("in the shown tab")), "swap by direction needs the tab shown: {swapped}");
            this.run("pane.focus", json!({ "pane": "e1" }), window, cx);
            assert_eq!(this.model.active_tab().map(|t| t.id.clone()).as_deref(), Some("away"), "the focus command shows the tab");
        })
    });
    settle(cx);
    // The tab shown: the pane the split makes has the focus, so the default pane of `pane.swap` and `pane.zoom` is it.
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.model.select_tab("shown");
            let ids = three(this, "shown", "f1", window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": ids[1], "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.swap", json!({ "target": ids[1] }), window, cx);
            this.run("pane.close", json!({ "pane": ids[1] }), window, cx);
            assert_eq!(weights(this, "shown"), [0.5, 0.25, 0.25], "swapping the focused pane with the old one keeps the share");
            let new = panes(this, "shown")[1].clone();
            assert!(!ids.contains(&new), "the new pane is in the old one's place");
            assert_eq!(this.model.tab("shown").unwrap().focused.as_deref(), Some(new.as_str()));

            this.model.select_tab("hides");
            this.run("pane.split", json!({ "direction": "right", "pane": "g1", "mod": "pane-probe", "props": props(2) }), window, cx);
            let hidden = panes(this, "hides")[1].clone();
            this.run("pane.focus", json!({ "pane": "g1" }), window, cx);
            this.run("pane.zoom", json!({ "pane": "g1" }), window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": hidden, "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.swap", json!({ "target": hidden }), window, cx);
            this.run("pane.close", json!({ "pane": hidden }), window, cx);
            this.run("pane.zoom", json!({ "pane": "g1" }), window, cx);
            this.run("pane.focus", json!({ "pane": "g1" }), window, cx);
            let tab = this.model.tab("hides").unwrap();
            assert_eq!((tab.zoomed.as_deref(), tab.focused.as_deref()), (Some("g1"), Some("g1")), "the zoom is back, with the focus on the zoomed pane");
            assert_eq!(weights(this, "hides"), [0.5, 0.5]);

            this.model.select_tab("zoomed");
            this.run("pane.split", json!({ "direction": "right", "pane": "h1", "mod": "pane-probe", "props": props(2) }), window, cx);
            this.run("pane.focus", json!({ "pane": "h1" }), window, cx);
            this.run("pane.zoom", json!({ "pane": "h1" }), window, cx);
            this.run("pane.split", json!({ "direction": "right", "pane": "h1", "mod": "pane-probe", "props": props(9) }), window, cx);
            this.run("pane.swap", json!({ "target": "h1" }), window, cx);
            this.run("pane.close", json!({ "pane": "h1" }), window, cx);
            this.run("pane.zoom", json!({}), window, cx);
            let new = panes(this, "zoomed")[0].clone();
            let tab = this.model.tab("zoomed").unwrap();
            assert_ne!(new, "h1");
            assert_eq!((tab.zoomed.as_deref(), tab.focused.as_deref()), (Some(new.as_str()), Some(new.as_str())), "the zoom is on the pane that took the zoomed one's place");
            assert_eq!(weights(this, "zoomed"), [0.5, 0.5]);
            this.model.select_tab("tb1");
        })
    });
    settle(cx);
    view.read_with(cx, |this, _| {
        let started: Vec<bool> = panes(this, "away").iter().map(|p| this.copies.contains_key(p)).collect();
        assert_eq!(started, [true, true], "and starts the panes of that tab that had not started");
    });
}
