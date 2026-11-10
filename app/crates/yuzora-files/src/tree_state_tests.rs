//! The Files sidebar's tree state (`tree-state.js`), run as it ships, against a
//! host whose every call waits for the test to answer it.

const PRELUDE: &str = r#"
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const file = (name, dir = false) => ({ name, dir, link: false, hidden: false, size: 0 });
const listing = (...names) => ({ entries: names.map(n => file(n.replace(/\/$/, ""), n.endsWith("/"))), truncated: false, skipped: false });
const rel = (root, path) => path === root ? "" : path.startsWith(root + "/") ? path.slice(root.length + 1) : null;
const loc = (cwd, space = "s1", on = null) => ({ space, cwd, on, key: `${on ?? ""}|${cwd}` });
const names = (e, folder = "") => e.tree?.folders.get(folder)?.list.map(x => x.name) ?? null;
const repo = (top, marks = {}, more = {}) => ({ git: { top, branch: "main", detached: false, ahead: 0, behind: 0, marks, truncated: false, untrusted: false, ...more }, failed: null });

// Every call to the host waits here until the test answers it.
function host() {
  const calls = { list: [], git: [], exists: [] };
  const wait = (kind, call) => { const d = deferred(); calls[kind].push({ ...call, ...d, answered: false }); return d.promise; };
  return {
    calls,
    io: {
      listDir: (root, folder, o) => wait("list", { root, folder, on: o.on }),
      readGit: (cwd, o) => wait("git", { cwd, on: o.on }),
      exists: (root, r, on) => wait("exists", { root, r, on }),
      rel,
    },
    pending: kind => calls[kind].filter(c => !c.answered),
    answer(call, value) { call.answered = true; call.resolve(value); },
    fail(call, error) { call.answered = true; call.reject(error); },
  };
}

// Reads a tree to the end, answering what the host is asked: answers.git, answers[folder]
// (a function there is given the call and answers for the root it asks about).
async function load(store, h, e, answers) {
  let done = false;
  const run = store.reload(e, () => {});
  run.then(() => { done = true; });
  for (let turn = 0; !done; turn++) {
    if (turn > 50) throw new Error("the reload never finished");
    await flush();
    for (const c of h.pending("git")) h.answer(c, answers.git);
    for (const c of h.pending("list")) { const a = answers[c.folder]; h.answer(c, typeof a === "function" ? a(c) : a ?? listing()); }
  }
  return run;
}
"#;

fn run(body: &str) -> serde_json::Value {
    let src = include_str!("../../../defaults/mods/yuzora-sidebar-files/tree-state.js")
        .replace("export function ", "function ")
        .replace("export const ", "const ");
    let script = format!("(async () => {{ {src}\n{PRELUDE}\n{body}\n}})()");
    let runtime = quickjs::Runtime::new().unwrap();
    let context = quickjs::Context::full(&runtime).unwrap();
    let out: String = context.with(|ctx| {
        let promise: quickjs::Promise = ctx.eval(script).unwrap();
        promise.finish().unwrap()
    });
    serde_json::from_str(&out).unwrap()
}

#[test]
fn switching_back_to_a_folder_shows_the_tree_it_had_before_anything_is_read() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
await load(store, h, a, { git: repo("/a", { "/a/x.txt": "M" }), "": listing("src/", "x.txt"), src: listing("a.js") });
a.selected = "src/a.js";
const b = store.enter(loc("/b", "s2"));
await load(store, h, b, { git: repo("/b"), "": listing("other.txt") });

const back = store.enter(loc("/a", "s1"));
const first = panel(back);
const rows = { kind: first.kind, root: names(back), src: names(back, "src"), open: [...back.open], selected: back.selected, marks: back.git.marks };
store.reload(back, () => {});
await flush();
const again = panel(back);
return JSON.stringify({ same: back === a, rows, during: [again.kind, again.fresh], asked: [h.pending("git").length, h.pending("list").length] });
"#);
    assert_eq!(got["same"], true);
    assert_eq!(got["rows"]["kind"], "showing");
    assert_eq!(got["rows"]["root"], serde_json::json!(["src", "x.txt"]));
    assert_eq!(got["rows"]["src"], serde_json::json!(["a.js"]));
    assert_eq!(got["rows"]["open"], serde_json::json!(["src"]));
    assert_eq!(got["rows"]["selected"], "src/a.js");
    assert_eq!(got["rows"]["marks"], serde_json::json!({ "/a/x.txt": "M" }));
    assert_eq!(
        got["during"],
        serde_json::json!(["showing", "revalidating"])
    );
    assert_eq!(got["asked"], serde_json::json!([1, 2]));
}

#[test]
fn a_folder_never_read_is_empty_and_only_that_is_loading() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
const before = panel(a);
store.reload(a, () => {});
await flush();
const during = panel(a);
h.answer(h.pending("git")[0], repo("/a"));
await flush();
const git_first = panel(a);
return JSON.stringify({ before, during, git_first, none: panel(store.enter(loc(null, "s1"))).kind });
"#);
    assert_eq!(
        got["before"],
        serde_json::json!({ "kind": "empty", "error": null })
    );
    assert_eq!(
        got["during"],
        serde_json::json!({ "kind": "empty", "error": null })
    );
    assert_eq!(got["git_first"]["kind"], "empty");
    assert_eq!(got["none"], "none");
}

