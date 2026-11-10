// yuzora(v0.1.0 db-retarget): what a Yuzora mod's render can reach that the engine refuses in a frame
// (cx.notify, creating state, changing an input, binding keys) or that changes the app from a frame
// (setShared, run, overlays, a task started there).
//
// A frame may only look. The engine throws on the first group, and the throw leaves whatever ran
// before it half done: the Database table pane changed its target, then failed on `cx.notify()`
// inside the load it started, and stayed on "Loading…" for good. This walks the call graph from every
// `render` (through `this.x()`, local functions and closures, and `./x.js` imports) and lists each such
// call it can reach, with the chain that reaches it. It reads names, not conditions: a finding is a
// place to look, and scripts/yuzora-render-effects.test.ts holds the answer for each one.
//
// What runs in the frame: the function itself, the closures it passes to a callee that calls them
// (`.map`, `.when`, a local `guard(cx, action)`), and a task up to its first `await`. What does not: a
// handler (`on_*`), a timer callback, a closure that is only stored or passed on.
//
//   bun scripts/yuzora-render-effects.ts            every finding, with its chain
//   bun test scripts/yuzora-render-effects.test.ts  the findings against the reviewed list
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

type Tok = { k: "id" | "num" | "str" | "re" | "p"; v: string; at: number };

const REGEX_AFTER = new Set(["return", "typeof", "case", "do", "else", "in", "of", "void", "delete", "throw", "new", "instanceof", "yield", "await"]);
const KEYWORDS = new Set(["if", "for", "while", "switch", "catch", "function", "return", "typeof", "new", "await", "async", "else", "do", "try", "with", "super", "import", "export", "void", "delete", "in", "of", "yield"]);

export function lex(src: string): Tok[] {
  const out: Tok[] = [];
  lexInto(src, 0, out, false);
  return out;
}

function regexAllowed(out: Tok[]): boolean {
  const prev = out[out.length - 1];
  if (!prev) return true;
  if (prev.k === "id") return REGEX_AFTER.has(prev.v);
  if (prev.k === "num" || prev.k === "str" || prev.k === "re") return false;
  return ![")", "]", "}"].includes(prev.v);
}

function lexTemplate(src: string, from: number, out: Tok[]): number {
  let pos = from + 1;
  out.push({ k: "str", v: "`", at: from });
  while (pos < src.length && src[pos] !== "`") {
    if (src[pos] === "\\") pos += 2;
    else if (src[pos] === "$" && src[pos + 1] === "{") {
      out.push({ k: "p", v: "(", at: pos });
      pos = lexInto(src, pos + 2, out, true);
      out.push({ k: "p", v: ")", at: pos });
    } else pos++;
  }
  return pos + 1;
}

function lexInto(src: string, from: number, out: Tok[], inTemplate: boolean): number {
  let pos = from;
  let depth = 0;
  const n = src.length;
  while (pos < n) {
    const c = src[pos];
    if (/\s/.test(c)) { pos++; continue; }
    if (c === "/" && src[pos + 1] === "/") { const e = src.indexOf("\n", pos); pos = e < 0 ? n : e; continue; }
    if (c === "/" && src[pos + 1] === "*") { const e = src.indexOf("*/", pos + 2); pos = e < 0 ? n : e + 2; continue; }
    if (c === '"' || c === "'") {
      const s = pos++;
      while (pos < n && src[pos] !== c) pos += src[pos] === "\\" ? 2 : 1;
      pos++;
      out.push({ k: "str", v: src.slice(s, pos), at: s });
      continue;
    }
    if (c === "`") { pos = lexTemplate(src, pos, out); continue; }
    if (/[A-Za-z_$]/.test(c) || c.charCodeAt(0) > 127) {
      const s = pos;
      while (pos < n && (/[A-Za-z0-9_$]/.test(src[pos]) || src.charCodeAt(pos) > 127)) pos++;
      out.push({ k: "id", v: src.slice(s, pos), at: s });
      continue;
    }
    if (/[0-9]/.test(c)) {
      const s = pos;
      while (pos < n && /[0-9A-Za-z_.]/.test(src[pos]) && !(src[pos] === "." && src[pos + 1] === ".")) pos++;
      out.push({ k: "num", v: src.slice(s, pos), at: s });
      continue;
    }
    if (c === "/" && regexAllowed(out)) {
      const s = pos++;
      let inClass = false;
      while (pos < n && (src[pos] !== "/" || inClass)) {
        if (src[pos] === "\\") pos++;
        else if (src[pos] === "[") inClass = true;
        else if (src[pos] === "]") inClass = false;
        pos++;
      }
      pos++;
      while (pos < n && /[a-z]/.test(src[pos])) pos++;
      out.push({ k: "re", v: src.slice(s, pos), at: s });
      continue;
    }
    if (inTemplate) {
      if (c === "{") depth++;
      else if (c === "}") {
        if (depth === 0) return pos + 1;
        depth--;
      }
    }
    const two = src.slice(pos, pos + 2);
    const three = src.slice(pos, pos + 3);
    const v = three === "..." ? three : two === "=>" || two === "?." ? two : c;
    out.push({ k: "p", v, at: pos });
    pos += v.length;
  }
  return pos;
}

