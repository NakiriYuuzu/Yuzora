// yuzora(dock): included into `window::tests`, in a file of its own so upstream's new tests in window.rs never
// meet these in a merge. `include_str!` here reads from src/window/.

// yuzora(dock): W1-W3 and the guard below: the right dock never reaches a layout file.

/// A personal layout shaped like `main`: a `panel` column with a `panel-body` stack in the root row.
fn panel_layout() -> serde_json::Value {
    serde_json::json!({
        "id": "root", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "hidden": true, "size": { "px": 368 }, "children": [
                { "id": "panel-body", "type": "stack", "active": "files", "children": [
                    { "id": "files", "type": "block", "content": { "mod": "files" } },
                    { "id": "changes", "type": "block", "content": { "mod": "changes" } } ] } ] } ]
    })
}

/// No node of the dock's, and the resource chip (in every layout, the dock on or off).
fn chip_without_dock(n: &crate::layout::Node) -> bool {
    let chip = ["yuzora-dock-root", "yuzora-dock-body", "yuzora-dock-perf"];
    n.walk().iter().all(|x| !x.id.starts_with("yuzora-dock") || chip.contains(&x.id.as_str())) && n.find("yuzora-dock-perf").is_some()
}

fn layout_file() -> String {
    std::fs::read_to_string(super::layout_path("test")).unwrap()
}

#[test]
fn the_dock_never_reaches_a_personal_layout_file() {
    in_own_process("dock_never_reaches_file");
}

#[gpui::test]
fn dock_never_reaches_file(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_never_reaches_file") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let original = panel_layout().to_string();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    assert_eq!(layout_file(), original, "showing a layout writes nothing");
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some() && this.file_chrome.find("yuzora-dock-bar").is_some(), "the dock is in both trees"));

    // Clicks on the dock (a mode, the panel open) are the person's, kept in .state only.
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
        this.run("layout.show", json!({ "id": "panel" }), window, cx);
        this.save();
    }));
    assert_eq!(layout_file(), original, "clicks write no layout file");
    let state = std::fs::read_to_string(super::state_path("test")).unwrap();
    assert!(state.contains("yuzora-dock-git"), "{state}");

    // Shown again (a restart): the dock is back, on Git.
    view.update(cx, |this, _| {
        this.load_layout("test");
        assert!(this.chrome.find("yuzora-dock-bar").is_some());
        assert!(!this.chrome.find("panel").unwrap().hidden);
        assert!(matches!(&this.chrome.find("panel-body").unwrap().kind, crate::layout::Kind::Stack { active, .. } if active.as_deref() == Some("yuzora-dock-git")));
    });
    assert_eq!(layout_file(), original);

    // A mod docking a block is an edit: the file gets it, never the dock.
    view.update(cx, |this, _| {
        this.chrome.children_mut().unwrap().push(crate::layout::Node::block("docked", crate::layout::Content::Mod { name: "unused".into(), props: serde_json::Value::Null }));
        this.save();
    });
    let written = layout_file();
    assert!(written.contains("docked") && !written.contains("yuzora-dock"), "{written}");
    view.update(cx, |this, _| {
        assert!(this.chrome.find("yuzora-dock-bar").is_some(), "still shown");
        assert!(this.file_chrome.find("yuzora-dock-bar").is_some());
    });
    let after_save = layout_file();

    // layout.set with the tree a mod read from layout() (dock and all) plus a change.
    cx.update(|window, cx| view.update(cx, |this, cx| {
        let mut tree = serde_json::to_value(&this.chrome).unwrap();
        tree["children"].as_array_mut().unwrap().push(json!({ "id": "second", "type": "block", "content": { "mod": "unused" } }));
        this.run("layout.set", json!({ "layout": tree }), window, cx);
    }));
    let written = layout_file();
    assert_ne!(written, after_save);
    assert!(written.contains("second") && !written.contains("yuzora-dock"), "{written}");
    let back: crate::layout::Node = serde_json::from_str(&written).unwrap();
    assert!(matches!(&back.find("panel-body").unwrap().kind, crate::layout::Kind::Stack { active, .. } if active.as_deref() == Some("files")), "the stack is back on the file's own choice");
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some()));

    // A tree built without the dock (a script, not layout()) is shown with it all the same.
    cx.update(|window, cx| view.update(cx, |this, cx| {
        let tree = serde_json::to_value(crate::yuzora::dock::strip(&this.chrome, None)).unwrap();
        this.run("layout.set", json!({ "layout": tree }), window, cx);
    }));
    assert!(!layout_file().contains("yuzora-dock"));
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some() && this.file_chrome.find("yuzora-dock-bar").is_some()));
}

#[test]
fn the_dock_follows_a_file_edit() {
    in_own_process("dock_follows_file_edit");
}

