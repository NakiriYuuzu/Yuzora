//! Asking before unsaved changes are lost. A person's close of a pane, tab or
//! Space that would leave an unsaved document with no pane, and closing the
//! window, quitting or restarting into an update, open the `yuzora-unsaved`
//! dialog (Save, Don't Save, Cancel) instead of running. Its answer runs the
//! command again, which then finds nothing unsaved. Scripts and agents
//! (`person: false`) and layout changes sent by herdr-core are not asked: the
//! documents stay in memory and the next quit asks about them.

use crate::layout::{Content, Node};
use crate::model::Model;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// The dialog.
pub const MOD: &str = "yuzora-unsaved";

/// Panes that open a document for the file their props name. Git diff panes
/// also carry a `path` and hold no document.
const DOCUMENT_MODS: [&str; 2] = ["yuzora-editor", "yuzora-markdown"];

/// An open document, as far as closing goes.
#[derive(Clone, Debug)]
pub struct Doc {
    pub id: u64,
    pub on: Option<String>,
    pub path: String,
    /// The pane it claims (`docs::for_pane`): it follows a rename that the
    /// pane's props can't.
    pub pane: Option<String>,
    pub dirty: bool,
}

/// The unsaved documents a command would lose, in the order they were opened.
#[derive(Debug)]
pub struct Ask<'a> {
    pub docs: Vec<&'a Doc>,
}

/// What `command` would lose. Closing a pane, tab or Space loses a document
/// one of its panes holds and no other pane does; quitting, closing the window
/// and restarting into an update lose every unsaved document.
pub fn asks<'a>(command: &str, args: &Value, model: &Model, docs: &'a [Doc]) -> Option<Ask<'a>> {
    if !docs.iter().any(|d| d.dirty) {
        return None;
    }
    let mut lost: Vec<&Doc> = match command {
        "app.quit" | "window.close" | "app.update" => docs.iter().filter(|d| d.dirty).collect(),
        "pane.close" | "tab.close" | "space.close" => {
            let closing: HashSet<&str> = closing(command, args, model)?.into_iter().map(|n| n.id.as_str()).collect();
            let shown = Shown::new(docs);
            let (mut gone, mut kept) = (HashSet::new(), HashSet::new());
            for pane in model.spaces.iter().flat_map(|s| &s.tabs).flat_map(|t| t.root.panes()) {
                if let Some(doc) = shown.doc_of(pane) {
                    if closing.contains(pane.id.as_str()) { gone.insert(doc) } else { kept.insert(doc) };
                }
            }
            docs.iter().filter(|d| d.dirty && gone.contains(&d.id) && !kept.contains(&d.id)).collect()
        }
        _ => return None,
    };
    lost.sort_by_key(|d| d.id);
    (!lost.is_empty()).then_some(Ask { docs: lost })
}

/// The panes `command` closes now, as `dispatch` resolves it. None where it
/// closes nothing, or where it first asks "Close space?": that dialog's
/// Confirm runs `space.close` again with `confirmed`, and is asked then.
fn closing<'m>(command: &str, args: &Value, model: &'m Model) -> Option<Vec<&'m Node>> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_owned);
    let mut confirmed = args.get("confirmed").and_then(Value::as_bool) == Some(true);
    let tab = match command {
        "space.close" => {
            let space = model.space(&arg("id").or_else(|| model.active.clone())?)?;
            return confirmed.then(|| space.tabs.iter().flat_map(|t| t.root.panes()).collect());
        }
        "pane.close" => {
            let pane = arg("pane").or_else(|| model.focused_pane())?;
            let tab = model.tab_of(&pane)?;
            let node = tab.root.find(&pane).filter(|n| n.closable())?;
            if tab.root.panes().len() > 1 {
                return Some(vec![node]);
            }
            // Its tab goes with it, and that `tab.close` carries no `confirmed`.
            confirmed = false;
            tab.id.clone()
        }
        _ => arg("id").or_else(|| model.active_tab().map(|t| t.id.clone()))?,
    };
    let space = model.space_of(&tab)?;
    if space.tabs.len() == 1 {
        return confirmed.then(|| space.tabs.iter().flat_map(|t| t.root.panes()).collect());
    }
    Some(model.tab(&tab)?.root.panes())
}