#[test]
fn the_root_is_drawn_when_it_is_listed_whatever_git_is_doing() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
const drawn = [];
const done = store.reload(a, () => drawn.push(panel(a).kind));
await flush();
const asked = { git: h.pending("git").length, list: h.pending("list").map(c => c.folder) };
h.answer(h.pending("list")[0], listing("src/", "x.txt"));
await flush();
const tree = { kind: panel(a).kind, rows: names(a), git: a.git, drawn: [...drawn] };
h.answer(h.pending("git")[0], repo("/a", { "/a/x.txt": "M" }));
await done;
return JSON.stringify({ asked, tree, marks: a.git.marks, branch: info(a).branch });
"#);
    assert_eq!(got["asked"], serde_json::json!({ "git": 1, "list": [""] }));
    assert_eq!(got["tree"]["kind"], "showing");
    assert_eq!(got["tree"]["rows"], serde_json::json!(["src", "x.txt"]));
    assert_eq!(got["tree"]["git"], serde_json::Value::Null);
    assert_eq!(got["tree"]["drawn"], serde_json::json!(["showing"]));
    assert_eq!(got["marks"], serde_json::json!({ "/a/x.txt": "M" }));
    assert_eq!(got["branch"], "main");
}

#[test]
fn open_folders_are_listed_together_but_not_all_at_once() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
for (let i = 1; i <= 8; i++) a.open.add(`d${i}`);
let done = false, most = 0, total = 0, guard = 0;
const run = store.reload(a, () => {});
run.then(() => { done = true; });
while (!done) {
  if (++guard > 100) throw new Error("the reload never finished");
  await flush();
  most = Math.max(most, h.pending("list").length);
  for (const c of h.pending("git")) h.answer(c, repo("/a"));
  const next = h.pending("list")[0];
  if (next) { total++; h.answer(next, listing("f.txt")); }
}
return JSON.stringify({ most, total, listed: [...a.tree.folders.keys()].sort() });
"#);
    assert_eq!(got["most"], 6);
    assert_eq!(got["total"], 9);
    assert_eq!(
        got["listed"],
        serde_json::json!(["", "d1", "d2", "d3", "d4", "d5", "d6", "d7", "d8"])
    );
}

#[test]
fn an_older_listing_never_replaces_a_newer_one() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: repo("/a"), "": listing("docs/") });
// Opening docs asks for its listing; a reload that starts meanwhile asks again.
const opened = store.expand(a, "/a", "docs", () => {});
await flush();
const reload = store.reload(a, () => {});
await flush();
const [fromOpen, fromReload] = h.pending("list").filter(c => c.folder === "docs");
h.answer(h.pending("git")[0], repo("/a"));
h.answer(h.pending("list").find(c => c.folder === ""), listing("docs/"));
h.answer(fromReload, listing("new.md"));
await flush();
h.answer(fromOpen, listing("old.md"));
await Promise.all([opened, reload]);
return JSON.stringify({ docs: names(a, "docs") });
"#);
    assert_eq!(got["docs"], serde_json::json!(["new.md"]));
}

#[test]
fn a_cached_folder_asks_git_again_and_an_untrusted_one_never_shows_marks() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: repo("/a", {}, { untrusted: true }), "": listing("x.txt") });
const untrusted = { marks: a.git.marks, note: info(a).untrusted };
store.enter(loc("/b", "s2"));
const back = store.enter(loc("/a", "s1"));
const shown = { marks: back.git.marks, untrusted: info(back).untrusted };
store.reload(back, () => {});
await flush();
const asked = h.pending("git").length;
// Trusted since: the marks arrive with the answer, not before.
const beforeAnswer = Object.keys(back.git.marks).length;
h.answer(h.pending("git")[0], repo("/a", { "/a/x.txt": "M" }));
h.answer(h.pending("list")[0], listing("x.txt"));
await flush();
const trusted = { marks: back.git.marks, untrusted: info(back).untrusted };
// And untrusted again: what was shown is gone as soon as git says so.
await load(store, h, back, { git: repo("/a", {}, { untrusted: true }), "": listing("x.txt") });
return JSON.stringify({ untrusted, shown, asked, beforeAnswer, trusted, again: { marks: back.git.marks, untrusted: info(back).untrusted } });
"#);
    assert_eq!(
        got["untrusted"],
        serde_json::json!({ "marks": {}, "note": true })
    );
    assert_eq!(
        got["shown"],
        serde_json::json!({ "marks": {}, "untrusted": true })
    );
    assert_eq!(got["asked"], 1);
    assert_eq!(got["beforeAnswer"], 0);
    assert_eq!(
        got["trusted"],
        serde_json::json!({ "marks": { "/a/x.txt": "M" }, "untrusted": false })
    );
    assert_eq!(
        got["again"],
        serde_json::json!({ "marks": {}, "untrusted": true })
    );
}

#[test]
fn a_row_that_is_gone_is_dropped_when_it_is_clicked_and_not_before() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
await load(store, h, a, { git: repo("/a"), "": listing("src/", "gone.txt", "kept.txt"), src: listing("inner.js") });
store.reload(a, () => {});
await flush();
const shown = { kind: panel(a).kind, fresh: panel(a).fresh, rows: names(a) };