#[gpui::test]
fn dock_follows_file_edit(cx: &mut gpui::TestAppContext) {
    if !running_alone("dock_follows_file_edit") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some()));
    let mut off = panel_layout();
    off["yuzora"] = serde_json::json!({ "dock": "off" });
    std::fs::write(super::layout_path("test"), off.to_string()).unwrap();
    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
    view.update(cx, |this, _| {
        assert!(chip_without_dock(&this.chrome) && chip_without_dock(&this.file_chrome));
        assert!(!super::file("layout.error").exists(), "the opt-out is a valid layout");
    });
    // And back: taking the opt-out out brings the dock back.
    std::fs::write(super::layout_path("test"), panel_layout().to_string()).unwrap();
    let mtime = || std::fs::metadata(super::layout_path("test")).unwrap().modified().unwrap();
    let written_at = mtime();
    std::thread::sleep(std::time::Duration::from_millis(20));
    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
    assert_eq!(layout_file(), panel_layout().to_string(), "reading the edit rewrites nothing (the dock is in the file's tree too, so it is no edit)");
    assert_eq!(mtime(), written_at, "and the file isn't touched");
    let before_save = layout_file();
    view.update(cx, |this, _| {
        assert!(this.chrome.find("yuzora-dock-bar").is_some());
        assert!(this.file_chrome.find("yuzora-dock-bar").is_some(), "the file's tree has the dock too, or its clicks are edits");
        this.save();
    });
    assert_eq!(layout_file(), before_save, "a save after a file edit rewrites nothing");
}

// W3b: a file edit with an id-less node is given its ids (and written) before the dock goes in.
#[test]
fn a_file_edit_without_ids_never_writes_the_dock() {
    in_own_process("dock_watcher_keep_ids");
}

#[gpui::test]
fn dock_watcher_keep_ids(cx: &mut gpui::TestAppContext) {
    if !running_alone("dock_watcher_keep_ids") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let mut edited = panel_layout();
    edited["children"].as_array_mut().unwrap().push(serde_json::json!({ "type": "block", "content": { "mod": "unused" } }));
    std::fs::write(super::layout_path("test"), edited.to_string()).unwrap();
    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
    let written = layout_file();
    assert!(written.contains("node-1"), "keep_ids wrote the file: {written}");
    assert!(!written.contains("yuzora-dock"), "{written}");
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some() && this.file_chrome.find("yuzora-dock-bar").is_some()));
}

#[test]
fn keep_ids_never_writes_the_dock() {
    in_own_process("keep_ids_never_writes_dock");
}

#[gpui::test]
fn keep_ids_never_writes_dock(cx: &mut gpui::TestAppContext) {
    if !running_alone("keep_ids_never_writes_dock") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    // A node without an id: loading gives it one and writes the file (keep_ids).
    let mut layout = panel_layout();
    layout["children"].as_array_mut().unwrap().push(serde_json::json!({ "type": "block", "content": { "mod": "unused" } }));
    let (view, cx) = open_window(cx, "unused", "", layout);
    let written = layout_file();
    assert!(written.contains("node-1"), "keep_ids wrote the file: {written}");
    assert!(!written.contains("yuzora-dock"), "{written}");
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some(), "yet the dock shows"));
}

/// What the dock shows after a click: the stack's mode, whether the panel and the bar are hidden.
fn dock_view(n: &crate::layout::Node) -> (Option<String>, bool, bool) {
    let active = match &n.find("panel-body").unwrap().kind {
        crate::layout::Kind::Stack { active, .. } => active.clone(),
        _ => None,
    };
    (active, n.find("panel").unwrap().hidden, n.find("yuzora-dock-bar").unwrap().hidden)
}

// W1b (B1): layout.set again and again, then a structural save: the file never has the dock, and keeps the person's own choice.
#[test]
fn layout_set_twice_and_a_save_never_write_the_dock() {
    in_own_process("dock_set_twice");
}

#[gpui::test]
fn dock_set_twice(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_set_twice") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let set = |name: &str, cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                let mut tree = serde_json::to_value(&this.chrome).unwrap();
                tree["children"].as_array_mut().unwrap().push(json!({ "id": name, "type": "block", "content": { "mod": "unused" } }));
                this.run("layout.set", json!({ "layout": tree }), window, cx);
            })
        });
    };
    let own_choice = |cx: &mut gpui::VisualTestContext| {
        let written = layout_file();
        assert!(!written.contains("yuzora-dock"), "{written}");
        let back: crate::layout::Node = serde_json::from_str(&written).unwrap();
        assert_eq!(dock_choice(&back), Some("files".to_string()), "{written}");
        view.update(cx, |this, _| assert!(this.file_chrome.find("yuzora-dock-bar").is_some() && this.chrome.find("yuzora-dock-bar").is_some()));
    };
    fn dock_choice(n: &crate::layout::Node) -> Option<String> {
        match &n.find("panel-body")?.kind {
            crate::layout::Kind::Stack { active, .. } => active.clone(),
            _ => None,
        }
    }
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx)));
    set("one", cx);
    own_choice(cx);
    set("two", cx);
    own_choice(cx);
    view.update(cx, |this, _| {
        this.chrome.children_mut().unwrap().push(crate::layout::Node::block("three", crate::layout::Content::Mod { name: "unused".into(), props: serde_json::Value::Null }));
        this.save();
    });
    own_choice(cx);
    assert!(layout_file().contains("three"));
    view.update(cx, |this, _| assert_eq!(dock_view(&this.chrome).0.as_deref(), Some("yuzora-dock-git"), "the click is still shown"));
}

