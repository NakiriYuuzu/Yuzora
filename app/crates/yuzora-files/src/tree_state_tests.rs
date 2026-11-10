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

// Reads a tree to the end, answering what the host is asked: answers.git, answers[folder].
async function load(store, h, e, answers) {
  let done = false;
  const run = store.reload(e, () => {});
  run.then(() => { done = true; });
  for (let turn = 0; !done; turn++) {
    if (turn > 50) throw new Error("the reload never finished");
    await flush();
    for (const c of h.pending("git")) h.answer(c, answers.git);
    for (const c of h.pending("list")) h.answer(c, answers[c.folder] ?? listing());
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
const opened = store.expand(a, "docs", () => {});
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

const gone = store.confirm(a, "gone.txt", () => {});
await flush();
h.answer(h.pending("exists")[0], false);
const goneResult = await gone;
const afterGone = names(a);

const kept = store.confirm(a, "kept.txt", () => {});
await flush();
h.answer(h.pending("exists")[0], true);
const keptResult = await kept;
const afterKept = names(a);

const folder = store.expand(a, "src", () => {});
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
const opened = store.expand(a, "locked", () => {});
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
const failed = { ran, panel: panel(a).kind, fresh: panel(a).fresh, rows: names(a), notes, open: [...a.open] };
await load(store, h, a, { git: repo("/a"), "": listing("src/", "x.txt"), src: listing("a.js") });
return JSON.stringify({ failed, healed: panel(a).fresh });
"#);
    assert_eq!(
        got["failed"],
        serde_json::json!({
            "ran": false, "panel": "showing", "fresh": { "stale": "offline" },
            "rows": ["src", "x.txt"], "notes": 1, "open": []
        })
    );
    assert_eq!(got["healed"], "fresh");
}

#[test]
fn a_repo_above_the_folder_is_listed_from_its_top_once_git_says_so() {
    let got = run(r#"
const h = host(), store = createStore(h.io);
const a = store.enter(loc("/repo/sub"));
const run = store.reload(a, () => {});
await flush();
const provisional = h.pending("list")[0];
h.answer(h.pending("git")[0], repo("/repo"));
await flush();
h.answer(provisional, listing("wrong.txt"));
await flush();
const roots = h.calls.list.map(c => c.root);
const between = panel(a).kind;
h.answer(h.pending("list")[0], listing("right.txt"));
await run;
return JSON.stringify({ roots, between, root: a.root, rows: names(a), sure: a.rootSure });
"#);
    assert_eq!(got["roots"], serde_json::json!(["/repo/sub", "/repo"]));
    assert_eq!(got["between"], "empty");
    assert_eq!(got["root"], "/repo");
    assert_eq!(got["rows"], serde_json::json!(["right.txt"]));
    assert_eq!(got["sure"], true);
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
for (const c of h.pending("git")) h.answer(c, repo("/a"));
for (const c of h.pending("list")) h.answer(c, listing("x.txt"));
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