const gone = store.confirm(a, "/a", "gone.txt", () => {});
await flush();
h.answer(h.pending("exists")[0], false);
const goneResult = await gone;
const afterGone = names(a);

const kept = store.confirm(a, "/a", "kept.txt", () => {});
await flush();
h.answer(h.pending("exists")[0], true);
const keptResult = await kept;
const afterKept = names(a);

const folder = store.expand(a, "/a", "src", () => {});
await flush();
h.fail(h.pending("list").filter(c => c.folder === "src").pop(), new Error("no such folder"));
await flush();
h.answer(h.pending("exists")[0], false);
const failure = await folder;
return JSON.stringify({ shown, goneResult, afterGone, keptResult, afterKept, folder: { gone: failure.gone, message: failure.error.message }, open: [...a.open], rootAfter: names(a) });
"#);
    assert_eq!(
        got["shown"],
        serde_json::json!({ "kind": "showing", "fresh": "revalidating", "rows": ["src", "gone.txt", "kept.txt"] })
    );
    assert_eq!(got["goneResult"], false);
    assert_eq!(got["afterGone"], serde_json::json!(["src", "kept.txt"]));
    assert_eq!(got["keptResult"], true);
    assert_eq!(got["afterKept"], serde_json::json!(["src", "kept.txt"]));
    assert_eq!(
        got["folder"],
        serde_json::json!({ "gone": true, "message": "no such folder" })
    );
    assert_eq!(got["open"], serde_json::json!([]));
    assert_eq!(got["rootAfter"], serde_json::json!(["kept.txt"]));
}

#[test]
fn a_folder_that_cannot_be_listed_but_is_there_stays_closed_with_its_error() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: repo("/a"), "": listing("locked/") });
const opened = store.expand(a, "/a", "locked", () => {});
await flush();
h.fail(h.pending("list")[0], new Error("permission denied"));
await flush();
h.answer(h.pending("exists")[0], true);
const failure = await opened;
return JSON.stringify({ gone: failure.gone, message: failure.error.message, open: [...a.open], root: names(a) });
"#);
    assert_eq!(got["gone"], false);
    assert_eq!(got["message"], "permission denied");
    assert_eq!(got["open"], serde_json::json!([]));
    assert_eq!(got["root"], serde_json::json!(["locked"]));
}

#[test]
fn what_is_kept_is_bounded_and_a_closed_space_takes_its_folders_with_it() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const first = store.enter(loc("/f0", "s0"));
first.open.add("src");
first.selected = "src/a.js";
await load(store, h, first, { git: repo("/f0"), "": listing("src/"), src: listing("a.js") });
const all = [first];
for (let i = 1; i < 12; i++) {
  const e = store.enter(loc(`/f${i}`, `s${i}`));
  all.push(e);
  await load(store, h, e, { git: repo(`/f${i}`), "": listing("x.txt") });
}
const kept = all.map(e => e.tree ? 1 : 0).join("");
const remembered = { kind: panel(first).kind, open: [...first.open], selected: first.selected };

// A listing too large to keep next to the others.
const big = store.enter(loc("/big", "sbig"));
await load(store, h, big, { git: repo("/big"), "": { entries: Array.from({ length: 70000 }, (_, i) => file(`f${i}`)), truncated: false, skipped: false } });
const other = store.enter(loc("/other", "sother"));
await load(store, h, other, { git: repo("/other"), "": listing("x.txt") });
const bigKept = Boolean(big.tree);

// Spaces close: their folders go, and a read still in flight does not bring one back.
const c = store.enter(loc("/c", "sc"));
store.reload(c, () => {});
await flush();
store.enter(loc("/other", "sother"));
store.forget(new Set(["sother"]));
await flush();
for (const call of h.pending("git")) h.answer(call, repo("/c"));
for (const call of h.pending("list")) h.answer(call, listing("late.txt"));
await flush();
const reborn = store.enter(loc("/c", "sc"));
const closed = store.enter(loc("/f0", "s0"));
return JSON.stringify({ kept, remembered, bigKept, reborn: [reborn === c, reborn.tree === null], closed: [closed === first, closed.selected, [...closed.open]] });
"#);
    // Of twelve folders read, the eight shown most recently keep their trees.
    assert_eq!(got["kept"], "000011111111");
    assert_eq!(
        got["remembered"],
        serde_json::json!({ "kind": "empty", "open": ["src"], "selected": "src/a.js" })
    );
    assert_eq!(got["bigKept"], false);
    assert_eq!(got["reborn"], serde_json::json!([false, true]));
    assert_eq!(got["closed"], serde_json::json!([false, null, []]));
}