// S2: the dock's clicks that came in with a layout.set are the same after a restart.
#[test]
fn dock_clicks_made_before_a_layout_set_survive_a_restart() {
    in_own_process("dock_set_restart");
}

#[gpui::test]
fn dock_set_restart(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_set_restart") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
            this.run("layout.show", json!({ "id": "panel" }), window, cx);
            this.run("layout.hide", json!({ "id": "yuzora-dock-bar" }), window, cx);
            let tree = serde_json::to_value(&this.chrome).unwrap();
            this.run("layout.set", json!({ "layout": tree }), window, cx);
            this.save();
        })
    });
    let before = view.update(cx, |this, _| dock_view(&this.chrome));
    assert_eq!(before, (Some("yuzora-dock-git".to_string()), false, true));
    let after = view.update(cx, |this, _| {
        this.load_layout("test");
        dock_view(&this.chrome)
    });
    assert_eq!(after, before);
    assert!(!layout_file().contains("yuzora-dock"));
}

// W4: the person's choice for a layout (settings `shell.dock.layouts.<name>`) applies at once, with
// `layout.switch` to the layout shown, and leaves the layout file alone and the dock's clicks in `.state`.
#[test]
fn a_persons_dock_choice_applies_at_once() {
    in_own_process("dock_personal_choice");
}

#[gpui::test]
fn dock_personal_choice(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_personal_choice") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let original = panel_layout().to_string();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let choose = |word: Option<&str>| {
        let shell = word.map_or(serde_json::Value::Null, |w| json!({ "dock": { "layouts": { "test": w } } }));
        yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, shell).unwrap();
    };
    let switch = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
        cx.run_until_parked();
    };
    // The dock on Git, panel wider than the file has it: clicks.
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
        this.run("layout.show", json!({ "id": "panel" }), window, cx);
        this.run("layout.resize", json!({ "id": "panel", "px": 420 }), window, cx);
    }));
    let shown = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| (dock_view(&this.chrome), this.chrome.find("panel").unwrap().size));
    let before = shown(cx);
    assert_eq!(before.0, (Some("yuzora-dock-git".to_string()), false, false));

    // Off: the dock is gone at once (in both trees), the layout file is the same bytes.
    choose(Some("off"));
    switch(cx);
    view.update(cx, |this, _| {
        assert!(chip_without_dock(&this.chrome) && chip_without_dock(&this.file_chrome));
    });
    assert_eq!(layout_file(), original);
    // Saved while it is off (a click elsewhere, a switch to the same layout): still nothing in the file.
    view.update(cx, |this, _| this.save());
    switch(cx);
    assert_eq!(layout_file(), original);

    // Auto again: the dock is back, on Git, at the width it was dragged to.
    choose(Some("auto"));
    switch(cx);
    assert_eq!(shown(cx), before, "the clicks on the dock survived the time it was off");
    assert_eq!(layout_file(), original);

    // Drawer: the panel's bar gives way to the drawer; auto brings the panel back.
    choose(Some("drawer"));
    switch(cx);
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_none() && this.chrome.find("yuzora-dock").is_none() && this.chrome.walk().iter().any(|n| n.floating.iter().any(|f| f.id == "yuzora-dock"))));
    choose(None);
    switch(cx);
    assert_eq!(shown(cx), before);
    assert!(!layout_file().contains("yuzora-dock"));

    // Over the layout file's own field: the file says off, the person says auto (the dock is shown); no choice, off.
    let mut off = panel_layout();
    off["yuzora"] = json!({ "dock": "off" });
    std::fs::write(super::layout_path("test"), off.to_string()).unwrap();
    choose(Some("auto"));
    switch(cx);
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some(), "the person's auto beats the file's off"));
    choose(None);
    switch(cx);
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_none(), "no choice: the file's off"));
}

// "When Yuzora opens" (general.startup.dock): the first bar of a run opens the dock on that mode (or closes it), once.
#[test]
fn the_first_dock_of_a_run_opens_the_mode_settings_ask_for() {
    in_own_process("dock_startup_mode");
}

