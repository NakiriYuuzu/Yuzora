// Opening a file in the right pane, and small path helpers (paths.js).
import { panes, focused, run } from "herdr";
import { kindOf, documentFor, focusDocument, revealLine } from "yuzora:files";
import { base } from "./paths.js";

export { join, parent, base, rel } from "./paths.js";

const COLORS = {
  tsx: "#60a5fa", jsx: "#60a5fa", ts: "#e5c07b", mts: "#e5c07b", cts: "#e5c07b", js: "#e5c07b", mjs: "#e5c07b", cjs: "#e5c07b",
  css: "#f472b6", scss: "#f472b6", md: "#93c5fd", mdx: "#93c5fd", json: "#86d17a", jsonc: "#86d17a",
  rs: "#f08a5d", py: "#60a5fa", go: "#7dd3fc", toml: "#9ca3af", yaml: "#f87171", yml: "#f87171",
  html: "#f08a5d", sh: "#86d17a", zsh: "#86d17a", bash: "#86d17a", sql: "#a78bfa",
  png: "#c678dd", jpg: "#c678dd", jpeg: "#c678dd", gif: "#c678dd", webp: "#c678dd", svg: "#c678dd", ico: "#c678dd",
  lock: "#6f7078",
};
export const fileColor = name => {
  const dot = name.lastIndexOf(".");
  return (dot > 0 && COLORS[name.slice(dot + 1).toLowerCase()]) || "#8b8b94";
};

const MOD = { image: "yuzora-files-image", markdown: "yuzora-markdown", text: "yuzora-editor" };
// Image panes opened from here, by file: an image has no document to note its pane.
// An image pane keeps its file, so a live pane of that mod still shows it.
const imagePanes = new Map();
const imageKey = (path, on) => `${on ?? ""}\0${path}`;

/**
 * Opens a file: focuses the pane already showing it, else splits the focused
 * pane to the right (a new tab when no pane has focus). `at`: {line, column}.
 */
export function openFile(path, on, at = {}) {
  const kind = kindOf(path);
  if (kind === "image") {
    const shown = imagePanes.get(imageKey(path, on));
    if (shown && panes().some(p => p.pane === shown && p.mod === MOD.image)) { run("pane.focus", { pane: shown }); return; }
    imagePanes.delete(imageKey(path, on));
  }
  const open = kind === "image" ? null : documentFor(path, { on });
  if (open?.pane && panes().some(p => p.pane === open.pane)) {
    run("pane.focus", { pane: open.pane });
    // A preview there switches to the source for the line.
    if (at.line) revealLine(open.doc, at.line, at.column ?? 1);
    else focusDocument(open.doc);
    return;
  }
  const props = { path, on: on ?? null, opened: Date.now(), ...(at.line ? { line: at.line, column: at.column ?? 1 } : {}) };
  const f = focused();
  const pane = f?.pane ? panes().find(p => p.pane === f.pane) : null;
  if (pane) run("pane.split", { direction: "right", pane: pane.pane, mod: MOD[kind], props });
  else run("tab.new", { name: base(path), mod: MOD[kind], props });
  // The new pane has focus now.
  if (kind === "image") {
    const opened = focused()?.pane;
    if (opened && opened !== pane?.pane && panes().some(p => p.pane === opened && p.mod === MOD.image)) imagePanes.set(imageKey(path, on), opened);
  }
}