#[test]
fn a_reload_that_finds_nothing_new_changes_nothing_on_screen() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
const marks = { "/a/x.txt": "M" };
const answers = () => ({ git: repo("/a", { ...marks }), "": listing("src/", "x.txt"), src: listing("a.js") });
await load(store, h, a, answers());
a.selected = "x.txt";
let notes = 0;
const reload = async (changes = {}) => {
  let done = false;
  const run = store.reload(a, () => { notes++; });
  run.then(() => { done = true; });
  const now = { ...answers(), ...changes };
  for (let turn = 0; !done; turn++) { if (turn > 50) throw new Error("the reload never finished"); await flush(); for (const c of h.pending("git")) h.answer(c, now.git); for (const c of h.pending("list")) h.answer(c, now[c.folder]); }
};
const tree = a.tree, src = a.tree.folders.get("src"), root = a.tree.folders.get("");
// Same names, one file grown: not a change to what is drawn.
const grown = listing("src/", "x.txt"); grown.entries[1].size = 9000;
await reload({ "": grown });
const quiet = { notes, sameTree: a.tree === tree, sameRoot: a.tree.folders.get("") === root };
await reload({ src: listing("a.js", "b.js") });
return JSON.stringify({ quiet, after: { notes, sameRoot: a.tree.folders.get("") === root, src: names(a, "src"), open: [...a.open], selected: a.selected } });
"#);
    assert_eq!(
        got["quiet"],
        serde_json::json!({ "notes": 0, "sameTree": true, "sameRoot": true })
    );
    assert_eq!(
        got["after"],
        serde_json::json!({ "notes": 1, "sameRoot": true, "src": ["a.js", "b.js"], "open": ["src"], "selected": "x.txt" })
    );
}

#[test]
fn a_failed_reload_keeps_the_tree_and_says_why_and_a_good_one_clears_it() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
await load(store, h, a, { git: repo("/a"), "": listing("src/", "x.txt"), src: listing("a.js") });
let notes = 0;
let done = false;
const run = store.reload(a, () => { notes++; });
run.then(() => { done = true; });
await flush();
for (const c of h.pending("git")) h.answer(c, repo("/a"));
for (const c of h.pending("list")) {
  if (c.folder === "") h.fail(c, new Error("offline"));
  else h.fail(c, new Error("no such folder"));
}
await flush();
const ran = await run;
const failed = { ran: ran.ok, panel: panel(a).kind, fresh: panel(a).fresh, rows: names(a), notes, open: [...a.open] };
await load(store, h, a, { git: repo("/a"), "": listing("src/", "x.txt"), src: listing("a.js") });
return JSON.stringify({ failed, healed: panel(a).fresh });
"#);
    assert_eq!(
        got["failed"],
        serde_json::json!({
            "ran": false, "panel": "showing", "fresh": { "stale": "offline" },
            "rows": ["src", "x.txt"], "notes": 2, "open": []
        })
    );
    assert_eq!(got["healed"], "fresh");
}

#[test]
fn a_repo_above_the_folder_is_listed_from_its_top_as_soon_as_git_says_so_and_replaces_the_tree_when_that_is_listed() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const drawn = [];
const run = store.reload(a, () => drawn.push(panel(a).kind));
await flush();
h.answer(h.pending("list")[0], listing("lib/", "README.md"));
await flush();
a.open.add("lib");
a.selected = "lib";
const provisional = h.pending("list").length;
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
const between = { kind: panel(a).kind, root: a.tree.root, rows: names(a), open: [...a.open], selected: a.selected, asked: h.calls.list.map(c => `${c.root}|${c.folder}`), unanswered: h.pending("list").map(c => c.root) };
h.answer(h.pending("list").find(c => c.root === "/repo"), listing("lib/", "sub/"));
await run;
const was = store.listed(a, "/repo/sub/README.md");
const dir = store.listed(a, "/repo/sub/lib");
const never = store.listed(a, "/repo/sub/other.txt");
return JSON.stringify({ provisional, between, after: { root: a.root, treeRoot: a.tree.root, rows: names(a), open: [...a.open], selected: a.selected }, drawn, was, dir, never });
"#);
    assert_eq!(got["provisional"], 0);
    assert_eq!(
        got["between"],
        serde_json::json!({
            "kind": "showing", "root": "/repo/sub", "rows": ["lib", "README.md"], "open": ["lib"], "selected": "lib",
            "asked": ["/repo/sub|", "/repo|"], "unanswered": ["/repo"]
        })
    );
    assert_eq!(
        got["after"],
        serde_json::json!({ "root": "/repo", "treeRoot": "/repo", "rows": ["lib", "sub"], "open": [], "selected": null })
    );
    assert!(!got["drawn"].as_array().unwrap().iter().any(|k| k == "empty"));
    assert_eq!(
        got["was"],
        serde_json::json!({ "root": "/repo/sub", "rel": "README.md", "name": "README.md", "dir": false })
    );
    assert_eq!(got["dir"]["dir"], true);
    assert_eq!(got["never"], serde_json::Value::Null);
}

#[test]
fn the_repos_top_is_listed_without_waiting_for_the_lists_of_the_root_git_leaves() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
a.open.add("a"); a.open.add("b");
const run = store.reload(a, () => {});
await flush();
h.answer(h.pending("list").find(c => c.folder === ""), listing("a/", "b/"));
await flush();
const waiting = h.pending("list").map(c => `${c.root}|${c.folder}`).sort();
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
const asked = h.pending("list").map(c => `${c.root}|${c.folder}`).sort();
for (const c of h.pending("list")) h.answer(c, listing("x.txt"));
await run;
return JSON.stringify({ waiting, asked, root: a.root });
"#);
    assert_eq!(
        got["waiting"],
        serde_json::json!(["/repo/sub|a", "/repo/sub|b"])
    );
    assert_eq!(
        got["asked"],
        serde_json::json!(["/repo/sub|a", "/repo/sub|b", "/repo|"])
    );
    assert_eq!(got["root"], "/repo");
}