#[gpui::test]
fn dock_startup_mode(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_startup_mode") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let general = |dock: &str| yuzora_core::settings::set_section(yuzora_core::settings::GENERAL, json!({ "startup": { "dock": dock } })).unwrap();
    general("database");
    let (view, cx) = open_window(cx, "yuzora-modebar", include_str!("../../defaults/mods/yuzora-modebar/main.js"), panel_layout());
    cx.run_until_parked();
    let shown = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| dock_view(&this.chrome));
    assert_eq!(shown(cx), (Some("yuzora-dock-database".to_string()), false, false), "the panel opens on the mode settings name");
    assert_eq!(crate::mods::shared("yuzora.startup.honored"), Some(json!(true)), "settings may show the choice now");

    // The person goes on to Files; a layout switch (the bar's mod loads again) does not take that back.
    std::fs::write(super::layout_path("other"), json!({ "id": "root", "type": "block", "yuzora": { "dock": "off" }, "content": { "tab": "active" } }).to_string()).unwrap();
    let switch = |name: &str, cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": name }), window, cx)));
        cx.run_until_parked();
    };
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.select", json!({ "stack": "panel-body", "child": "files" }), window, cx)));
    switch("other", cx);
    switch("test", cx);
    assert_eq!(shown(cx).0, Some("files".to_string()), "once per run");

    // A new run (another session) with none: the dock starts closed.
    crate::mods::set_shared("yuzora.startup.applied".into(), json!("an earlier run"));
    general("none");
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.show", json!({ "id": "panel" }), window, cx)));
    assert!(!shown(cx).1);
    switch("other", cx);
    switch("test", cx);
    assert!(shown(cx).1, "none: the panel is closed");
}

// yuzora(dock): PR-4 review r1 and acceptance fixes (S1, S2, S4, S5, panel left open).

/// The modes of the drawer's windows (key `yuzora-dock`) that are open.
fn dock_windows(this: &super::AppWindow) -> Vec<String> {
    this.overlays
        .iter()
        .filter(|o| o.key.as_deref() == Some("yuzora-dock"))
        .map(|o| {
            let l = o.layout.as_ref().map(|n| serde_json::to_value(n).unwrap()).unwrap_or_default();
            l["children"][0]["content"]["props"]["mode"].as_str().unwrap_or("?").to_owned()
        })
        .collect()
}

fn dock_window_spec(mode: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "window", "key": "yuzora-dock", "keep": true, "width": 380, "height": 0.85, "layout": { "id": "dock", "type": "column", "children": [
        { "id": "head", "type": "block", "size": { "px": 34 }, "content": { "mod": "unused", "props": { "dock": "head", "mode": mode } } },
        { "id": "body", "type": "block", "content": { "mod": "unused" } } ] } })
}

// S1: the drawer's window is the drawer's: once the layout shows no drawer (off, or auto with a panel),
// the window does not come back with a restart (or a switch back), and its keep is forgotten.
#[test]
fn the_drawers_window_does_not_outlive_the_drawer() {
    in_own_process("dock_window_outlives_drawer");
}

#[gpui::test]
fn dock_window_outlives_drawer(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_window_outlives_drawer") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let choose = |w: &str| yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, json!({ "dock": { "layouts": { "test": w } } })).unwrap();
    choose("drawer");
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let kept = || std::fs::read_to_string(super::file("windows.json")).unwrap_or_default().contains("\"yuzora-dock\"");
    for next in ["off", "auto"] {
        choose("drawer");
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
        view.update(cx, |this, _| assert!(this.chrome.floating.iter().any(|f| f.id == "yuzora-dock"), "the drawer"));
        cx.update(|window, cx| view.update(cx, |this, cx| this.open_overlay("o-dock".into(), dock_window_spec("git"), window, cx)));
        assert!(kept(), "{next}: kept while the drawer shows");
        choose(next);
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
        // The app quits (windows go without a close) and starts again: the kept windows of the layout reopen.
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.remove_overlay("o-dock", window, cx);
            this.open_kept_windows(window, cx);
        }));
        cx.run_until_parked();
        assert_eq!(view.update(cx, |this, _| dock_windows(this)), Vec::<String>::new(), "{next}: no window");
        assert!(!kept(), "{next}: not kept either");
        // Auto with a panel: the bar and no drawer.
        if next == "auto" {
            view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some() && this.chrome.floating.iter().all(|f| f.id != "yuzora-dock")));
        }
    }
    // A drawer that is there keeps its window.
    choose("drawer");
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.open_overlay("o-dock".into(), dock_window_spec("git"), window, cx);
        this.remove_overlay("o-dock", window, cx);
        this.open_kept_windows(window, cx);
    }));
    cx.run_until_parked();
    assert_eq!(view.update(cx, |this, _| dock_windows(this)), ["git"], "the drawer's window reopens with the drawer");
}

// S2: the layout file changes while the dock is off (or a drawer): the clicks on the dock are still there on its return.
#[test]
fn a_file_edit_while_the_dock_is_off_keeps_its_clicks() {
    in_own_process("dock_edit_while_off");
}

