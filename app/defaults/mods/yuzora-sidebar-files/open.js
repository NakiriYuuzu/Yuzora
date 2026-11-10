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

// An image has no document: its pane is the one whose props name it.
function imagePane(path, on) {
  const there = panes().find(p => p.mod === MOD.image && holds(p, path, on));
  return there ? { pane: there.pane, doc: null } : null;
}

/**
 * Opens a file: shows the pane (or tab) already showing it, else opens a new tab of the Space shown
 * (`place` "tab", the default) or splits the focused pane to the right ("side"; a tab when no pane has focus).
 * `at`: {line, column}.
 */
export function openFile(path, on, { place = "tab", ...at } = {}) {
  if (!PLACES.includes(place)) throw new Error(`unknown place: ${place}`);
  const kind = kindOf(path);
  const open = kind === "image" ? imagePane(path, on) : docPane(path, on);
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
  const f = focused();
  const beside = place === "side" && f?.pane ? panes().find(p => p.pane === f.pane) : null;
  if (beside) run("pane.split", { direction: "right", pane: beside.pane, mod: MOD[kind], props });
  // The Space shown now, not the one shown by the time the command runs.
  else run("tab.new", { name: base(path), space: spaces().find(s => s.active)?.id, mod: MOD[kind], props });
}

/**
 * After `from` was renamed to `to`, the panes showing it (or a file under it). A document follows the rename in
 * the same pane, whose props keep the old name: say whose it is, so the new name finds that pane. An image pane's
 * props can't change, so a new pane takes the old one's place; focus stays put.
 */
export function moved(from, to, on) {
  const before = focused()?.pane;
  const gone = new Set();
  for (const p of panes()) {
    const path = p.props?.path;
    if (!path || (p.props.on ?? null) !== (on ?? null)) continue;
    const rest = path === from ? "" : path.startsWith(from + "/") ? path.slice(from.length) : null;
    if (rest === null) continue;
    if (DOC_MODS.includes(p.mod)) {
      const open = documentFor(to + rest, { on });
      if (open) notePane(open.doc, p.pane);
    } else if (p.mod === MOD.image) {
      run("pane.split", { direction: "right", pane: p.pane, mod: MOD.image, props: { path: to + rest, on: on ?? null, opened: Date.now() } });
      run("pane.close", { pane: p.pane });
      gone.add(p.pane);
    }
  }
  // A split takes the keys; a pane that was replaced hands them to its new pane.
  if (gone.size && before && !gone.has(before)) run("pane.focus", { pane: before });
}
