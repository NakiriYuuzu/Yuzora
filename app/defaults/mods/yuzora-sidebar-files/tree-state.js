// What the Files sidebar knows about each folder it has been pointed at, how a
// tree is read, and what is kept for switching back. No imports (a test runs
// this as it is; `export` is dropped) and no host calls: `io` brings them in.
//
// A folder is Empty until its root has been listed, and Showing from then on:
// what was listed before stays on screen while it is read again. `panel` is the
// one place that says which, and the sidebar writes "Loading" for nothing else.

// The shapes. An entry per (machine, folder):
//   { key, on, cwd, spaces: Set<space id>,
//     root, rootSure,           // the tree's top; sure once git has said where the repo's is
//     open: Set<folder>, selected, pending: Set<folder>,   // remembered even when the tree is dropped
//     tree: null | { folders: Map<folder, { list, truncated, skipped }> },
//     git:  null | { top, branch, ahead, behind, untrusted, marksCut, failed, marks, dirMarks, count },
//     error, reloading, used, seen: Map<piece, number> }
// `io`: listDir(root, folder, { on }), readGit(cwd, options) -> { git, failed }, exists(root, rel, on), rel(root, path).

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
  const folders = e.tree ? [...e.tree.folders.values()] : [];
  const git = e.git;
  return {
    limited: folders.some(f => f.truncated), skipped: folders.some(f => f.skipped),
    rootCut: Boolean(e.tree?.folders.get("")?.truncated),
    gitFailed: git?.failed ?? null, marksCut: Boolean(git?.marksCut), untrusted: Boolean(git?.untrusted),
    branch: git?.branch ?? null, ahead: git?.ahead ?? 0, behind: git?.behind ?? 0,
  };
}

const sameList = (a, b) => a.length === b.length && a.every((x, i) => x.name === b[i].name && x.dir === b[i].dir && x.link === b[i].link && x.hidden === b[i].hidden);
const sameListing = (a, b) => a.truncated === b.truncated && a.skipped === b.skipped && sameList(a.list, b.list);
const sameGit = (a, b) => {
  for (const k of ["top", "branch", "ahead", "behind", "untrusted", "marksCut", "failed"]) if (a[k] !== b[k]) return false;
  const [x, y] = [Object.keys(a.marks), Object.keys(b.marks)];
  return x.length === y.length && x.every(p => a.marks[p] === b.marks[p]);
};

// Each folder above a mark shows a dot while it is closed.
function dirMarks(root, marks, rel) {
  const dirs = new Map();
  for (const [path, mark] of Object.entries(marks)) {
    let r = rel(root, path);
    if (r === null) continue;
    while (r.includes("/")) {
      r = r.slice(0, r.lastIndexOf("/"));
      if (!dirs.has(r)) dirs.set(r, mark);
    }
  }
  return dirs;
}