#[gpui::test]
fn dock_edit_while_off(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_edit_while_off") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let choose = |w: &str| yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, json!({ "dock": { "layouts": { "test": w } } })).unwrap();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let switch = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
        cx.run_until_parked();
    };
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
        this.run("layout.show", json!({ "id": "panel" }), window, cx);
        this.run("layout.hide", json!({ "id": "yuzora-dock-bar" }), window, cx);
    }));
    let before = view.update(cx, |this, _| dock_view(&this.chrome));
    assert_eq!(before, (Some("yuzora-dock-git".to_string()), false, true));
    for off in ["off", "drawer"] {
        choose(off);
        switch(cx);
        let mut edited = panel_layout();
        edited["children"].as_array_mut().unwrap().push(json!({ "id": format!("extra-{off}"), "type": "block", "content": { "mod": "unused" } }));
        std::fs::write(super::layout_path("test"), edited.to_string()).unwrap();
        cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
        assert!(view.update(cx, |this, _| this.chrome.find("panel").unwrap().hidden), "{off}: the panel opened for the dock stays shut after the edit");
        choose("auto");
        switch(cx);
        assert_eq!(view.update(cx, |this, _| dock_view(&this.chrome)), before, "{off}: the clicks survive an edit of the file meanwhile");
        std::fs::write(super::layout_path("test"), panel_layout().to_string()).unwrap();
        cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
    }
}

// The person's own panel (the tab bar's Files / Changes button) opened while the dock is off or a drawer outlives a file edit and a reload.
#[test]
fn a_panel_the_person_opened_while_the_dock_is_hidden_stays_open() {
    in_own_process("dock_own_panel_while_hidden");
}

#[gpui::test]
fn dock_own_panel_while_hidden(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_own_panel_while_hidden") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    std::fs::write(super::layout_path("other"), json!({ "id": "root", "type": "column", "children": [ { "id": "main", "type": "block", "content": { "tab": "active" } } ] }).to_string()).unwrap();
    let choose = |w: &str| yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, json!({ "dock": { "layouts": { "test": w } } })).unwrap();
    let switch = |cx: &mut gpui::VisualTestContext, name: &str| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": name }), window, cx)));
        cx.run_until_parked();
    };
    let panel = |cx: &mut gpui::VisualTestContext| {
        view.update(cx, |this, _| {
            let active = match &this.chrome.find("panel-body").unwrap().kind {
                crate::layout::Kind::Stack { active, .. } => active.clone(),
                _ => None,
            };
            (active, this.chrome.find("panel").unwrap().hidden)
        })
    };
    for mode in ["off", "drawer"] {
        for pick in ["files", "changes"] {
            for how in ["edit", "reload"] {
                let at = format!("{mode}/{pick}/{how}");
                std::fs::write(super::layout_path("test"), panel_layout().to_string()).unwrap();
                cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
                choose("auto");
                switch(cx, "test");
                cx.update(|window, cx| view.update(cx, |this, cx| {
                    this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
                    this.run("layout.show", json!({ "id": "panel" }), window, cx);
                }));
                choose(mode);
                switch(cx, "test");
                assert!(panel(cx).1, "{at}: the dock's panel is shut while it is hidden");
                // The person's own click: the tab bar's button.
                cx.update(|window, cx| view.update(cx, |this, cx| {
                    this.run("layout.select", json!({ "stack": "panel-body", "child": pick }), window, cx);
                    this.run("layout.show", json!({ "id": "panel" }), window, cx);
                    this.save();
                }));
                cx.run_until_parked();
                assert_eq!(panel(cx), (Some(pick.to_string()), false), "{at}: opened");
                if how == "edit" {
                    let mut edited = panel_layout();
                    edited["children"].as_array_mut().unwrap().push(json!({ "id": "extra", "type": "block", "content": { "mod": "unused" } }));
                    std::fs::write(super::layout_path("test"), edited.to_string()).unwrap();
                    cx.update(|window, cx| view.update(cx, |this, cx| this.check_layout_file(window, cx)));
                } else {
                    switch(cx, "other");
                    switch(cx, "test");
                }
                cx.run_until_parked();
                assert_eq!(panel(cx), (Some(pick.to_string()), false), "{at}: still the person's panel after the {how}");
                choose("auto");
                switch(cx, "test");
                assert_eq!(panel(cx), (Some(pick.to_string()), false), "{at}: back to auto it shows what the person chose");
            }
        }
    }
}

// S4: the drawer's startup mode ("When Yuzora opens"): the first drawer of a run opens that mode's window.
#[test]
fn the_first_drawer_of_a_run_opens_the_startup_mode() {
    in_own_process("dock_startup_drawer");
}

