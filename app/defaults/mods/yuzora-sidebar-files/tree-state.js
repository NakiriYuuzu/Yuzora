// What the Files sidebar knows about each folder it has been pointed at, how a
// tree is read, and what is kept for switching back. No imports (a test runs
// this as it is; `export` is dropped) and no host calls: `io` brings them in.
//
// A folder is Empty until its root has been listed, and Showing from then on:
// what was listed before stays on screen while it is read again. `panel` is the
// one place that says which, and the sidebar writes "Loading" for nothing else.

// The shapes. An entry per (machine, folder):
//   { key, on, cwd, spaces: Set<space id>,
//     root,                     // where listing starts: the repo's top as git last said it, else the folder
//     open: Set<folder>, selected, pending: Set<root + folder>,   // relative to `root`; kept when the tree is dropped, reset when `root` moves
//     tree: null | { root, folders: Map<folder, { list, truncated, skipped }> },
//     git:  null | { top, branch, ahead, behind, untrusted, marksCut, failed, marks, dirMarks, count },
//     error, reloading, queued, used, seen: Map<folder, number> }
// A tree carries the root it was listed from: a row's path is that root and the row's folder, and a tree is
// replaced, never re-rooted, so no row is ever read against a root it was not listed from.
// `io`: listDir(root, folder, { on }), readGit(cwd, options) -> { git, failed }, exists(root, rel, on).

/** Folder listings in flight at once. */
export const LISTINGS = 6;
/** Trees kept to switch back to, and the entries and marks they may hold between them. */
export const KEEP_TREES = 8;
export const KEEP_ITEMS = 60000;
/** Folders whose open folders and selection are remembered. */
export const KEEP_FOLDERS = 64;

export function panel(e) {
  if (!e?.cwd) return { kind: "none" };
  if (!e.tree?.folders.has("")) return { kind: "empty", error: e.error };
  return { kind: "showing", tree: e.tree, fresh: e.reloading ? "revalidating" : e.error ? { stale: e.error } : "fresh" };
}

/** What the sidebar notes under the tree. */
export function info(e) {
  const folders = e.tree ? shownFolders(e) : [];
  const git = e.git;
  return {
    limited: folders.some(f => f.truncated), skipped: folders.some(f => f.skipped),
    rootCut: Boolean(e.tree?.folders.get("")?.truncated),
    gitFailed: git?.failed ?? null, marksCut: Boolean(git?.marksCut), untrusted: Boolean(git?.untrusted),
    branch: git?.branch ?? null, ahead: git?.ahead ?? 0, behind: git?.behind ?? 0,
  };
}

// The listings the tree draws: the root's, and those of the open folders that sit under open folders only.
function shownFolders(e) {
  const out = [];
  const under = folder => {
    for (let i = folder.indexOf("/"); i >= 0; i = folder.indexOf("/", i + 1)) if (!e.open.has(folder.slice(0, i))) return false;
    return true;
  };
  for (const folder of ["", ...e.open]) {
    const listing = e.tree.folders.get(folder);
    if (listing && under(folder)) out.push(listing);
  }
  return out;
}

const sameList = (a, b) => a.length === b.length && a.every((x, i) => x.name === b[i].name && x.dir === b[i].dir && x.link === b[i].link && x.hidden === b[i].hidden);
const sameListing = (a, b) => a.truncated === b.truncated && a.skipped === b.skipped && sameList(a.list, b.list);
const sameGit = (a, b) => {
  for (const k of ["top", "branch", "ahead", "behind", "untrusted", "marksCut", "failed"]) if (a[k] !== b[k]) return false;
  const [x, y] = [Object.keys(a.marks), Object.keys(b.marks)];
  return x.length === y.length && x.every(p => a.marks[p] === b.marks[p]);
};

// Each folder above a mark shows a dot while it is closed. Keyed by the folder's path, so the root does not matter.
function dirMarks(marks) {
  const dirs = new Map();
  for (const [path, mark] of Object.entries(marks)) {
    for (let i = path.lastIndexOf("/"); i > 0; i = path.lastIndexOf("/", i - 1)) {
      const dir = path.slice(0, i);
      if (dirs.has(dir)) break;
      dirs.set(dir, mark);
    }
  }
  return dirs;
}