export type Fn = { name: string; owner: string | null; file: string; from: number; to: number; params: string[]; async: boolean };
type Cls = { name: string; ext: string | null; methods: Map<string, Fn> };
type Import = { from: string; name: string };
export type Unit = {
  file: string;
  src: string;
  toks: Tok[];
  match: number[];
  fns: Map<string, Fn>;
  classes: Map<string, Cls>;
  imports: Map<string, Import>;
  defaultName: string | null;
};

function pairs(toks: Tok[], file: string): number[] {
  const match: number[] = new Array(toks.length).fill(-1);
  const stack: number[] = [];
  const close: Record<string, string> = { ")": "(", "]": "[", "}": "{" };
  toks.forEach((t, i) => {
    if (t.k !== "p") return;
    if ("([{".includes(t.v)) stack.push(i);
    else if (t.v in close) {
      const open = stack.pop();
      if (open === undefined || toks[open].v !== close[t.v]) throw new Error(`${file}: unbalanced ${t.v} at ${t.at}`);
      match[open] = i;
      match[i] = open;
    }
  });
  if (stack.length) throw new Error(`${file}: unclosed ${toks[stack[stack.length - 1]].v}`);
  return match;
}

const DECL = new Set(["const", "let", "var", "function", "class", "export", "import"]);
const opens = (t: Tok | undefined) => t?.k === "p" && "([{".includes(t.v);

/** The names a parameter list `(open, close)` binds (a destructured parameter binds none this reads). */
function paramNames(toks: Tok[], open: number, close: number, match: number[]): string[] {
  const names: string[] = [];
  let segment = true;
  for (let i = open + 1; i < close; i++) {
    const t = toks[i];
    if (segment) {
      segment = false;
      if (t.v === "...") continue;
      names.push(t.k === "id" ? t.v : "");
    }
    if (opens(t)) i = match[i];
    else if (t.v === "," && t.k === "p") segment = true;
  }
  return names;
}