export function createStore(io) {
  const entries = new Map();
  let clock = 0, reads = 0, shown = null;

  const live = e => entries.get(e.key) === e;
  const touch = (e, notify) => { if (shown === e) notify(); };
  // A read that started before the one already applied for the same piece is older than the screen.
  const older = (e, piece, n) => {
    if ((e.seen.get(piece) ?? 0) > n) return true;
    e.seen.set(piece, n);
    return false;
  };

  function enter(loc) {
    let e = entries.get(loc.key);
    if (!e) {
      e = { key: loc.key, on: loc.on, cwd: loc.cwd, spaces: new Set(), root: null, rootSure: false, open: new Set(), pending: new Set(),
        selected: null, tree: null, git: null, error: null, reloading: false, used: 0, seen: new Map() };
      entries.set(loc.key, e);
    }
    e.spaces.add(loc.space);
    e.used = ++clock;
    shown = e;
    trim();
    return e;
  }

  const size = e => (e.git?.count ?? 0) + (e.tree ? [...e.tree.folders.values()].reduce((n, f) => n + f.list.length, 0) : 0);

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
    if (!live(e) || e.root !== root || older(e, `list:${folder}`, n)) return;
    const next = { list: got.entries, truncated: Boolean(got.truncated), skipped: Boolean(got.skipped) };
    const had = e.tree?.folders.get(folder);
    if (had && sameListing(had, next)) return;
    (e.tree ??= { folders: new Map() }).folders.set(folder, next);
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
          if (!folder) rootFailure = err;
          else if (e.root === root) e.open.delete(folder);
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(LISTINGS, folders.length) }, worker));
    if (rootFailure) throw rootFailure;
  }

  async function readGit(e, notify) {
    const n = ++reads;
    const got = await io.readGit(e.cwd, { on: e.on });
    if (!live(e) || older(e, "git", n)) return;
    if (!e.rootSure) {
      e.rootSure = true;
      const top = got.git?.top ?? e.cwd;
      // The folder was listed from where it was before git said where the repo is.
      if (top !== e.root) { e.root = top; e.open = new Set(); e.selected = null; e.tree = null; }
    }
    const git = got.git, marks = git?.marks ?? {};
    const next = { top: git?.top ?? null, branch: git?.branch ?? null, ahead: git?.ahead ?? 0, behind: git?.behind ?? 0,
      untrusted: Boolean(git?.untrusted), marksCut: Boolean(git?.truncated), failed: got.failed, marks };
    if (e.git && sameGit(e.git, next)) return;
    next.dirMarks = dirMarks(e.root, marks, io.rel);
    next.count = Object.keys(marks).length;
    e.git = next;
    touch(e, notify);
  }

  /**
   * Reads the tree again from where it is: git and every listing at once, each
   * applied when it arrives, and only if it changes what is shown. null when a
   * read is already running; else whether the root could be listed.
   */
  async function reload(e, notify) {
    if (!e.cwd || e.reloading || !live(e)) return null;
    e.reloading = true;
    const before = e.error;
    let failure = null;
    try {
      // Until git says where the repo is, the folder itself is the root.
      const root = e.root ??= e.cwd;
      const fail = err => { failure = err; };
      await Promise.all([
        readGit(e, notify).catch(fail),
        listAll(e, root, ["", ...[...e.open].sort()], notify).catch(fail),
      ]);
      if (e.root !== root && live(e)) await listAll(e, e.root, [""], notify).catch(fail);
      e.error = failure ? String(failure?.message ?? failure) : null;
      return !failure;
    } finally {
      e.reloading = false;
      trim();
      if (e.error !== before) touch(e, notify);
    }
  }

  /** Opens a folder: null when it could be listed, else { error, gone } and it stays closed. */
  async function expand(e, folder, notify) {
    e.open.add(folder);
    touch(e, notify);
    // Listed again on every open: what is kept from before shows meanwhile.
    if (e.pending.has(folder)) return null;
    e.pending.add(folder);
    try {
      await list(e, e.root, folder, notify);
      return null;
    } catch (error) {
      if (!live(e)) return null;
      e.open.delete(folder);
      const gone = !(await alive(e, folder));
      if (gone) drop(e, folder, notify);
      touch(e, notify);
      return { error, gone };
    } finally {
      e.pending.delete(folder);
      touch(e, notify);
    }
  }

  const alive = (e, rel) => Promise.resolve(io.exists(e.root, rel, e.on)).then(Boolean, () => true);

  /** `rel` is gone from its folder: it leaves the list, and what was open under it closes. */
  function drop(e, rel, notify) {
    const slash = rel.lastIndexOf("/");
    const folder = e.tree?.folders.get(slash < 0 ? "" : rel.slice(0, slash));
    if (folder) folder.list = folder.list.filter(x => x.name !== rel.slice(slash + 1));
    for (const o of [...e.open]) if (o === rel || o.startsWith(`${rel}/`)) e.open.delete(o);
    touch(e, notify);
  }

  /** Before opening a file the tree may be out of date about: whether it is still there; if not, it is dropped. */
  async function confirm(e, rel, notify) {
    if (await alive(e, rel)) return true;
    if (live(e)) drop(e, rel, notify);
    return false;
  }

  return { enter, forget, reload, expand, confirm, drop };
}