#[gpui::test]
fn dock_startup_drawer(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_startup_drawer") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    yuzora_core::settings::set_section(yuzora_core::settings::GENERAL, json!({ "startup": { "dock": "git" } })).unwrap();
    let home = crate::mods::app_dir();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("windows.json"), json!({ "kept": { "test": { "yuzora-dock": dock_window_spec("files") } } }).to_string()).unwrap();
    let layout = json!({ "id": "root", "type": "row", "children": [ { "id": "main", "type": "block", "content": { "tab": "active" } } ] });
    let (view, cx) = open_window(cx, "yuzora-modebar", include_str!("../../defaults/mods/yuzora-modebar/main.js"), layout);
    cx.run_until_parked();
    assert_eq!(view.update(cx, |this, _| dock_windows(this)), ["git"], "the startup mode's window replaced the kept one");
}

// S5: while the dock is off for a layout, `layout.set` of a shipped layout (the win98 desktop picker) is accepted,
// and one of a personal layout writes a file without the dock that a later save leaves alone.
#[test]
fn layout_set_while_the_dock_is_off_is_read_with_the_persons_choice() {
    in_own_process("dock_set_while_off");
}

#[gpui::test]
fn dock_set_while_off(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_set_while_off") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, json!({ "dock": { "layouts": { "win98": "off", "test": "off" } } })).unwrap();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    // Shipped (win98), as its desktop picker does it: layout() with another background.
    view.update(cx, |this, _| this.load_layout("win98"));
    let mut tree = view.update(cx, |this, _| serde_json::to_value(&this.chrome).unwrap());
    tree["background"] = json!("teal");
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", json!({ "layout": tree }), window, cx)));
    let painted = view.update(cx, |this, _| this.chrome.background == Some(crate::layout::Background::Color("teal".into())));
    assert!(painted, "an off dock is no edit: the shipped layout's set is accepted");
    // Personal: no dock in the tree (off), none in the file, and a save rewrites nothing.
    view.update(cx, |this, _| this.load_layout("test"));
    let mut tree = view.update(cx, |this, _| serde_json::to_value(&this.chrome).unwrap());
    tree["children"].as_array_mut().unwrap().push(json!({ "id": "extra", "type": "block", "content": { "mod": "unused" } }));
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", json!({ "layout": tree }), window, cx)));
    let written = layout_file();
    assert!(written.contains("extra") && !written.contains("yuzora-dock"), "{written}");
    // What was written reads back as the person's off shows it (the dock's own nodes are none of its tree). A save
    // right after `layout.set` makes up for a wrong answer here, so this asks `as_read` itself.
    let tree: crate::layout::Node = serde_json::from_str(&written).unwrap();
    let read = crate::yuzora::dock::as_read(&tree, "test", None);
    assert!(chip_without_dock(&read) && read.floating.is_empty(), "off for `test`: nothing of the dock but the chip");
    assert!(crate::yuzora::dock::as_read(&tree, "another", None).find("yuzora-dock-bar").is_some(), "no choice for `another`: the dock");
    view.update(cx, |this, _| this.save());
    assert_eq!(layout_file(), written);
    view.update(cx, |this, _| assert!(chip_without_dock(&this.chrome) && chip_without_dock(&this.file_chrome)));
}

// The panel the dock opened is the dock's: off or a drawer, the layout's own Files panel does not show
// beside the drawer (in any order of the choices, in one run), and the dock's return finds it as it was.
#[test]
fn the_panel_the_dock_opened_closes_with_it() {
    in_own_process("dock_panel_follows_choice");
}

#[gpui::test]
fn dock_panel_follows_choice(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_panel_follows_choice") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    let choose = |w: &str| yuzora_core::settings::set_section(crate::yuzora::dock::SHELL, json!({ "dock": { "layouts": { "test": w } } })).unwrap();
    let switch = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.switch", json!({ "name": "test" }), window, cx)));
        cx.run_until_parked();
    };
    let panel_hidden = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.chrome.find("panel").unwrap().hidden);
    let open_git = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
            this.run("layout.show", json!({ "id": "panel" }), window, cx);
        }));
    };
    for order in [vec!["off", "auto", "drawer"], vec!["off", "drawer"], vec!["drawer", "off", "auto"], vec!["drawer", "auto", "off", "drawer"]] {
        choose("auto");
        switch(cx);
        open_git(cx);
        for word in &order {
            choose(word);
            switch(cx);
            let want_hidden = *word != "auto";
            assert_eq!(panel_hidden(cx), want_hidden, "{order:?} at {word}");
        }
        // Back on auto: the panel is on Git as the person left it.
        choose("auto");
        switch(cx);
        assert_eq!(view.update(cx, |this, _| dock_view(&this.chrome)), (Some("yuzora-dock-git".to_string()), false, false), "{order:?}: back");
    }
}

// SF1: a layout.set that makes the dock be rebuilt (not kept as it came) keeps the file's own stack choice and the dock's click.
#[test]
fn a_rebuilt_dock_keeps_the_files_stack_choice() {
    in_own_process("dock_rebuilt_keeps_choice");
}