/// Which document each pane holds: the one that claims it, else the one its
/// props name, as the Files sidebar finds a file's pane.
struct Shown<'a> {
    claimed: HashMap<&'a str, u64>,
    by_file: HashMap<(Option<&'a str>, &'a str), u64>,
}

impl<'a> Shown<'a> {
    fn new(docs: &'a [Doc]) -> Self {
        let claimed = docs.iter().filter_map(|d| Some((d.pane.as_deref()?, d.id))).collect();
        let by_file = docs.iter().map(|d| ((d.on.as_deref(), d.path.as_str()), d.id)).collect();
        Shown { claimed, by_file }
    }

    fn doc_of(&self, pane: &Node) -> Option<u64> {
        if let Some(&doc) = self.claimed.get(pane.id.as_str()) {
            return Some(doc);
        }
        let Some(Content::Mod { name, props }) = pane.content() else { return None };
        if !DOCUMENT_MODS.contains(&name.as_str()) {
            return None;
        }
        let on = props.get("on").and_then(Value::as_str).filter(|m| !m.is_empty());
        self.by_file.get(&(on, props.get("path")?.as_str()?)).copied()
    }
}

/// The open documents now.
fn open_docs(cx: &gpui::App) -> Vec<Doc> {
    yuzora_files::docs::all(cx)
        .into_iter()
        .map(|view| {
            let d = view.read(cx);
            Doc { id: d.id, on: d.key.on.clone(), path: d.key.path.clone(), pane: d.pane().map(str::to_owned), dirty: d.dirty() }
        })
        .collect()
}

/// The dialog a person's `command` opens instead of running, if it would
/// lose unsaved changes. `app.update` with no update ready (`update_ready`,
/// asked only for it) fails by itself, losing nothing, so it is not asked.
pub fn dialog(command: &str, args: &Value, model: &Model, update_ready: impl FnOnce() -> bool, cx: &gpui::App) -> Option<Value> {
    match command {
        "pane.close" | "tab.close" | "space.close" | "app.quit" | "window.close" => {}
        "app.update" if update_ready() => {}
        _ => return None,
    }
    let docs = open_docs(cx);
    let ask = asks(command, args, model, &docs)?;
    let listed: Vec<Value> = ask
        .docs
        .iter()
        .map(|d| {
            let name = d.path.rsplit(['/', '\\']).next().unwrap_or(&d.path);
            json!({ "doc": d.id, "name": name, "path": d.path, "on": d.on })
        })
        .collect();
    Some(json!({ "mod": MOD, "kind": "dialog", "width": 440, "props": { "docs": listed, "command": { "name": command, "args": pinned(command, args, model) } } }))
}

/// The command as the dialog runs it again: a close with no target (a key)
/// names the pane, tab or Space it was going to close, so a tab shown while
/// the dialog is open (ctrl-tab still switches) is not the one closed.
fn pinned(command: &str, args: &Value, model: &Model) -> Value {
    let (key, target) = match command {
        "pane.close" => ("pane", model.focused_pane()),
        "tab.close" => ("id", model.active_tab().map(|t| t.id.clone())),
        "space.close" => ("id", model.active.clone()),
        _ => return args.clone(),
    };
    let mut pinned = if args.is_object() { args.clone() } else { json!({}) };
    if pinned.get(key).and_then(Value::as_str).is_none()
        && let Some(target) = target
    {
        pinned[key] = json!(target);
    }
    pinned
}