export function parse(file: string, src: string): Unit {
  const toks = lex(src);
  const match = pairs(toks, file);
  const unit: Unit = { file, src, toks, match, fns: new Map(), classes: new Map(), imports: new Map(), defaultName: null };
  const is = (i: number, v: string) => toks[i]?.v === v && toks[i].k !== "str";
  const statementEnd = (i: number) => {
    let j = i;
    while (j < toks.length && !is(j, ";") && !(j > i && toks[j].k === "id" && DECL.has(toks[j].v))) j = opens(toks[j]) ? match[j] + 1 : j + 1;
    return j;
  };
  let i = 0;
  while (i < toks.length) {
    const t = toks[i];
    if (t.k === "id" && t.v === "import") {
      let j = i + 1;
      const locals: [string, string][] = [];
      while (j < toks.length && toks[j].k !== "str") {
        if (is(j, "{")) {
          for (let k = j + 1; k < match[j]; k++) {
            if (toks[k].k !== "id") continue;
            const aliased = toks[k + 1]?.v === "as";
            locals.push([aliased ? toks[k + 2].v : toks[k].v, toks[k].v]);
            if (aliased) k += 2;
          }
          j = match[j];
        } else if (is(j, "*")) {
          locals.push([toks[j + 2].v, "*"]);
          j += 2;
        } else if (toks[j].k === "id" && toks[j].v !== "from") locals.push([toks[j].v, "default"]);
        j++;
      }
      const from = toks[j]?.v.slice(1, -1) ?? "";
      for (const [local, name] of locals) unit.imports.set(local, { from, name });
      i = j + 1;
      continue;
    }
    if (t.k === "id" && (t.v === "export" || t.v === "default")) {
      if (t.v === "default" && !["class", "function", "async"].includes(toks[i + 1]?.v)) {
        i = statementEnd(i);
        continue;
      }
      i++;
      continue;
    }
    if (t.k === "id" && t.v === "class") {
      const name = toks[i + 1].v;
      let j = i + 2;
      let ext: string | null = null;
      if (toks[j]?.v === "extends") { ext = toks[j + 1].v; j += 2; }
      while (!is(j, "{")) j++;
      const cls: Cls = { name, ext, methods: new Map() };
      let k = j + 1;
      while (k < match[j]) {
        let async = false;
        while (["static", "async", "get", "set", "*"].includes(toks[k].v) && !is(k + 1, "(")) {
          if (toks[k].v === "async") async = true;
          k++;
        }
        if (toks[k].k === "id" && is(k + 1, "(") && is(match[k + 1] + 1, "{")) {
          const body = match[k + 1] + 1;
          cls.methods.set(toks[k].v, { name: toks[k].v, owner: name, file, from: body + 1, to: match[body], params: paramNames(toks, k + 1, match[k + 1], match), async });
          k = match[body] + 1;
        } else throw new Error(`${file}: a class member this reader does not understand at ${toks[k].at}: ${toks[k].v}`);
      }
      unit.classes.set(name, cls);
      if (toks[i - 1]?.v === "default") unit.defaultName = name;
      i = match[j] + 1;
      continue;
    }
    const async = t.k === "id" && t.v === "async" && toks[i + 1]?.v === "function";
    const fnAt = async ? i + 1 : i;
    if (toks[fnAt].k === "id" && toks[fnAt].v === "function") {
      const nameAt = is(fnAt + 1, "*") ? fnAt + 2 : fnAt + 1;
      const name = toks[nameAt].v;
      const body = match[nameAt + 1] + 1;
      unit.fns.set(name, { name, owner: null, file, from: body + 1, to: match[body], params: paramNames(toks, nameAt + 1, match[nameAt + 1], match), async });
      if (toks[i - 1]?.v === "default") unit.defaultName = name;
      i = match[body] + 1;
      continue;
    }
    if (t.k === "id" && ["const", "let", "var"].includes(t.v) && toks[i + 1]?.k === "id" && is(i + 2, "=")) {
      const name = toks[i + 1].v;
      let j = i + 3;
      const isAsync = toks[j]?.v === "async" && toks[j].k === "id";
      if (isAsync) j++;
      let arrow = -1;
      let params: string[] = [];
      if (is(j, "(") && is(match[j] + 1, "=>")) { arrow = match[j] + 1; params = paramNames(toks, j, match[j], match); }
      else if (toks[j]?.k === "id" && is(j + 1, "=>")) { arrow = j + 1; params = [toks[j].v]; }
      if (arrow >= 0) {
        if (is(arrow + 1, "{")) {
          unit.fns.set(name, { name, owner: null, file, from: arrow + 2, to: match[arrow + 1], params, async: isAsync });
          i = match[arrow + 1] + 1;
        } else {
          const end = statementEnd(arrow + 1);
          unit.fns.set(name, { name, owner: null, file, from: arrow + 1, to: end, params, async: isAsync });
          i = end;
        }
        continue;
      }
      i = statementEnd(i + 1);
      continue;
    }
    i = opens(t) ? match[i] + 1 : i + 1;
  }
  return unit;
}