#[gpui::test]
fn dock_rebuilt_keeps_choice(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_rebuilt_keeps_choice") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let mut layout = panel_layout();
    layout["children"][1]["children"][0]["active"] = json!("changes");
    let (view, cx) = open_window(cx, "unused", "", layout.clone());
    let file_choice = || {
        let n: crate::layout::Node = serde_json::from_str(&layout_file()).unwrap();
        dock_choice(&n)
    };
    fn dock_choice(n: &crate::layout::Node) -> Option<String> {
        match &n.find("panel-body").unwrap().kind {
            crate::layout::Kind::Stack { active, .. } => active.clone(),
            _ => None,
        }
    }
    // (trigger, does the dock's Git click survive: the panel's dock nodes are still there). The row is shown first in a
    // column, the resource chip under it.
    type Edit = fn(&mut serde_json::Value);
    let triggers: [(&str, Edit, bool); 3] = [
        ("the row reversed", |t| t["children"][0]["children"].as_array_mut().unwrap().reverse(), true),
        ("a dock node dropped", |t| t["children"][0]["children"][1]["children"][0]["children"].as_array_mut().unwrap().retain(|c| c["id"] != "yuzora-dock-ssh"), true),
        ("opted into the drawer", |t| t["yuzora"] = json!({ "dock": "drawer" }), false),
    ];
    for (what, edit, click_kept) in triggers {
        std::fs::write(super::layout_path("test"), layout.to_string()).unwrap();
        let _ = std::fs::remove_file(super::state_path("test"));
        view.update(cx, |this, _| this.load_layout("test"));
        cx.update(|window, cx| view.update(cx, |this, cx| {
            this.run("layout.select", json!({ "stack": "panel-body", "child": "yuzora-dock-git" }), window, cx);
            let mut tree = serde_json::to_value(&this.chrome).unwrap();
            edit(&mut tree);
            this.run("layout.set", json!({ "layout": tree }), window, cx);
            this.save();
        }));
        assert_eq!(file_choice().as_deref(), Some("changes"), "{what}: the file keeps its own choice");
        assert!(!layout_file().contains("yuzora-dock"), "{what}");
        let want = Some(if click_kept { "yuzora-dock-git" } else { "changes" }.to_string());
        let shown = view.update(cx, |this, _| dock_choice(&this.chrome));
        assert_eq!(shown, want, "{what}: shown");
        let after = view.update(cx, |this, _| {
            this.load_layout("test");
            dock_choice(&this.chrome)
        });
        assert_eq!(after, want, "{what}: after a restart");
    }
}

// B2: a layout the dock can't place by position (repeated ids), or a layout.set tree without ids, is no crash.
#[test]
fn dock_in_a_layout_with_repeated_ids_is_shown_not_a_crash() {
    in_own_process("dock_repeated_ids");
}

#[gpui::test]
fn dock_repeated_ids(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_repeated_ids") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let layout = json!({ "id": "root", "type": "row", "children": [
        { "id": "r", "type": "row", "children": [{ "id": "a", "type": "block", "content": { "tab": "active" } }] },
        { "id": "c", "type": "column", "children": [{ "id": "r", "type": "row", "children": [
            { "id": "main", "type": "block", "content": { "tab": "active" } },
            { "id": "panel", "type": "column", "children": [{ "id": "panel-body", "type": "stack", "children": [] }] }] }] }] });
    let (view, cx) = open_window(cx, "unused", "", layout);
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some()));
    // layout.set on a personal layout with the tree gone without ids.
    let bare = json!({ "type": "row", "children": [{ "type": "column", "children": [{ "type": "row", "children": [
        { "id": "panel", "type": "column", "children": [{ "id": "panel-body", "type": "stack", "children": [] }] }] }] }] });
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", json!({ "layout": bare.clone() }), window, cx)));
    view.update(cx, |this, _| assert!(this.chrome.find("yuzora-dock-bar").is_some()));
    assert!(!layout_file().contains("yuzora-dock"));
    // And on a shipped layout (win98's side: no ids, nothing to keep).
    view.update(cx, |this, _| this.load_layout("main"));
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", json!({ "layout": bare }), window, cx)));
    view.update(cx, |this, _| assert!(this.chrome.find("main").is_some(), "still shows a layout"));
}

#[test]
fn layout_set_on_a_shipped_layout_accepts_the_dock() {
    in_own_process("dock_layout_set_shipped");
}