/// The Quit menu item and its key (macOS) are an action, not a command, so
/// they never pass `run_as`. With something unsaved the window gets a
/// person's `app.quit`, which asks. Otherwise `now` quits. That is also what
/// happens with no window to ask in (after an agent's `window.close`), and
/// the changes are lost.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn quit(cx: &mut gpui::App, now: impl FnOnce(&mut gpui::App)) {
    let window = cx.windows().iter().any(|w| w.downcast::<crate::window::AppWindow>().is_some());
    if window && yuzora_files::docs::unsaved(cx) > 0 {
        crate::mods::request(crate::mods::Request::Run { name: "app.quit".into(), args: json!({}) });
    } else {
        now(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Axis;
    use crate::model::Tab;

    fn editor(id: &str, path: &str) -> Node {
        Node::block(id, Content::Mod { name: "yuzora-editor".into(), props: json!({ "path": path, "on": null, "opened": 1 }) })
    }

    fn pane(id: &str, name: &str, props: Value) -> Node {
        Node::block(id, Content::Mod { name: name.into(), props })
    }

    fn terminal(id: &str) -> Node {
        Node::block(id, Content::Terminal { terminal: format!("local/{id}") })
    }

    fn tab(id: &str, panes: Vec<Node>) -> Tab {
        let mut t = Tab::with_pane(id, id, panes[0].clone());
        t.root = Node::container(id, Axis::Row, panes);
        t
    }

    fn doc(id: u64, path: &str, dirty: bool) -> Doc {
        Doc { id, on: None, path: path.into(), pane: None, dirty }
    }

    /// Space `s1`: tab `shell` (a terminal), tab `files` (`e1` on a.txt, `e2` on b.md, a terminal).
    /// Space `s2`: tab `other` (`e3` on c.txt).
    fn world() -> Model {
        let mut m = Model::default();
        m.add_space("s1", "", "/w", tab("shell", vec![terminal("p1")]));
        m.add_tab("s1", tab("files", vec![editor("e1", "/w/a.txt"), pane("e2", "yuzora-markdown", json!({ "path": "/w/b.md" })), terminal("p2")]));
        m.add_space_unshown("s2", "", "/v", tab("other", vec![editor("e3", "/v/c.txt")]));
        m
    }

    fn asked(command: &str, args: Value, model: &Model, docs: &[Doc]) -> Option<Vec<u64>> {
        asks(command, &args, model, docs).map(|a| a.docs.iter().map(|d| d.id).collect())
    }

    const EVERY: [(&str, &str); 6] = [
        ("pane.close", r#"{"pane":"e1"}"#),
        ("tab.close", r#"{"id":"files"}"#),
        ("space.close", r#"{"id":"s1","confirmed":true}"#),
        ("app.quit", "{}"),
        ("window.close", "{}"),
        ("app.update", "{}"),
    ];

    #[test]
    fn nothing_unsaved_asks_nothing() {
        let m = world();
        let clean = [doc(1, "/w/a.txt", false), doc(2, "/w/b.md", false)];
        for (command, args) in EVERY {
            let args: Value = serde_json::from_str(args).unwrap();
            assert_eq!(asked(command, args.clone(), &m, &clean), None, "{command}");
            assert_eq!(asked(command, args, &m, &[]), None, "{command} with no document open");
        }
    }

    #[test]
    fn every_close_and_quit_asks_about_the_one_unsaved_document_it_loses() {
        let m = world();
        let docs = [doc(1, "/w/a.txt", true), doc(2, "/w/b.md", false)];
        for (command, args) in EVERY {
            assert_eq!(asked(command, serde_json::from_str(args).unwrap(), &m, &docs), Some(vec![1]), "{command}");
        }
    }

    #[test]
    fn closing_what_holds_no_unsaved_document_asks_nothing() {
        let m = world();
        let docs = [doc(1, "/w/a.txt", true)];
        assert_eq!(asked("pane.close", json!({ "pane": "e2" }), &m, &docs), None);
        assert_eq!(asked("pane.close", json!({ "pane": "p2" }), &m, &docs), None);
        assert_eq!(asked("tab.close", json!({ "id": "shell" }), &m, &docs), None);
        assert_eq!(asked("space.close", json!({ "id": "s2", "confirmed": true }), &m, &docs), None);
        assert_eq!(asked("layout.switch", json!({ "name": "main" }), &m, &docs), None);
    }

    #[test]
    fn a_tab_asks_about_every_unsaved_document_in_it() {
        let m = world();
        let docs = [doc(2, "/w/b.md", true), doc(1, "/w/a.txt", true), doc(3, "/v/c.txt", true)];
        assert_eq!(asked("tab.close", json!({ "id": "files" }), &m, &docs), Some(vec![1, 2]));
        // No id: the tab shown.
        assert_eq!(asked("tab.close", json!({}), &m, &docs), Some(vec![1, 2]));
        assert_eq!(asked("space.close", json!({ "confirmed": true }), &m, &docs), Some(vec![1, 2]));
    }

    #[test]
    fn a_pane_with_no_id_is_the_focused_one() {
        let mut m = world();
        let docs = [doc(1, "/w/a.txt", true)];
        m.tab_mut("files").unwrap().focus("e1");
        assert_eq!(asked("pane.close", json!({}), &m, &docs), Some(vec![1]));
        m.tab_mut("files").unwrap().focus("p2");
        assert_eq!(asked("pane.close", json!({}), &m, &docs), None);
    }

    #[test]
    fn of_two_panes_on_one_document_only_the_last_to_close_asks() {
        let mut m = world();
        m.add_tab("s2", tab("side", vec![editor("e4", "/w/a.txt")]));
        let docs = [doc(1, "/w/a.txt", true)];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), None);
        assert_eq!(asked("tab.close", json!({ "id": "side" }), &m, &docs), None);
        m.tab_mut("files").unwrap().root = Node::container("files", Axis::Row, vec![pane("e2", "yuzora-markdown", json!({ "path": "/w/b.md" })), terminal("p2")]);
        assert_eq!(asked("tab.close", json!({ "id": "side" }), &m, &docs), Some(vec![1]));
    }

    #[test]
    fn a_pane_that_has_not_started_holds_the_file_its_props_name() {
        // After a restart a tab not shown yet has no copy and opened nothing: it opens the file when shown.
        let mut m = world();
        m.add_tab("s2", tab("later", vec![editor("e4", "/w/a.txt")]));
        let docs = [Doc { pane: Some("e1".into()), ..doc(1, "/w/a.txt", true) }];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), None);
    }

    #[test]
    fn a_renamed_document_is_held_by_the_pane_it_claims() {
        // The sidebar renamed a.txt to z.txt: the document followed, the pane's props did not.
        let m = world();
        let docs = [Doc { pane: Some("e1".into()), ..doc(1, "/w/z.txt", true) }];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), Some(vec![1]));
        // A pane claimed by one document does not also hold the file its props name.
        let docs = [Doc { pane: Some("e1".into()), ..doc(1, "/w/z.txt", false) }, doc(2, "/w/a.txt", true)];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), None);
    }

    #[test]
    fn the_sidebars_split_then_close_never_asks() {
        // `replace` in yuzora-sidebar-files/open.js: a new pane beside the old one, then the old one closes.
        let mut m = world();
        let docs = [doc(1, "/w/a.txt", true)];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), Some(vec![1]));
        files_root(&mut m, vec![editor("e1", "/w/a.txt"), editor("e1b", "/w/a.txt"), terminal("p2")]);
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), None);
        // After a rename the new pane names the new path, where the document now is.
        let renamed = [doc(1, "/w/z.txt", true)];
        files_root(&mut m, vec![editor("e1", "/w/a.txt"), editor("e1b", "/w/z.txt")]);
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &renamed), None);
        assert_eq!(asked("pane.close", json!({ "pane": "e1b" }), &m, &renamed), Some(vec![1]));
    }

    fn files_root(m: &mut Model, panes: Vec<Node>) {
        m.tab_mut("files").unwrap().root = Node::container("files", Axis::Row, panes);
    }

    #[test]
    fn a_document_no_pane_holds_is_asked_about_only_on_quitting() {
        let m = world();
        let docs = [doc(1, "/elsewhere/kept.txt", true)];
        for (command, args) in EVERY {
            let quits = matches!(command, "app.quit" | "window.close" | "app.update");
            let expected = quits.then(|| vec![1]);
            assert_eq!(asked(command, serde_json::from_str(args).unwrap(), &m, &docs), expected, "{command}");
        }
    }

    #[test]
    fn a_space_asks_only_once_its_close_is_confirmed() {
        // Unconfirmed, a person first gets "Close space?"; its Confirm runs space.close with confirmed.
        let mut m = world();
        let docs = [doc(1, "/w/a.txt", true), doc(3, "/v/c.txt", true)];
        assert_eq!(asked("space.close", json!({ "id": "s1" }), &m, &docs), None);
        assert_eq!(asked("space.close", json!({ "id": "s1", "confirmed": true }), &m, &docs), Some(vec![1]));
        // The last tab and the last pane of a space close the space: the same.
        assert_eq!(asked("tab.close", json!({ "id": "other" }), &m, &docs), None);
        assert_eq!(asked("tab.close", json!({ "id": "other", "confirmed": true }), &m, &docs), Some(vec![3]));
        assert_eq!(asked("pane.close", json!({ "pane": "e3" }), &m, &docs), None);
        // A tab's last pane in a space with other tabs closes just that tab.
        files_root(&mut m, vec![editor("e1", "/w/a.txt")]);
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &docs), Some(vec![1]));
    }

    #[test]
    fn a_pane_that_cannot_close_asks_nothing() {
        let mut m = world();
        let mut fixed = editor("e1", "/w/a.txt");
        fixed.closable = Some(false);
        files_root(&mut m, vec![fixed, terminal("p2")]);
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &[doc(1, "/w/a.txt", true)]), None);
    }

    #[test]
    fn only_editor_and_markdown_panes_hold_the_file_their_props_name() {
        let mut m = world();
        files_root(&mut m, vec![pane("d1", "yuzora-git-diff", json!({ "path": "/w/a.txt" })), terminal("p2")]);
        assert_eq!(asked("pane.close", json!({ "pane": "d1" }), &m, &[doc(1, "/w/a.txt", true)]), None);
    }

    #[test]
    fn a_file_on_a_machine_is_not_the_same_file_here() {
        let m = world();
        let there = [Doc { on: Some("lab".into()), ..doc(1, "/w/a.txt", true) }];
        assert_eq!(asked("pane.close", json!({ "pane": "e1" }), &m, &there), None);
        let mut m = world();
        files_root(&mut m, vec![pane("r1", "yuzora-editor", json!({ "path": "/w/a.txt", "on": "lab" })), terminal("p2")]);
        assert_eq!(asked("pane.close", json!({ "pane": "r1" }), &m, &there), Some(vec![1]));
    }

    #[test]
    fn a_close_with_no_target_is_run_again_on_the_one_it_was_going_to_close() {
        let mut m = world();
        m.tab_mut("files").unwrap().focus("e1");
        assert_eq!(pinned("pane.close", &Value::Null, &m), json!({ "pane": "e1" }));
        assert_eq!(pinned("tab.close", &json!({}), &m), json!({ "id": "files" }));
        assert_eq!(pinned("space.close", &json!({ "confirmed": true }), &m), json!({ "id": "s1", "confirmed": true }));
        assert_eq!(pinned("pane.close", &json!({ "pane": "e2" }), &m), json!({ "pane": "e2" }), "a target given is kept");
        assert_eq!(pinned("app.quit", &Value::Null, &m), Value::Null);
    }

    /// `asks` with 200 documents open, half of them unsaved, over 200 panes.
    /// Budget: a median under 1 ms.
    #[test]
    #[ignore]
    fn perf_asks_200() {
        let mut m = Model::default();
        let docs: Vec<Doc> = (0..200u64).map(|i| Doc { pane: (i % 3 == 0).then(|| format!("e{i}")), ..doc(i, &format!("/w/{i}.txt"), i % 2 == 0) }).collect();
        m.add_space("s1", "", "/w", tab("t0", vec![terminal("p0")]));
        for t in 0..20 {
            let panes = (0..10).map(|j| editor(&format!("e{}", t * 10 + j), &format!("/w/{}.txt", t * 10 + j))).collect();
            m.add_tab("s1", tab(&format!("t{}", t + 1), panes));
        }
        let mut times: Vec<std::time::Duration> = (0..10)
            .map(|_| {
                let at = std::time::Instant::now();
                for command in ["pane.close", "tab.close", "app.quit"] {
                    let args = json!({ "pane": "e105", "id": "t11" });
                    std::hint::black_box(asks(command, &args, &m, &docs));
                }
                at.elapsed() / 3
            })
            .collect();
        times.sort();
        let median = times[5];
        eprintln!("perf_asks_200: median {median:?} per call, runs {times:?}");
        assert!(median < std::time::Duration::from_millis(1), "asks took {median:?}");
    }

    /// All of `dialog` (the open documents read from the registry, then
    /// `asks`) for a close or quit with 200 documents open and none unsaved,
    /// as every close and quit is most of the time. Budget: a median under 1 ms.
    #[gpui::test]
    #[ignore]
    fn perf_dialog_200_nothing_unsaved(cx: &mut gpui::TestAppContext) {
        use std::time::{Duration, Instant};
        // The documents are read on threads of their own.
        cx.executor().allow_parking();
        let dir = crate::test_tmp().join(format!("herdr-unsaved-perf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |i: usize| dir.join(format!("{i}.txt")).to_string_lossy().into_owned();
        let mut m = Model::default();
        m.add_space("s1", "", "/w", tab("t0", vec![terminal("p0")]));
        for t in 0..20 {
            m.add_tab("s1", tab(&format!("t{}", t + 1), (0..10).map(|j| editor(&format!("e{}", t * 10 + j), &path(t * 10 + j))).collect()));
        }
        for i in 0..200 {
            std::fs::write(path(i), "x\n").unwrap();
            cx.update(|cx| yuzora_files::docs::open(yuzora_files::docs::DocKey { on: None, path: path(i) }, cx));
        }
        let loaded = |cx: &mut gpui::TestAppContext| cx.update(|cx| yuzora_files::docs::all(cx).iter().filter(|d| d.read(cx).text(cx).as_ref() == "x\n").count());
        for _ in 0..500 {
            cx.run_until_parked();
            if loaded(cx) == 200 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(loaded(cx), 200, "the documents never loaded");
        let args = json!({ "pane": "e105", "id": "t11" });
        let commands = ["pane.close", "tab.close", "app.quit"];
        let per_call = |run: &dyn Fn(&str, &gpui::App), cx: &mut gpui::TestAppContext| -> Vec<Duration> {
            let mut times: Vec<Duration> = (0..10)
                .map(|_| {
                    cx.update(|cx| {
                        let at = Instant::now();
                        for _ in 0..10 {
                            for command in commands {
                                run(command, cx);
                            }
                        }
                        at.elapsed() / 30
                    })
                })
                .collect();
            times.sort();
            times
        };
        let whole = per_call(&|c, cx| assert!(std::hint::black_box(dialog(c, &args, &m, || false, cx)).is_none()), cx);
        let read = per_call(&|_, cx| drop(std::hint::black_box(open_docs(cx))), cx);
        let snapshot = cx.update(|cx| open_docs(cx));
        let decide = per_call(&|c, _| drop(std::hint::black_box(asks(c, &args, &m, &snapshot))), cx);
        eprintln!("perf_dialog_200_nothing_unsaved: dialog median {:?} (runs {whole:?}); open_docs median {:?} (runs {read:?}); asks median {:?} (runs {decide:?})", whole[5], read[5], decide[5]);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(whole[5] < Duration::from_millis(1), "dialog took {:?}", whole[5]);
    }
}