#[test]
fn a_folder_git_cannot_read_still_shows_its_files() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: { git: null, failed: "git timed out" }, "": listing("x.txt") });
return JSON.stringify({ kind: panel(a).kind, rows: names(a), note: info(a).gitFailed, root: a.root, marks: a.git.marks });
"#);
    assert_eq!(got["kind"], "showing");
    assert_eq!(got["rows"], serde_json::json!(["x.txt"]));
    assert_eq!(got["note"], "git timed out");
    assert_eq!(got["root"], "/a");
    assert_eq!(got["marks"], serde_json::json!({}));
}

#[test]
fn one_reload_runs_at_a_time_per_folder_and_a_shared_folder_is_one_tree() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a", "s1"));
const first = store.reload(a, () => {});
const second = await store.reload(a, () => {});
await flush();
const asked = [h.pending("git").length, h.pending("list").length];
for (let turn = 0, done = false; !done; turn++) {
  if (turn > 20) throw new Error("never finished");
  await flush();
  for (const c of h.pending("git")) h.answer(c, repo("/a"));
  for (const c of h.pending("list")) h.answer(c, listing("x.txt"));
  done = !a.reloading;
}
await first;
const twin = store.enter(loc("/a", "s2"));
store.forget(new Set(["s2"]));
return JSON.stringify({ second, asked, twin: twin === a, kept: store.enter(loc("/a", "s2")) === a });
"#);
    assert_eq!(got["second"], serde_json::Value::Null);
    assert_eq!(got["asked"], serde_json::json!([1, 1]));
    assert_eq!(got["twin"], true);
    assert_eq!(got["kept"], true);
}

#[test]
fn reads_asked_for_while_one_runs_are_one_more_read_after_it() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: repo("/a"), "": listing("x.txt") });
const base = { git: h.calls.git.length, list: h.calls.list.length };
let done = false;
const first = store.reload(a, () => {});
first.then(() => { done = true; });
await flush();
const merged = await Promise.all([store.reload(a, () => {}), store.reload(a, () => {}), store.reload(a, () => {})]);
h.answer(h.pending("git")[0], repo("/a"));
h.answer(h.pending("list")[0], listing("x.txt"));
await flush();
const second = { git: h.pending("git").length, list: h.pending("list").length, done };
for (const c of h.pending("git")) h.answer(c, repo("/a"));
for (const c of h.pending("list")) h.answer(c, listing("x.txt", "y.txt"));
await first;
await flush();
return JSON.stringify({ merged, second, rows: names(a), asked: { git: h.calls.git.length - base.git, list: h.calls.list.length - base.list }, still: [h.pending("git").length, h.pending("list").length], reloading: a.reloading });
"#);
    assert_eq!(got["merged"], serde_json::json!([null, null, null]));
    assert_eq!(
        got["second"],
        serde_json::json!({ "git": 1, "list": 1, "done": false })
    );
    assert_eq!(got["rows"], serde_json::json!(["x.txt", "y.txt"]));
    assert_eq!(got["asked"], serde_json::json!({ "git": 2, "list": 2 }));
    assert_eq!(got["still"], serde_json::json!([0, 0]));
    assert_eq!(got["reloading"], false);
}

#[test]
fn a_git_that_could_not_answer_leaves_the_root_where_it_is_and_the_next_answer_moves_it() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const tree = c => c.root === "/repo" ? listing("sub/", "top.txt") : listing("inner.txt");
await load(store, h, a, { git: { git: null, failed: "git timed out" }, "": tree });
const failed = { root: a.root, rows: names(a), note: info(a).gitFailed };
await load(store, h, a, { git: repo("/repo"), "": tree });
const answered = { root: a.root, rows: names(a), note: info(a).gitFailed };
await load(store, h, a, { git: { git: null, failed: "git timed out" }, "": tree });
const failedAgain = { root: a.root, rows: names(a) };
return JSON.stringify({ failed, answered, failedAgain });
"#);
    assert_eq!(
        got["failed"],
        serde_json::json!({ "root": "/repo/sub", "rows": ["inner.txt"], "note": "git timed out" })
    );
    assert_eq!(
        got["answered"],
        serde_json::json!({ "root": "/repo", "rows": ["sub", "top.txt"], "note": null })
    );
    assert_eq!(
        got["failedAgain"],
        serde_json::json!({ "root": "/repo", "rows": ["sub", "top.txt"] })
    );
}

#[test]
fn a_root_git_has_named_survives_one_answer_of_not_a_repo_and_goes_on_the_second_in_a_row() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const tree = c => c.root === "/repo" ? listing("lib/", "top.txt") : listing("inner.txt");
const none = { git: null, failed: null };
await load(store, h, a, { git: repo("/repo"), "": tree });
a.open.add("lib");
a.selected = "lib";
await load(store, h, a, { git: none, "": tree });
const once = { root: a.root, open: [...a.open], selected: a.selected, git: a.git.top };
await load(store, h, a, { git: repo("/repo"), "": tree });
await load(store, h, a, { git: none, "": tree });
const separate = { root: a.root, open: [...a.open] };
await load(store, h, a, { git: none, "": tree });
const twice = { root: a.root, rows: names(a), open: [...a.open], selected: a.selected };
await load(store, h, a, { git: none, "": tree });
const still = a.root;
await load(store, h, a, { git: repo("/repo"), "": tree });
return JSON.stringify({ once, separate, twice, still, back: a.root });
"#);
    assert_eq!(
        got["once"],
        serde_json::json!({ "root": "/repo", "open": ["lib"], "selected": "lib", "git": null })
    );
    assert_eq!(
        got["separate"],
        serde_json::json!({ "root": "/repo", "open": ["lib"] })
    );
    assert_eq!(
        got["twice"],
        serde_json::json!({ "root": "/repo/sub", "rows": ["inner.txt"], "open": [], "selected": null })
    );
    assert_eq!(got["still"], "/repo/sub");
    assert_eq!(got["back"], "/repo");
}