export function createStore(io) {
  const entries = new Map();
  let clock = 0, reads = 0, shown = null;

  const live = e => entries.get(e.key) === e;
  const touch = (e, notify) => { if (shown === e) notify(); };
  // A listing that started before the one already applied for the same folder is older than the screen.
  const older = (e, folder, n) => {
    if ((e.seen.get(folder) ?? 0) > n) return true;
    e.seen.set(folder, n);
    return false;
  };

  function enter(loc) {
    let e = entries.get(loc.key);
    if (!e) {
      e = { key: loc.key, on: loc.on, cwd: loc.cwd, spaces: new Set(), root: null, open: new Set(), pending: new Set(),
        selected: null, tree: null, git: null, error: null, reloading: false, queued: null, used: 0, seen: new Map() };
      entries.set(loc.key, e);
    }
    e.spaces.add(loc.space);
    e.used = ++clock;
    shown = e;
    trim();
    return e;
  }

  // What an entry holds: listed entries, marks, and the folders above marks. The entry on screen is never
  // dropped, whatever its size (its own read would blank it), so the limit bounds what is kept besides it.
  const size = e => (e.git ? e.git.count + e.git.dirMarks.size : 0) + (e.tree ? [...e.tree.folders.values()].reduce((n, f) => n + f.list.length, 0) : 0);

  function trim() {
    let items = 0;
    [...entries.values()].filter(e => e.tree || e.git).sort((a, b) => b.used - a.used).forEach((e, i) => {
      items += size(e);
      if (e !== shown && (i >= KEEP_TREES || items > KEEP_ITEMS)) { e.tree = null; e.git = null; }
    });
    for (const e of [...entries.values()].sort((a, b) => a.used - b.used)) {
      if (entries.size <= KEEP_FOLDERS) break;
      if (e !== shown) entries.delete(e.key);
    }
  }

  /** `spaces`: the ids of the spaces that exist. A folder nothing shows any more is dropped. */
  function forget(spaces) {
    for (const e of [...entries.values()]) {
      for (const id of e.spaces) if (!spaces.has(id)) e.spaces.delete(id);
      if (!e.spaces.size && e !== shown) entries.delete(e.key);
    }
  }

  async function list(e, root, folder, notify) {
    const n = ++reads;
    const got = await io.listDir(root, folder, { on: e.on });
    if (!live(e) || e.root !== root || older(e, folder, n)) return;
    const next = { list: got.entries, truncated: Boolean(got.truncated), skipped: Boolean(got.skipped) };
    const had = e.tree?.folders.get(folder);
    if (had && sameListing(had, next)) return;
    (e.tree ??= { root, folders: new Map() }).folders.set(folder, next);
    touch(e, notify);
  }

  // The root first, then the open folders, a few at a time.
  async function listAll(e, root, folders, notify) {
    let at = 0, rootFailure = null;
    const worker = async () => {
      while (at < folders.length) {
        const folder = folders[at++];
        if (folder && !e.open.has(folder)) continue;
        try { await list(e, root, folder, notify); }
        catch (err) {
          // A root that was left says nothing of the tree that replaced it.
          if (!live(e) || e.root !== root) continue;
          if (!folder) rootFailure = err;
          else { e.open.delete(folder); touch(e, notify); }
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(LISTINGS, folders.length) }, worker));
    if (rootFailure) throw rootFailure;
  }

  // The tree's top is where git last said the repo's is (the folder itself when it is not in one). A read
  // that could not say, a failure, says nothing and leaves the top as it was.
  function moveRoot(e, root, notify) {
    e.root = root; e.open = new Set(); e.selected = null; e.tree = null;
    touch(e, notify);
  }

  async function readGit(e, notify) {
    const got = await io.readGit(e.cwd, { on: e.on });
    if (!live(e)) return;
    const git = got.git, marks = git?.marks ?? {};
    if (!got.failed && (git?.top ?? e.cwd) !== e.root) moveRoot(e, git?.top ?? e.cwd, notify);
    const next = { top: git?.top ?? null, branch: git?.branch ?? null, ahead: git?.ahead ?? 0, behind: git?.behind ?? 0,
      untrusted: Boolean(git?.untrusted), marksCut: Boolean(git?.truncated), failed: got.failed, marks };
    if (e.git && sameGit(e.git, next)) return;
    next.dirMarks = dirMarks(marks);
    next.count = Object.keys(marks).length;
    e.git = next;
    touch(e, notify);
  }

  // One read of the tree: git and every listing at once, each applied when it arrives, and only if it changes
  // what is shown. Whether the root could be listed.
  async function pass(e, notify) {
    const before = e.error;
    // Until git says where the repo is, the folder itself is the root.
    const root = e.root ??= e.cwd;
    const failed = new Map();
    const fail = at => err => { failed.set(at, err); };
    await Promise.all([
      readGit(e, notify).catch(fail(null)),
      listAll(e, root, ["", ...[...e.open].sort()], notify).catch(fail(root)),
    ]);
    if (e.root !== root && live(e)) await listAll(e, e.root, [""], notify).catch(fail(e.root));
    const failure = failed.get(null) ?? failed.get(e.root);
    e.error = failure ? String(failure?.message ?? failure) : null;
    if (e.error !== before) touch(e, notify);
    return !failure;
  }

  /**
   * Reads the tree again from where it is. A read asked for while one runs is not lost: they are one more
   * read when this one ends, and the tree ends up as the last of them found it. null for such a request
   * (the running read answers it); else { ok: whether the root could be listed, timed: whether every request
   * served was the timer's }.
   */
  async function reload(e, notify, { timed = false } = {}) {
    if (!e.cwd || !live(e)) return null;
    if (e.reloading) {
      e.queued = { timed: (e.queued?.timed ?? true) && timed };
      return null;
    }
    e.reloading = true;
    let ok = false, allTimed = timed;
    try {
      do {
        e.queued = null;
        ok = await pass(e, notify);
        if (e.queued) allTimed &&= e.queued.timed;
      } while (e.queued && live(e));
      return { ok, timed: allTimed };
    } finally {
      e.reloading = false;
      e.queued = null;
      trim();
    }
  }

  /**
   * Opens a folder of the tree drawn from `root`: null when it could be listed, else { error, gone } and it
   * stays closed. Nothing happens when the tree has moved off `root` since the row was drawn.
   */
  async function expand(e, root, folder, notify) {
    if (!live(e) || e.root !== root) return null;
    e.open.add(folder);
    touch(e, notify);
    // Listed again on every open: what is kept from before shows meanwhile.
    const key = `${root}\0${folder}`;
    if (e.pending.has(key)) return null;
    e.pending.add(key);
    try {
      await list(e, root, folder, notify);
      return null;
    } catch (error) {
      if (!live(e) || e.root !== root) return null;
      e.open.delete(folder);
      const gone = (await alive(e, root, folder)) === false;
      if (gone) drop(e, root, folder, notify);
      touch(e, notify);
      return { error, gone };
    } finally {
      e.pending.delete(key);
      touch(e, notify);
    }
  }

  // true: it is there; false: it is not; null: the check could not tell, which says nothing about the file.
  const alive = (e, root, rel) => Promise.resolve(io.exists(root, rel, e.on)).then(found => Boolean(found), () => null);

  /** `rel` is gone from its folder in the tree drawn from `root`: it leaves the list, and what was open under it closes. */
  function drop(e, root, rel, notify) {
    if (e.root !== root) return;
    const slash = rel.lastIndexOf("/");
    const folder = e.tree?.folders.get(slash < 0 ? "" : rel.slice(0, slash));
    if (folder) folder.list = folder.list.filter(x => x.name !== rel.slice(slash + 1));
    for (const o of [...e.open]) if (o === rel || o.startsWith(`${rel}/`)) e.open.delete(o);
    touch(e, notify);
  }

  /**
   * Before opening a file the tree may be out of date about: whether it is there (true), is not (false; it is
   * dropped from the tree drawn from `root`), or could not be told (null).
   */
  async function confirm(e, root, rel, notify) {
    const found = await alive(e, root, rel);
    if (found === false && live(e)) drop(e, root, rel, notify);
    return found;
  }

  return { enter, forget, reload, expand, confirm, drop };
}