// What the engine refuses in a frame (vendor/gpui-kit/crates/shell/src/engine/quickjs: refuse_*, allows_notify).
const INPUT_CALLS = new Set(["set_selected_range", "set_value", "replace_with_token", "replace_range_with_token", "focus"]);
const STATE_CLASSES = new Set(["InputState", "TextareaState", "EditorState", "SliderState", "CalendarState", "OtpState", "VirtualListScrollHandle", "DockArea"]);
// The window, theme, dock and nested-view calls that are refused the same way (window_api, theme_api, dock_api, mod.rs refuse_nested_view_mutation).
const REFUSED_CALLS = new Set(["set_theme", "set_rem_size", "focus_next", "focus_prev", "activate_window", "minimize_window", "zoom_window", "toggle_fullscreen", "dispatch_action", "add_panel", "remove_panel", "toggle_dock", "remove_dock", "set_dock_size", "set_dock_collapsible", "set_locked", "zoom_out", "set_props"]);
// What the `herdr` module does to the app when it is called (it queues the change).
const HERDR_CHANGES = new Set(["setShared", "run", "openOverlay", "closeOverlay", "notify", "playSound", "setAgentOrder", "presentUpdateNotes", "writeTextFile", "sendFeedback", "pickFolder", "exec", "hc", "feedbackPasteImage", "feedbackClearImages", "feedbackAttachImages", "feedbackPickImages"]);

// What a `yuzora:*` host function does when called in a frame: these only read. Any other that a render reaches is
// reported (`openTable` starts a job), so a new host call has to be looked at once.
const HOST_READS = new Set([
  "yuzora:core.getSetting", "yuzora:core.motion", "yuzora:core.panes", "yuzora:core.session", "yuzora:core.settingsPages", "yuzora:core.t", "yuzora:core.tokens",
  "yuzora:db.connections", "yuzora:db.history", "yuzora:db.result",
  "yuzora:git.logInfo", "yuzora:git.logRows", "yuzora:git.settings",
  "yuzora:settings.templateText", "yuzora:shell.spaceBadges", "yuzora:shell.workspace",
  "yuzora:ssh.edits", "yuzora:ssh.hosts", "yuzora:ssh.transfers",
]);

// Callees that call the closure they are given before they return, and those that keep it for another phase.
// A callee not named here (or a local one that calls its parameter) is reported by `scanMod` as unclassified.
const CALLS_NOW = new Set(["map", "filter", "forEach", "find", "findIndex", "findLast", "some", "every", "reduce", "flatMap", "sort", "toSorted", "when", "unless", "from", "replace", "replaceAll", "uniform_list", "virtual_list", "v_virtual_list"]);
const KEEPS = new Set(["hover", "tooltip", "then", "catch", "finally", "after", "every", "observe", "subscribe", "on_size_change"]);

export type Finding = { mod: string; file: string; where: string; effect: string; detail: string; chain: string[]; line: number };

type Event = { kind: "this" | "super" | "call" | "effect"; name: string; effect?: string; at: number; ids: string[]; ns?: string };
type Scope = { events: Event[]; closures: Map<string, Fn>; unclassified: Set<string> };
type Ctx = {
  resolveThis(name: string): { fn: Fn; unit: Unit } | undefined;
  resolveBare(unit: Unit, name: string, closures: Map<string, Fn>, ns?: string): { fn: Fn; unit: Unit } | undefined;
};

type Lit = { start: number; end: number; from: number; to: number; async: boolean; params: string[] };

function literalAt(unit: Unit, arrow: number): Lit {
  const { toks, match } = unit;
  let start: number;
  let params: string[];
  if (toks[arrow - 1].v === ")") {
    start = match[arrow - 1];
    params = paramNames(toks, start, arrow - 1, match);
  } else {
    start = arrow - 1;
    params = [toks[start].v];
  }
  const async = toks[start - 1]?.v === "async" && toks[start - 1].k === "id";
  if (async) start--;
  if (toks[arrow + 1].v === "{" && toks[arrow + 1].k === "p") return { start, end: match[arrow + 1] + 1, from: arrow + 2, to: match[arrow + 1], async, params };
  let j = arrow + 1;
  while (j < toks.length) {
    const t = toks[j];
    if (t.k === "p" && ([")", "]", "}", ",", ";"].includes(t.v))) break;
    j = opens(t) ? match[j] + 1 : j + 1;
  }
  return { start, end: j, from: arrow + 1, to: j, async, params };
}