#[test]
fn a_folder_that_git_spells_another_way_is_the_same_folder_and_keeps_its_tree() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const out = {};
for (const [name, cwd, top] of [["windows", "C:\\work\\app", "C:/work/app"], ["drive", "c:/work/app/", "C:/work/app"], ["private", "/tmp/x/app", "/private/tmp/x/app"], ["slash", "/srv/app/", "/srv/app"]]) {
  const a = store.enter(loc(cwd, name));
  a.open.add("lib");
  const shown = [];
  const answers = { git: repo(top), "": c => listing("lib/", "x.txt"), lib: listing("y.txt") };
  await load(store, h, a, answers);
  const tree = a.tree;
  const folders = tree.folders;
  await load(store, h, a, { ...answers, "": () => listing("lib/", "x.txt") });
  const run = store.reload(a, n => shown.push(panel(a).kind));
  await flush();
  for (const c of h.pending("git")) h.answer(c, repo(top));
  for (const c of h.pending("list")) h.answer(c, c.folder === "lib" ? listing("y.txt") : listing("lib/", "x.txt"));
  await run;
  out[name] = { root: a.root, treeRoot: a.tree.root, open: [...a.open], sameFolders: a.tree.folders === folders, rows: names(a), next: a.next ?? null, kinds: [...new Set(shown)], was: store.listed(a, `${cwd.replace(/[\\/]+$/, "").replace(/\\/g, "/")}/x.txt`)?.name ?? null };
}
const other = store.enter(loc("/repo/sub", "other"));
await load(store, h, other, { git: repo("/repo"), "": c => c.root === "/repo" ? listing("sub/") : listing("inner.txt") });
out.other = { root: other.root, rows: names(other) };
return JSON.stringify(out);
"#);
    for (name, root) in [
        ("windows", "C:/work/app"),
        ("drive", "C:/work/app"),
        ("private", "/private/tmp/x/app"),
        ("slash", "/srv/app"),
    ] {
        let one = &got[name];
        assert_eq!(one["root"], root, "{name}");
        assert_eq!(one["treeRoot"], root, "{name}");
        assert_eq!(one["open"], serde_json::json!(["lib"]), "{name}");
        assert_eq!(one["sameFolders"], true, "{name}");
        assert_eq!(one["rows"], serde_json::json!(["lib", "x.txt"]), "{name}");
        assert_eq!(one["next"], serde_json::Value::Null, "{name}");
        assert!(!one["kinds"].as_array().unwrap().iter().any(|k| k == "empty"), "{name}");
    }
    assert_eq!(got["other"], serde_json::json!({ "root": "/repo", "rows": ["sub"] }));
}

#[test]
fn a_listing_of_the_folder_git_moved_off_is_not_the_new_trees_error() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
let notes = 0;
const run = store.reload(a, () => { notes++; });
await flush();
const provisional = h.pending("list")[0];
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
h.fail(provisional, new Error("EACCES /repo/sub"));
await flush();
h.answer(h.pending("list")[0], listing("a.txt"));
await run;
return JSON.stringify({ error: a.error, rows: names(a), fresh: panel(a).fresh });
"#);
    assert_eq!(got["error"], serde_json::Value::Null);
    assert_eq!(got["rows"], serde_json::json!(["a.txt"]));
    assert_eq!(got["fresh"], "fresh");
}

#[test]
fn an_open_folder_that_cannot_be_listed_again_closes_and_the_screen_is_told() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
const answers = { git: repo("/a"), "": listing("src/"), src: listing("a.js") };
await load(store, h, a, answers);
let notes = 0;
let done = false;
const run = store.reload(a, () => { notes++; });
run.then(() => { done = true; });
await flush();
for (const c of h.pending("git")) h.answer(c, repo("/a"));
for (const c of h.pending("list")) {
  if (c.folder === "src") h.fail(c, new Error("Permission denied")); else h.answer(c, listing("src/"));
}
await run;
return JSON.stringify({ open: [...a.open], notes, error: a.error });
"#);
    assert_eq!(got["open"], serde_json::json!([]));
    assert_eq!(got["notes"], 1);
    assert_eq!(got["error"], serde_json::Value::Null);
}

