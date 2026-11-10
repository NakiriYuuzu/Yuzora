// Opening a file in a tab of its own or beside the focused pane, and small path helpers (paths.js).
import { spaces, panes, focused, run } from "herdr";
import { kindOf, documentFor, paneDocument, notePane, focusDocument, revealLine } from "yuzora:files";
import { base } from "./paths.js";

export { join, parent, base, rel } from "./paths.js";

const MOD = { image: "yuzora-files-image", markdown: "yuzora-markdown", text: "yuzora-editor" };
// Where a file opens: a new tab of the Space shown, or beside the focused pane.
const PLACES = ["tab", "side"];
// A document is shown by an editor pane or a Markdown pane alike.
const DOC_MODS = [MOD.text, MOD.markdown];
// A mod pane's props name its file (the layout keeps them, so a pane restored from it has them too).
const holds = (p, path, on) => p.props?.path === path && (p.props.on ?? null) === (on ?? null);
// Image panes opened from here, by file: an image has no document to note its pane.
// An image pane keeps its file, so a live pane of that mod still shows it.
const imagePanes = new Map();
const imageKey = (path, on) => `${on ?? ""}\0${path}`;
// `run` only queues a command, so the pane an image opens in shows in panes() a moment later: it is the one
// image pane that was not there when the image was asked for (more than one: not guessed). One image waits at a time.
let arriving = null;
const imagePaneIds = () => panes().filter(p => p.mod === MOD.image).map(p => p.pane);
const expectImage = key => { arriving = { key, known: new Set(imagePaneIds()) }; };
function landImage() {
  if (!arriving) return;
  const fresh = imagePaneIds().filter(id => !arriving.known.has(id));
  if (fresh.length === 1) imagePanes.set(arriving.key, fresh[0]);
  if (fresh.length) arriving = null;
}

/**
 * The pane showing a document, and the document if it is open. The pane its document was last seen in, which
 * follows a rename; else a pane whose props name the file and that no document claims. A pane the layout restored
 * is one: its tab may not have been shown since the app started, and then it has no document at all.
 */
function docPane(path, on) {
  const open = documentFor(path, { on });
  const all = panes();
  if (open?.pane && all.some(p => p.pane === open.pane)) return { pane: open.pane, doc: open.doc };
  const there = all.find(p => DOC_MODS.includes(p.mod) && holds(p, path, on) && !paneDocument(p.pane));
  return there ? { pane: there.pane, doc: open?.doc ?? null } : null;
}

/**
 * Opens a file: shows the pane (or tab) already showing it, else opens a new tab of the Space shown
 * (`place` "tab", the default) or splits the focused pane to the right ("side"; a tab when no pane has focus).
 * `at`: {line, column}.
 */
export function openFile(path, on, { place = "tab", ...at } = {}) {
  if (!PLACES.includes(place)) throw new Error(`unknown place: ${place}`);
  landImage();
  const kind = kindOf(path);
  if (kind === "image") {
    const shown = imagePanes.get(imageKey(path, on));
    if (shown && panes().some(p => p.pane === shown && p.mod === MOD.image)) { run("pane.focus", { pane: shown }); return; }
    imagePanes.delete(imageKey(path, on));
  }
  const open = kind === "image" ? null : docPane(path, on);
  if (open) {
    run("pane.focus", { pane: open.pane });
    // A pane that has not started has no document to ask: it opens at the top, and its pane takes the keys.
    // A preview there switches to the source for the line.
    if (open.doc != null) {
      if (at.line) revealLine(open.doc, at.line, at.column ?? 1);
      else focusDocument(open.doc);
    }
    return;
  }
  const props = { path, on: on ?? null, opened: Date.now(), ...(at.line ? { line: at.line, column: at.column ?? 1 } : {}) };
  if (kind === "image") expectImage(imageKey(path, on));
  const f = focused();
  const beside = place === "side" && f?.pane ? panes().find(p => p.pane === f.pane) : null;
  if (beside) run("pane.split", { direction: "right", pane: beside.pane, mod: MOD[kind], props });
  // The Space shown now, not the one shown by the time the command runs.
  else run("tab.new", { name: base(path), space: spaces().find(s => s.active)?.id, mod: MOD[kind], props });
}

// A renamed document stays in its pane, whose props keep the name it was opened with: say whose it is, so the
// new name finds it.
function noteRenamed(from, to, on) {
  for (const p of panes()) {
    if (!DOC_MODS.includes(p.mod) || !p.props?.path || (p.props.on ?? null) !== (on ?? null)) continue;
    const rest = p.props.path === from ? "" : p.props.path.startsWith(from + "/") ? p.props.path.slice(from.length) : null;
    const open = rest === null ? null : documentFor(to + rest, { on });
    if (open) notePane(open.doc, p.pane);
  }
}

/**
 * After `from` was renamed to `to`: an image pane opened from here showing it
 * (or a file under it) shows the new path instead, in the same place. Props
 * can't change, so a new pane takes the old one's place; focus stays put.
 */
export function moved(from, to, on) {
  landImage();
  const before = focused()?.pane;
  const gone = new Set();
  noteRenamed(from, to, on);
  for (const [key, pane] of [...imagePanes]) {
    const [machine, path] = key.split("\0");
    if (machine !== (on ?? "")) continue;
    const rest = path === from ? "" : path.startsWith(from + "/") ? path.slice(from.length) : null;
    if (rest === null) continue;
    imagePanes.delete(key);
    if (!panes().some(p => p.pane === pane && p.mod === MOD.image)) continue;
    const next = to + rest;
    expectImage(imageKey(next, on));
    run("pane.split", { direction: "right", pane, mod: MOD.image, props: { path: next, on: on ?? null, opened: Date.now() } });
    run("pane.close", { pane });
    gone.add(pane);
  }
  // A split takes the keys; a pane that was replaced hands them to its new pane.
  if (gone.size && before && !gone.has(before)) run("pane.focus", { pane: before });
}