/** The closures a body names (`const leave = () => …`, a nested `function f() {}`): their code runs when they are called. */
function closuresIn(unit: Unit, fn: Fn): Map<string, Fn> {
  const { toks, match } = unit;
  const closures = new Map<string, Fn>();
  for (let i = fn.from; i < fn.to; i++) {
    const t = toks[i];
    if (t.v === "=>" && t.k === "p") {
      const lit = literalAt(unit, i);
      const named = toks[lit.start - 1]?.v === "=" && toks[lit.start - 2]?.k === "id" && ["const", "let", "var"].includes(toks[lit.start - 3]?.v);
      if (named) closures.set(toks[lit.start - 2].v, { name: `${fn.name}/${toks[lit.start - 2].v}`, owner: fn.owner, file: fn.file, from: lit.from, to: lit.to, params: lit.params, async: lit.async });
    } else if (t.k === "id" && t.v === "function" && toks[i + 1]?.k === "id" && toks[i + 2]?.v === "(") {
      const body = match[i + 2] + 1;
      closures.set(toks[i + 1].v, { name: `${fn.name}/${toks[i + 1].v}`, owner: fn.owner, file: fn.file, from: body + 1, to: match[body], params: paramNames(toks, i + 2, match[i + 2], match), async: toks[i - 1]?.v === "async" });
    }
  }
  return closures;
}

/** The arguments of the call opened at `open`: where each starts and ends, and the `=>` of the one that is a closure. */
function argsOf(unit: Unit, open: number): { from: number; to: number; arrow: number }[] {
  const { toks, match } = unit;
  const close = match[open];
  const args: { from: number; to: number; arrow: number }[] = [];
  let from = open + 1;
  for (let j = open + 1; j <= close; j++) {
    const t = toks[j];
    if (j === close || (t.k === "p" && t.v === ",")) {
      if (j > from) {
        let arrow = -1;
        for (let k = from; k < j; k++) {
          if (toks[k].v === "=>" && toks[k].k === "p") { arrow = k; break; }
          if (opens(toks[k]) && match[k] + 1 < j && toks[match[k] + 1]?.v !== "=>") break;
          if (opens(toks[k])) k = match[k];
        }
        args.push({ from, to: j, arrow });
      }
      from = j + 1;
    } else if (opens(t)) j = match[j];
  }
  return args;
}