#[test]
fn only_the_folders_that_are_open_can_say_a_list_was_cut_short() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("big");
const cut = { ...listing("f1"), truncated: true };
await load(store, h, a, { git: repo("/a"), "": listing("big/", "x.txt"), big: cut });
const open = info(a).limited;
a.open.delete("big");
const closed = info(a).limited;
a.open.add("big");
const reopened = info(a).limited;
const deep = store.enter(loc("/d", "sd"));
deep.open.add("p"); deep.open.add("p/q");
await load(store, h, deep, { git: repo("/d"), "": listing("p/"), p: listing("q/"), "p/q": cut });
const nested = info(deep).limited;
deep.open.delete("p");
return JSON.stringify({ open, closed, reopened, nested, hidden: info(deep).limited });
"#);
    assert_eq!(got["open"], true);
    assert_eq!(got["closed"], false);
    assert_eq!(got["reopened"], true);
    assert_eq!(got["nested"], true);
    assert_eq!(got["hidden"], false);
}

#[test]
fn a_clean_repo_that_becomes_untrusted_is_a_change_to_what_is_drawn() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
await load(store, h, a, { git: repo("/a"), "": listing("x.txt") });
let notes = 0;
let done = false;
const run = store.reload(a, () => { notes++; });
run.then(() => { done = true; });
for (let turn = 0; !done; turn++) { if (turn > 50) throw new Error("never finished"); await flush(); for (const c of h.pending("git")) h.answer(c, repo("/a", {}, { untrusted: true })); for (const c of h.pending("list")) h.answer(c, listing("x.txt")); }
return JSON.stringify({ untrusted: info(a).untrusted, notes });
"#);
    assert_eq!(got["untrusted"], true);
    assert_eq!(got["notes"], 1);
}

#[test]
fn a_tree_on_screen_is_kept_whatever_its_size_and_marked_folders_count_toward_the_limit() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const big = store.enter(loc("/big", "sbig"));
const huge = { entries: Array.from({ length: 70000 }, (_, i) => file(`f${i}`)), truncated: false, skipped: false };
await load(store, h, big, { git: repo("/big"), "": huge });
const shown = { kind: panel(big).kind, tree: Boolean(big.tree) };
await load(store, h, big, { git: repo("/big"), "": huge });
const reloaded = { kind: panel(big).kind, tree: Boolean(big.tree) };

const marked = store.enter(loc("/marked", "smarked"));
const marks = {};
for (let i = 0; i < 20000; i++) marks[`/marked/d${i}/f`] = "M";
await load(store, h, marked, { git: repo("/marked", marks), "": listing("x.txt") });
const crowd = store.enter(loc("/crowd", "scrowd"));
await load(store, h, crowd, { git: repo("/crowd"), "": { entries: Array.from({ length: 25000 }, (_, i) => file(`g${i}`)), truncated: false, skipped: false } });
const latest = store.enter(loc("/latest", "slatest"));
await load(store, h, latest, { git: repo("/latest"), "": listing("x.txt") });
return JSON.stringify({ shown, reloaded, markedKept: Boolean(marked.tree), crowdKept: Boolean(crowd.tree) });
"#);
    assert_eq!(
        got["shown"],
        serde_json::json!({ "kind": "showing", "tree": true })
    );
    assert_eq!(
        got["reloaded"],
        serde_json::json!({ "kind": "showing", "tree": true })
    );
    assert_eq!(got["crowdKept"], true);
    assert_eq!(got["markedKept"], false);
}

#[test]
fn a_check_that_cannot_tell_says_nothing_about_the_file_and_the_row_stays() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
a.open.add("src");
await load(store, h, a, { git: repo("/a"), "": listing("src/", "x.txt"), src: listing("a.js") });
const checked = store.confirm(a, "/a", "x.txt", () => {});
await flush();
h.fail(h.pending("exists")[0], new Error("machine m1 is not connected"));
const told = await checked;
const folder = store.expand(a, "/a", "src", () => {});
await flush();
h.fail(h.pending("list").pop(), new Error("machine m1 is not connected"));
await flush();
h.fail(h.pending("exists")[0], new Error("machine m1 is not connected"));
const failure = await folder;
return JSON.stringify({ told, rows: names(a), folder: failure.gone, open: [...a.open] });
"#);
    assert_eq!(got["told"], serde_json::Value::Null);
    assert_eq!(got["rows"], serde_json::json!(["src", "x.txt"]));
    assert_eq!(got["folder"], false);
    assert_eq!(got["open"], serde_json::json!([]));
}

#[test]
fn a_folder_opened_in_a_root_that_was_left_changes_nothing_in_the_root_that_replaced_it() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const run = store.reload(a, () => {});
await flush();
h.answer(h.pending("list")[0], listing("docs/"));
await flush();
const stale = a.tree;
const opened = store.expand(a, "/repo/sub", "docs", () => {});
await flush();
const oldRead = h.pending("list").find(c => c.root === "/repo/sub" && c.folder === "docs");
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
h.answer(h.pending("list").find(c => c.root === "/repo"), listing("docs/", "sub/"));
await run;
const afterMove = { open: [...a.open], selected: a.selected };
const left = await store.expand(a, "/repo/sub", "x", () => {});
const afterLeft = [...a.open];
const staleAsked = h.calls.list.filter(c => c.root === "/repo/sub" && c.folder === "x").length;
const again = store.expand(a, "/repo", "docs", () => {});
await flush();
h.fail(oldRead, new Error("gone"));
await flush();
const exists = h.pending("exists").length;
const lateOpen = await opened;
const mine = h.pending("list").find(c => c.root === "/repo" && c.folder === "docs");
h.answer(mine, listing("new.md"));
await again;
return JSON.stringify({ afterMove, left, staleAsked, afterLeft, lateOpen, exists, root: a.root, treeRoot: a.tree.root, staleRoot: stale.root, open: [...a.open], docs: names(a, "docs"), top: names(a) });
"#);
    assert_eq!(
        got["afterMove"],
        serde_json::json!({ "open": [], "selected": null })
    );
    assert_eq!(got["left"], serde_json::Value::Null);
    assert_eq!(got["staleAsked"], 0);
    assert_eq!(got["afterLeft"], serde_json::json!([]));
    assert_eq!(got["lateOpen"], serde_json::Value::Null);
    assert_eq!(got["exists"], 0);
    assert_eq!(got["root"], "/repo");
    assert_eq!(got["treeRoot"], "/repo");
    assert_eq!(got["staleRoot"], "/repo/sub");
    assert_eq!(got["open"], serde_json::json!(["docs"]));
    assert_eq!(got["docs"], serde_json::json!(["new.md"]));
    assert_eq!(got["top"], serde_json::json!(["docs", "sub"]));
}