/// win98's `setDesktop`: a mod sets the look on a shipped layout with the
/// tree it read from `layout()`, or one built without the dock.
#[gpui::test]
fn dock_layout_set_shipped(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("dock_layout_set_shipped") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    view.update(cx, |this, _| this.load_layout("win98"));
    let set = |tree: serde_json::Value, cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.set", json!({ "layout": tree }), window, cx)));
    };
    let painted = |color: &str, cx: &mut gpui::VisualTestContext| view.update(cx, |this, _| this.chrome.background == Some(crate::layout::Background::Color(color.into())));
    view.update(cx, |this, _| assert!(this.chrome.floating.iter().any(|f| f.id == "yuzora-dock")));
    // As win98's fitLayout does first: the taskbar's height is a click, a status bar's height now.
    cx.update(|window, cx| view.update(cx, |this, cx| this.run("layout.resize", json!({ "id": "taskbar", "px": 28 }), window, cx)));
    let mut with_dock = view.update(cx, |this, _| serde_json::to_value(&this.chrome).unwrap());
    with_dock["background"] = json!("teal");
    set(with_dock, cx);
    assert!(painted("teal", cx), "the tree read from layout() is accepted");
    let mut without = view.update(cx, |this, _| serde_json::to_value(crate::yuzora::dock::strip(&this.chrome, None)).unwrap());
    without["background"] = json!("navy");
    set(without, cx);
    assert!(painted("navy", cx), "a tree built without the dock is accepted too");
    view.update(cx, |this, _| assert!(this.chrome.floating.iter().any(|f| f.id == "yuzora-dock")));
}

#[test]
fn every_shipped_layout_shows_the_resource_chip_in_the_corner() {
    in_own_process("perf_chip_corner");
}

/// The chip's copy loads once in each shipped layout and is drawn at the window's bottom-right, inside the root's padding.
#[gpui::test]
fn perf_chip_corner(cx: &mut gpui::TestAppContext) {
    if !running_alone("perf_chip_corner") {
        return;
    }
    crate::yuzora::dock::force_for_tests();
    let (view, cx) = open_window(cx, "unused", "", panel_layout());
    cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(800.0)));
    for name in ["main", "terminals", "win98", "yuzora"] {
        cx.update(|window, cx| view.update(cx, |this, cx| this.switch_layout(Some(name), window, cx)));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let (width, height) = cx.update(|window, _| (window.viewport_size().width.as_f32(), window.viewport_size().height.as_f32()));
        view.update(cx, |this, _| {
            let chips: Vec<_> = this.copies.iter().filter(|(_, c)| c.inst.name == "yuzora-perf").collect();
            assert_eq!(chips.len(), 1, "{name}: one chip");
            let (slot, copy) = chips[0];
            assert!(copy.root.is_some() && copy.error.is_none(), "{name}: {:?}", copy.error);
            let b = this.bounds.borrow()[slot];
            let (right, bottom) = (f32::from(b.origin.x + b.size.width), f32::from(b.origin.y + b.size.height));
            assert!(width - right <= 6.0 && height - bottom <= 6.0 && f32::from(b.size.height) >= 26.0, "{name}: {b:?} in {width}x{height}");
        });
    }
}

/// The layout files are written in four places only; a new one (an upstream
/// change) fails here until someone decides whether the dock may reach it.
/// Matches the text of the code, not what it does: the tests above are the
/// guard of what it does.
#[test]
fn dock_guard_the_layout_file_is_written_from_known_places_only() {
    let src = include_str!("../window.rs");
    let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
    // keep_ids, layout.set (a person's layout), layout.reset (the shipped text).
    assert_eq!(code.matches("write_atomic(&layout_path(").count(), 3, "a new layout file write: should the dock be stripped from it?");
    // save: the edited tree, stripped.
    assert_eq!(code.matches("write_atomic(&path, &layout)").count(), 1);
    assert!(code.contains("text_of(&crate::yuzora::dock::strip(&edited, Some(&self.file_chrome)))"));
    assert!(code.contains("text_of(&crate::yuzora::dock::strip(&n, Some(&self.file_chrome)))"));
    // Every load injects after keep_ids; every other read of a file is a copy (clone_layout) or only looks (missing_mods).
    assert_eq!(code.matches("crate::yuzora::dock::inject(").count(), 2);
    assert_eq!(code.matches("crate::yuzora::dock::ensure(&mut n, &self.layout_name, Some(&self.file_chrome))").count(), 2);
    // Every write of the clicks (save, and a file edit's) keeps the ones on a dock part that isn't shown; every apply of them skips the dock's panel click when the dock isn't in the panel.
    assert_eq!(code.matches("crate::yuzora::dock::keep_clicks(&mut state, &read_state(&self.layout_name), &").count(), 2);
    assert_eq!(code.matches("crate::yuzora::dock::without_panel_click(&state, &").count(), 2);
    assert_eq!(code.matches("std::fs::read_to_string(layout_path(").count(), 4, "a new read of a layout file: should it inject?");
    // The submodules of window never touch a layout file.
    for (name, sub) in [
        ("floats", include_str!("floats.rs")),
        ("movable", include_str!("movable.rs")),
        ("pane_drag", include_str!("pane_drag.rs")),
        ("pane_size", include_str!("pane_size.rs")),
        ("titlebar", include_str!("titlebar.rs")),
    ] {
        assert!(!sub.contains("layout_path(") && !sub.contains("\"layouts\""), "window/{name}.rs touches a layout file: should the dock be stripped/injected?");
    }
}
