// yuzora(unsaved): included into `window::tests`. On a real window with a real unsaved document: a person's close
// that would leave it with no pane opens the shipped `yuzora-unsaved` dialog instead, the Files sidebar's
// split-then-close does not, a script's close goes ahead as before, and Quit (macOS's menu item and cmd-q) asks.

#[test]
fn a_persons_close_that_would_lose_unsaved_changes_asks_first() {
    in_own_process("unsaved_close_asks");
}

#[gpui::test]
fn unsaved_close_asks(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("unsaved_close_asks") {
        return;
    }
    // The document is read on a thread of its own.
    cx.executor().allow_parking();
    let editor = r#"import { View, div } from "gpui-kit";
import { props } from "herdr";
import { openDocument, documentChanged, Editor } from "yuzora:files";

export default class Probe extends View {
  init(_p, cx) {
    this.doc = props()?.path ? openDocument(props().path) : null;
    if (this.doc) cx.spawn(async (cx) => { let seq = 0; for (;;) { seq = await documentChanged(this.doc, seq); if (!seq) return; cx.notify(); } });
  }
  render() { return div().size_full().child(this.doc ? Editor.new("ed", { doc: this.doc, owner: "probe" }) : div()); }
}
"#;
    let home = crate::mods::app_dir();
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("defaults/mods").join(crate::yuzora::unsaved::MOD);
    for file in ["mod.json", "main.js", "i18n/en.json"] {
        let to = home.join("mods").join(crate::yuzora::unsaved::MOD).join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(shipped.join(file), to).unwrap();
    }
    let file = home.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    let path = file.to_string_lossy().into_owned();
    let layout = json!({ "id": "root", "type": "row", "children": [{ "id": "tabs", "type": "block", "content": { "tab": "active" } }] });
    let (view, cx) = open_window(cx, "yuzora-editor", editor, layout);
    let settle = |cx: &mut gpui::VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    };
    let editor_pane = |id: &str, props: serde_json::Value| crate::layout::Node::block(id, crate::layout::Content::Mod { name: "yuzora-editor".into(), props });
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
        this.model.add_tab("sp1", crate::model::Tab::with_pane("tb2", "a.txt", editor_pane("e1", json!({ "path": path, "on": null }))));
        this.sync(window, cx);
        this.documents.get_mut("").expect("this machine's slot").available = true;
    }));
    let mut state = None;
    for _ in 0..400 {
        settle(cx);
        state = cx.update(|_, cx| yuzora_files::docs::all(cx).first().and_then(|d| d.read(cx).editor().cloned()));
        if state.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let state = state.expect("the editor pane never drew its document");
    cx.update(|window, cx| state.update(cx, |s, cx| gpui::EntityInputHandler::replace_text_in_range(s, None, "mine ", window, cx)));
    settle(cx);
    assert_eq!(cx.update(|_, cx| yuzora_files::docs::unsaved(cx)), 1, "typing made the document unsaved");

    let dialogs = |cx: &mut gpui::VisualTestContext| -> Vec<(String, serde_json::Value)> {
        view.read_with(cx, |this, _| {
            this.overlays.iter().filter_map(|o| this.copies.get(&o.slot)).filter(|c| c.inst.name == crate::yuzora::unsaved::MOD).map(|c| (c.error.clone().unwrap_or_default(), c.inst.props.clone())).collect()
        })
    };
    let panes = |cx: &mut gpui::VisualTestContext| -> Vec<String> { view.read_with(cx, |this, _| this.model.tabs().flat_map(|t| t.root.panes()).map(|n| n.id.clone()).collect()) };
    let close_dialogs = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |this, cx| {
            let ids: Vec<String> = this.overlays.iter().map(|o| o.id.clone()).collect();
            for id in ids {
                this.close_overlay(&id, window, cx);
            }
        }));
        settle(cx);
    };

    // The pane's close button: the last pane holding the document stays, and the dialog asks. Pressed again, or
    // cmd-q, while it is open: no second dialog.
    let reply = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("pane.close", json!({ "pane": "e1" }), true, window, cx)));
    let again = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("app.quit", json!({}), true, window, cx)));
    settle(cx);
    assert_eq!(reply, json!({ "asked": "unsaved" }));
    assert_eq!(again, json!({ "asked": "unsaved" }));
    assert!(panes(cx).contains(&"e1".to_owned()), "the pane is still there");
    let asked = dialogs(cx);
    assert_eq!(asked.len(), 1, "one dialog: {asked:?}");
    let (error, props) = &asked[0];
    assert_eq!(error, "", "the shipped dialog loaded");
    assert_eq!(props["docs"][0]["name"], json!("a.txt"));
    assert_eq!(props["docs"][0]["path"], json!(path));
    assert_eq!(props["command"], json!({ "name": "pane.close", "args": { "pane": "e1" } }));
    close_dialogs(cx);

    // The Files sidebar's `replace`: a pane on the same file beside it, then the old one closes. Nothing is lost.
    cx.update(|window, cx| view.update(cx, |this, cx| {
        this.run("pane.split", json!({ "direction": "right", "pane": "e1", "mod": "yuzora-editor", "props": { "path": path, "on": null, "line": 1 } }), window, cx);
        this.run("pane.close", json!({ "pane": "e1" }), window, cx);
    }));
    settle(cx);
    assert!(!panes(cx).contains(&"e1".to_owned()), "the old pane closed");
    assert_eq!(dialogs(cx), vec![], "the split-then-close asked");
    let new = view.read_with(cx, |this, _| this.model.tab("tb2").unwrap().root.panes()[0].id.clone());

    // A script's close (`herdr-app do`, an agent) closes as before; the document stays in memory, unsaved.
    let reply = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("pane.close", json!({ "pane": new }), false, window, cx)));
    settle(cx);
    assert_eq!(reply, json!({ "ok": true }));
    assert!(!panes(cx).contains(&new), "the script's close closed it");
    assert_eq!(dialogs(cx), vec![]);
    assert_eq!(cx.update(|_, cx| yuzora_files::docs::unsaved(cx)), 1);

    // Quit from the menu bar or cmd-q is an action, not a command: it reaches the window as a person's app.quit,
    // which asks about the document no pane holds any more.
    cx.update(|_, cx| crate::macos::init(cx));
    cx.dispatch_action(crate::macos::Quit);
    settle(cx);
    let asked = dialogs(cx);
    assert_eq!(asked.len(), 1, "Quit asked: {asked:?}");
    assert_eq!(asked[0].1["command"], json!({ "name": "app.quit", "args": {} }));
    assert_eq!(asked[0].1["docs"][0]["name"], json!("a.txt"));
    close_dialogs(cx);

    // With no update downloaded, app.update fails as it always did and loses nothing: no dialog.
    let reply = cx.update(|window, cx| view.update(cx, |this, cx| this.run_as("app.update", json!({}), true, window, cx)));
    settle(cx);
    assert!(reply.get("error").is_some(), "{reply}");
    assert_eq!(dialogs(cx), vec![]);

    // The window's own close button, where it closes (macOS hides the app instead, which the test platform can't).
    if !cfg!(target_os = "macos") {
        let closed = cx.simulate_close();
        settle(cx);
        let asked = dialogs(cx);
        assert!(!closed, "the window closed");
        assert_eq!(asked.len(), 1, "closing the window asked: {asked:?}");
        assert_eq!(asked[0].1["command"], json!({ "name": "window.close", "args": {} }));
    }
}