#[test]
fn a_read_that_ends_with_a_request_from_the_timer_only_is_a_timers_read() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/a"));
const finish = async promise => {
  let done = false;
  promise.then(() => { done = true; });
  for (let turn = 0; !done; turn++) { if (turn > 50) throw new Error("never finished"); await flush(); for (const c of h.pending("git")) h.answer(c, repo("/a")); for (const c of h.pending("list")) h.answer(c, listing("x.txt")); }
  return promise;
};
const alone = await finish(store.reload(a, () => {}, { timed: true }));
const manual = await finish(store.reload(a, () => {}));
const first = store.reload(a, () => {}, { timed: true });
await flush();
store.reload(a, () => {}, { timed: true });
store.reload(a, () => {}, { timed: false });
store.reload(a, () => {}, { timed: true });
const merged = await finish(first);
const timers = store.reload(a, () => {}, { timed: true });
await flush();
store.reload(a, () => {}, { timed: true });
const onlyTimers = await finish(timers);
return JSON.stringify({ alone, manual, merged, onlyTimers });
"#);
    assert_eq!(
        got["alone"],
        serde_json::json!({ "ok": true, "timed": true })
    );
    assert_eq!(
        got["manual"],
        serde_json::json!({ "ok": true, "timed": false })
    );
    assert_eq!(
        got["merged"],
        serde_json::json!({ "ok": true, "timed": false })
    );
    assert_eq!(
        got["onlyTimers"],
        serde_json::json!({ "ok": true, "timed": true })
    );
}

#[test]
fn a_failed_listing_of_the_root_git_is_about_to_leave_is_not_the_new_trees_error_whichever_comes_first(
) {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const run = store.reload(a, () => {});
await flush();
h.fail(h.pending("list")[0], new Error("EACCES /repo/sub"));
await flush();
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
h.answer(h.pending("list").find(c => c.root === "/repo"), listing("a.txt"));
const read = await run;
return JSON.stringify({ read, error: a.error, rows: names(a), fresh: panel(a).fresh });
"#);
    assert_eq!(
        got["read"],
        serde_json::json!({ "ok": true, "timed": false })
    );
    assert_eq!(got["error"], serde_json::Value::Null);
    assert_eq!(got["rows"], serde_json::json!(["a.txt"]));
    assert_eq!(got["fresh"], "fresh");
}

#[test]
fn an_open_folder_of_the_root_that_was_left_failing_to_list_does_not_close_the_new_roots_folder() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
a.open.add("docs");
let notes = 0;
const run = store.reload(a, () => { notes++; });
await flush();
const oldDocs = h.pending("list").find(c => c.root === "/repo/sub" && c.folder === "docs");
const oldRoot = h.pending("list").find(c => c.root === "/repo/sub" && c.folder === "");
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
a.open.add("docs");
const before = notes;
h.fail(oldDocs, new Error("gone from the old root"));
await flush();
const during = { open: [...a.open], notes: notes - before };
h.answer(oldRoot, listing("x.txt"));
await flush();
h.answer(h.pending("list").find(c => c.root === "/repo" && c.folder === ""), listing("docs/"));
await run;
return JSON.stringify({ during, open: [...a.open], error: a.error, rows: names(a) });
"#);
    assert_eq!(
        got["during"],
        serde_json::json!({ "open": ["docs"], "notes": 0 })
    );
    assert_eq!(got["open"], serde_json::json!(["docs"]));
    assert_eq!(got["error"], serde_json::Value::Null);
    assert_eq!(got["rows"], serde_json::json!(["docs"]));
}

#[test]
fn a_root_the_tree_was_drawn_from_is_still_the_trees_after_it_moved() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const run = store.reload(a, () => {});
await flush();
h.answer(h.pending("list")[0], listing("x.txt"));
await flush();
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
const during = ["/repo/sub", "/repo", "/elsewhere"].map(r => store.owns(a, r));
h.answer(h.pending("list")[0], listing("sub/"));
await run;
const after = ["/repo/sub", "/repo", "/elsewhere"].map(r => store.owns(a, r));
return JSON.stringify({ during, after });
"#);
    assert_eq!(got["during"], serde_json::json!([true, true, false]));
    assert_eq!(got["after"], serde_json::json!([true, true, false]));
}