/** What runs when `fn` runs in a frame: its calls and effects, in order. */
function scopeOf(unit: Unit, fn: Fn, ctx: Ctx, invokes: (target: { fn: Fn; unit: Unit }, k: number) => boolean): Scope {
  const { toks, match } = unit;
  const closures = closuresIn(unit, fn);
  const scope: Scope = { events: [], closures, unclassified: new Set() };
  const now = new Set<number>();
  const limits: number[] = fn.async ? [fn.to] : [];
  const ids = (args: { from: number; to: number }[]) => args.map((a) => (a.to - a.from === 1 && toks[a.from].k === "id" ? toks[a.from].v : ""));
  for (let i = fn.from; i < fn.to; i++) {
    while (limits.length && i >= limits[limits.length - 1]) limits.pop();
    const t = toks[i];
    if (t.k === "p" && t.v === "=>") {
      const lit = literalAt(unit, i);
      if (!now.has(i)) { i = lit.end - 1; continue; }
      if (lit.async) limits.push(lit.to);
      continue;
    }
    if (t.k !== "id") continue;
    if (t.v === "function" && toks[i + 1]?.k === "id" && toks[i + 2]?.v === "(") { i = match[match[i + 2] + 1]; continue; }
    if (t.v === "await") {
      if (limits.length) i = limits.pop()! - 1;
      continue;
    }
    if (!(toks[i + 1]?.v === "(" && toks[i + 1].k === "p")) continue;
    const prev = toks[i - 1];
    const member = prev?.v === "." || prev?.v === "?.";
    const open = i + 1;
    const args = argsOf(unit, open);
    const literal = args.map((a) => a.arrow).filter((a) => a >= 0);
    const call = (kind: Event["kind"], name: string, effect?: string, ns?: string) => scope.events.push({ kind, name, effect, at: t.at, ids: ids(args), ns });
    if (member) {
      const obj = toks[i - 2];
      if (t.v.startsWith("on_") || (["after", "every"].includes(t.v) && obj?.v === "timer")) { i = match[open]; continue; }
      const viaThis = obj?.v === "this" && !(toks[i - 3]?.v === "." || toks[i - 3]?.v === "?.");
      if (t.v === "spawn") {
        call("effect", "spawn", "spawn");
        for (const a of literal) now.add(a);
      } else if (t.v === "notify") call("effect", "cx.notify", "notify");
      else if (t.v === "focus_handle" || t.v === "bind_keys" || REFUSED_CALLS.has(t.v) || (t.v === "refresh" && obj?.v === "window")) call("effect", t.v, "refused");
      else if (t.v === "new" && (STATE_CLASSES.has(obj?.v) || obj?.v === "cx")) call("effect", `${obj.v}.new`, "refused");
      else if (obj?.v === "super") call("super", t.v);
      else if (INPUT_CALLS.has(t.v)) call("effect", t.v, "input");
      else if (viaThis) {
        call("this", t.v);
        const target = ctx.resolveThis(t.v);
        if (target) args.forEach((a, k) => { if (a.arrow >= 0 && invokes(target, k)) now.add(a.arrow); });
        else if (literal.length) scope.unclassified.add(`this.${t.v}`);
      } else if (obj?.k === "id" && unit.imports.get(obj.v)?.name === "*" && !(toks[i - 3]?.v === "." || toks[i - 3]?.v === "?.")) {
        call("call", t.v, undefined, obj.v);
        const target = ctx.resolveBare(unit, t.v, closures, obj.v);
        if (target) args.forEach((a, k) => { if (a.arrow >= 0 && invokes(target, k)) now.add(a.arrow); });
        else if (literal.length) scope.unclassified.add(`${obj.v}.${t.v}`);
      } else if (CALLS_NOW.has(t.v)) for (const a of literal) now.add(a);
      else if (literal.length && !KEEPS.has(t.v)) scope.unclassified.add(`.${t.v}`);
      continue;
    }
    if (prev?.v === "new" || KEYWORDS.has(t.v)) continue;
    const imported = unit.imports.get(t.v);
    if (imported?.from === "herdr" && HERDR_CHANGES.has(imported.name)) { call("effect", imported.name === "notify" ? "toast" : imported.name, "changes"); continue; }
    if (imported?.from.startsWith("gpui") && REFUSED_CALLS.has(imported.name)) { call("effect", imported.name, "refused"); continue; }
    call("call", t.v);
    const target = ctx.resolveBare(unit, t.v, closures);
    if (target) args.forEach((a, k) => { if (a.arrow >= 0 && invokes(target, k)) now.add(a.arrow); });
    else if (CALLS_NOW.has(t.v)) for (const a of literal) now.add(a);
    else if (literal.length) scope.unclassified.add(t.v);
  }
  return scope;
}

export type Mod = { name: string; units: Map<string, Unit> };

export function loadMod(name: string, dir: string): Mod {
  const units = new Map<string, Unit>();
  for (const f of readdirSync(dir)) if (f.endsWith(".js")) units.set(join(dir, f), parse(`${name}/${f}`, readFileSync(join(dir, f), "utf8")));
  return { name, units };
}

/** A mod from sources in memory: `files` maps a file name to its text. */
export function modFrom(name: string, files: Record<string, string>): Mod {
  return { name, units: new Map(Object.entries(files).map(([f, src]) => [`${name}/${f}`, parse(`${name}/${f}`, src)])) };
}

function lineOf(unit: Unit, at: number): number {
  return unit.src.slice(0, at).split("\n").length;
}

function unitFor(mod: Mod, unit: Unit, from: string): Unit | undefined {
  if (!from.startsWith(".")) return undefined;
  return [...mod.units.values()].find((u) => u.file.endsWith(`/${from.replace(/^\.\//, "")}`));
}

