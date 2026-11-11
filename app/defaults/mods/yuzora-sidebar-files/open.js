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
const FILE_MODS = [...DOC_MODS, MOD.image];
// A mod pane's props name its file (the layout keeps them, so a pane restored from it has them too).
const holds = (p, path, on) => p.props?.path === path && (p.props.on ?? null) === (on ?? null);

// The panes of the Space shown: a file a pane of another Space shows is opened again here, not shown there.
function shownPanes() {
  const tabs = new Set((spaces().find(s => s.active)?.tabs ?? []).map(t => t.id));
  return panes().filter(p => tabs.has(p.tab));
}

/**
 * The pane showing a document, and the document if it is open. The pane its document was last seen in, which
 * follows a rename; else a pane whose props name the file and that no document claims, one that has started ahead
 * of one that has not: a document says nothing about a pane that never ran, the other pane may be the one with it.
 */
function docPane(path, on) {
  const open = documentFor(path, { on });
  const all = shownPanes();
  const seen = open?.pane && all.find(p => p.pane === open.pane);
  if (seen) return { pane: seen, doc: open.doc };
  const mine = all.filter(p => DOC_MODS.includes(p.mod) && holds(p, path, on) && !paneDocument(p.pane));
  const there = mine.find(p => p.started) ?? mine[0];
  return there ? { pane: there, doc: there.started ? open?.doc ?? null : null } : null;
}

// An image has no document: its pane is the one whose props name it.
function imagePane(path, on) {
  const there = shownPanes().find(p => p.mod === MOD.image && holds(p, path, on));
  return there ? { pane: there, doc: null } : null;
}

/**
 * Opens a file: shows the pane (or tab) of the Space shown that already shows it, else opens a new tab of that Space
 * (`place` "tab", the default). `place` "side" always splits the focused pane to the right, though the file is
 * open (a tab when no pane has focus).
 * `at`: {line, column}. A line asked of a pane that has not started has no document to go to, and the pane
 * holds nothing, so a pane with the line takes its place: that is what a file that is not open gets.
 */
export function openFile(path, on, { place = "tab", ...at } = {}) {
  if (!PLACES.includes(place)) throw new Error(`unknown place: ${place}`);
  const kind = kindOf(path);
  const props = { path, on: on ?? null, opened: Date.now(), ...(at.line ? { line: at.line, column: at.column ?? 1 } : {}) };
  // To the side is asked for by name: a pane that already shows the file is no reason not to split.
  const open = place === "side" ? null : kind === "image" ? imagePane(path, on) : docPane(path, on);
  if (open) {
    const { pane, doc } = open;
    if (!pane.started && at.line) {
      replace(pane, props);
      run("tab.select", { id: pane.tab });
      return;
    }
    run("pane.focus", { pane: pane.pane });
    if (doc != null) {
      if (at.line) revealLine(doc, at.line, at.column ?? 1);
      else focusDocument(doc);
    }
    return;
  }
  const f = focused();
  const beside = place === "side" && f?.pane ? panes().find(p => p.pane === f.pane) : null;
  if (beside) run("pane.split", { direction: "right", pane: beside.pane, mod: MOD[kind], props });
  // The Space shown now, not the one shown by the time the command runs.
  else run("tab.new", { name: base(path), space: spaces().find(s => s.active)?.id, mod: MOD[kind], props });
}

/**
 * A pane's props can't change, so where nothing can follow a rename (a pane that has not started, an image) a new
 * pane with other props takes its place, beside it, and the old one goes. Says whether the old pane was in the tab shown.
 * There the pane the split makes has the focus, and `pane.swap` defaults to the focused pane, so swapping it with the
 * old one before that closes leaves the new one the old one's place and share of the row (`run()` doesn't say what the
 * split made, so a mod can't name it). In a tab not shown, or with a layout block holding the keyboard (no pane is
 * focused), the focused pane is not it: the pane before the old one then takes half of its share.
 */
function replace(pane, props) {
  const f = focused();
  const here = Boolean(f?.pane) && f.tab === pane.tab;
  run("pane.split", { direction: "right", pane: pane.pane, mod: pane.mod, props });
  if (here) run("pane.swap", { target: pane.pane });
  run("pane.close", { pane: pane.pane });
  return here;
}

/**
 * After `from` was renamed to `to`, the panes whose props name it (or a file under it). The props can't change, so
 * what a rename does depends on the pane, which the host says has started or not and a document may claim:
 * - not started: a new pane with the new name takes its place, though another pane holds the file's document.
 * - an image, started or not: the same, there being no document to follow the rename.
 * - started, no document claims it: its document followed the rename; say whose it is, so the new name finds this pane,
 *   unless a pane of the list already is its document's: a document names one pane, so a second pane still on the old
 *   name would be found by a file made under it. That pane is replaced like one that has not started.
 * - started, a document claims it: nothing, that document followed the rename by itself.
 * Focus stays put; a replaced pane that had the keys hands them to its new pane.
 */
export function moved(from, to, on) {
  const before = focused()?.pane;
  const all = panes();
  const listed = new Set(all.map(p => p.pane));
  const gone = new Set();
  // A split gives its tab's keys to the new pane: the pane that has them is replaced last in its tab.
  for (const p of [...all].sort((a, b) => (a.pane === before) - (b.pane === before))) {
    const path = p.props?.path;
    if (!FILE_MODS.includes(p.mod) || !path || (p.props.on ?? null) !== (on ?? null)) continue;
    const rest = path === from ? "" : path.startsWith(from + "/") ? path.slice(from.length) : null;
    if (rest === null) continue;
    const swapIn = () => {
      const here = replace(p, { path: to + rest, on: on ?? null, opened: Date.now() });
      gone.add(p.pane);
      // The split put the zoom of the tab shown away: back on the pane it was on (the new one if that is the one replaced).
      const zoomed = here && all.find(q => q.tab === p.tab && q.zoomed);
      if (zoomed) run("pane.zoom", zoomed.pane === p.pane ? {} : { pane: zoomed.pane });
    };
    if (!p.started || p.mod === MOD.image) swapIn();
    else if (!paneDocument(p.pane)) {
      const open = documentFor(to + rest, { on });
      if (open?.pane && listed.has(open.pane)) swapIn();
      else if (open) notePane(open.doc, p.pane);
    }
  }
  // A split takes the keys; a pane that was replaced hands them to its new pane.
  if (gone.size && before && !gone.has(before)) run("pane.focus", { pane: before });
}