function classOf(mod: Mod, unit: Unit, name: string): { cls: Cls; unit: Unit } | null {
  const local = unit.classes.get(name);
  if (local) return { cls: local, unit };
  const imp = unit.imports.get(name);
  const target = imp && unitFor(mod, unit, imp.from);
  if (!imp || !target) return null;
  const cls = imp.name === "default" && target.defaultName ? target.classes.get(target.defaultName) : target.classes.get(imp.name);
  return cls ? { cls, unit: target } : null;
}

/** The class `fn` is a method of extends, for `super.x()`. */
function parentOf(mod: Mod, unit: Unit, fn: Fn): { cls: Cls; unit: Unit } | null {
  const own = fn.owner ? unit.classes.get(fn.owner) : undefined;
  return own?.ext ? classOf(mod, unit, own.ext) : null;
}

/** Every method of `cls` and of its parents, nearest first. */
function methodsOf(mod: Mod, unit: Unit, cls: Cls): Map<string, { fn: Fn; unit: Unit }> {
  const all = new Map<string, { fn: Fn; unit: Unit }>();
  let at: { cls: Cls; unit: Unit } | null = { cls, unit };
  while (at) {
    for (const [name, fn] of at.cls.methods) if (!all.has(name)) all.set(name, { fn, unit: at.unit });
    at = at.cls.ext ? classOf(mod, at.unit, at.cls.ext) : null;
  }
  return all;
}

function label(fn: Fn): string {
  const file = fn.file.split("/").slice(1).join("/");
  return `${file === "main.js" ? "" : `${file}#`}${fn.owner ? `${fn.owner}.` : ""}${fn.name}`;
}

export type Report = { findings: Finding[]; unclassified: string[]; renders: string[]; host: string[] };

/** What each `render` of the mod's classes can reach. */
export function scanMod(mod: Mod): Report {
  const found = new Map<string, Finding>();
  const unclassified = new Set<string>();
  const host = new Set<string>();
  const renders: string[] = [];
  for (const unit of mod.units.values()) {
    for (const cls of unit.classes.values()) {
      const methods = methodsOf(mod, unit, cls);
      if (!methods.has("render")) continue;
      renders.push(`${mod.name}/${cls.name}`);
      const resolveBare = (from: Unit, name: string, closures: Map<string, Fn>, ns?: string) => {
        if (ns) {
          const target = unitFor(mod, from, from.imports.get(ns)!.from);
          const fn = target?.fns.get(name);
          return target && fn ? { fn, unit: target } : undefined;
        }
        const closure = closures.get(name);
        if (closure) return { fn: closure, unit: from };
        const local = from.fns.get(name);
        if (local) return { fn: local, unit: from };
        const imp = from.imports.get(name);
        const target = imp && unitFor(mod, from, imp.from);
        const fn = target?.fns.get(imp!.name === "default" ? target.defaultName ?? "" : imp!.name);
        return target && fn ? { fn, unit: target } : undefined;
      };
      const ctx: Ctx = { resolveThis: (name) => methods.get(name), resolveBare };
      const scopes = new Map<string, Scope>();
      const flowing = new Set<string>();
      const keyOf = (u: Unit, fn: Fn) => `${u.file}:${fn.owner}:${fn.name}:${fn.from}`;
      const invokes = (target: { fn: Fn; unit: Unit }, k: number): boolean => {
        const param = target.fn.params[k];
        if (!param) return false;
        const key = `${keyOf(target.unit, target.fn)}#${k}`;
        if (flowing.has(key)) return false;
        flowing.add(key);
        try {
          const scope = scope_(target.unit, target.fn);
          return scope.events.some((e) => e.kind === "call" && e.name === param) || scope.events.some((e) => e.ids.includes(param) && (() => {
            const next = e.kind === "this" ? ctx.resolveThis(e.name) : e.kind === "call" ? resolveBare(target.unit, e.name, scope.closures, e.ns) : undefined;
            return Boolean(next) && invokes(next!, e.ids.indexOf(param));
          })());
        } finally {
          flowing.delete(key);
        }
      };
      const scope_ = (u: Unit, fn: Fn): Scope => {
        const key = keyOf(u, fn);
        let scope = scopes.get(key);
        if (!scope) { scope = scopeOf(u, fn, ctx, invokes); scopes.set(key, scope); }
        return scope;
      };
      type Node = { fn: Fn; unit: Unit; chain: string[]; closures: Map<string, Fn> };
      const render = methods.get("render")!;
      const seen = new Set<string>();
      const queue: Node[] = [{ fn: render.fn, unit: render.unit, chain: [label(render.fn)], closures: new Map() }];
      while (queue.length) {
        const node = queue.shift()!;
        const key = keyOf(node.unit, node.fn);
        if (seen.has(key)) continue;
        seen.add(key);
        const scope = scope_(node.unit, node.fn);
        for (const u of scope.unclassified) unclassified.add(`${mod.name}: ${u}`);
        const inScope = new Map([...node.closures, ...scope.closures]);
        for (const e of scope.events) {
          if (e.kind === "effect") {
            const id = `${mod.name}/${label(node.fn)}:${e.name}`;
            if (!found.has(id)) found.set(id, { mod: mod.name, file: node.unit.file, where: label(node.fn), effect: e.effect!, detail: e.name, chain: [...node.chain, e.name], line: lineOf(node.unit, e.at) });
            continue;
          }
          const parent = e.kind === "super" ? parentOf(mod, node.unit, node.fn) : null;
          const next = e.kind === "this" ? methods.get(e.name) : parent ? methodsOf(mod, parent.unit, parent.cls).get(e.name) : e.kind === "call" ? resolveBare(node.unit, e.name, inScope, e.ns) : undefined;
          const imp = e.kind === "call" && !e.ns ? node.unit.imports.get(e.name) : undefined;
          if (imp && (imp.from.startsWith("yuzora:") || imp.from === "herdr")) host.add(`${imp.from}.${imp.name}`);
          if (imp?.from.startsWith("yuzora:") && !HOST_READS.has(`${imp.from}.${imp.name}`)) {
            const id = `${mod.name}/${label(node.fn)}:${imp.name}`;
            if (!found.has(id)) found.set(id, { mod: mod.name, file: node.unit.file, where: label(node.fn), effect: "host", detail: imp.name, chain: [...node.chain, imp.name], line: lineOf(node.unit, e.at) });
          }
          if (next) queue.push({ ...next, chain: [...node.chain, label(next.fn)], closures: inScope });
        }
      }
    }
  }
  return { findings: [...found.values()], unclassified: [...unclassified].sort(), renders, host: [...host] };
}

/** The mods that are Yuzora's: `defaults/mods/yuzora-*`, and the frame mods that carry Yuzora's own code. */
export function yuzoraMods(root: string): { name: string; dir: string }[] {
  const mods = readdirSync(join(root, "defaults/mods")).filter((n) => n.startsWith("yuzora-")).map((n) => ({ name: n, dir: join(root, "defaults/mods", n) }));
  const frame = readdirSync(join(root, "defaults/frame")).map((n) => ({ name: `frame/${n}`, dir: join(root, "defaults/frame", n) }));
  return [...mods, ...frame].filter((m) => existsSync(join(m.dir, "main.js")) && statSync(m.dir).isDirectory());
}

export function scanAll(root: string): Report {
  const all: Report = { findings: [], unclassified: [], renders: [], host: [] };
  for (const m of yuzoraMods(root)) {
    const r = scanMod(loadMod(m.name, m.dir));
    all.findings.push(...r.findings);
    all.unclassified.push(...r.unclassified);
    all.renders.push(...r.renders);
    all.host.push(...r.host);
  }
  return all;
}

if (import.meta.main) {
  const root = resolve(dirname(new URL(import.meta.url).pathname), "..");
  const report = scanAll(root);
  for (const f of report.findings) console.log(`${f.mod}/${f.where}:${f.detail} [${f.effect}] line ${f.line}\n    ${f.chain.join(" > ")}`);
  if (process.argv.includes("--host")) console.log([...new Set(report.host)].sort().join("\n"));
  console.log(`\n${report.renders.length} renders, ${report.findings.length} findings`);
  if (report.unclassified.length) console.log(`closures to a callee this does not know:\n  ${report.unclassified.join("\n  ")}`);
}
