import { afterEach, expect, it, vi } from "vitest"
import { EditorState, type ChangeSpec } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { history, isolateHistory, redo, undo } from "@codemirror/commands"
import { applyResolution, conflictMarkers } from "./conflictMarkers"

const processApi = (globalThis as unknown as {
  process?: {
    env: Record<string, string | undefined>
    cpuUsage(): { user: number; system: number }
    memoryUsage(): { rss: number; heapUsed: number }
  }
}).process
const probe = processApi?.env.YUZORA_CONFLICT_PROBE === "1"
const benchmark = processApi?.env.YUZORA_CONFLICT_BENCH === "1"
const control = processApi?.env.YUZORA_CONFLICT_CONTROL
const role = processApi?.env.YUZORA_CONFLICT_ROLE ?? "candidate"
const conflict = "<<<<<<< HEAD\nours text\n=======\ntheirs text\n>>>>>>> branch\n"

afterEach(() => vi.restoreAllMocks())

function decorate(text: string) {
  return EditorState.create({ doc: text, extensions: [conflictMarkers()] })
}

function decorations(state: EditorState) {
  const entries: { from: number; to: number; className?: string; index?: number }[] = []
  for (const set of state.facet(EditorView.decorations)) {
    if (typeof set === "function") throw new Error("Expected a state-provided decoration set")
    set.between(0, state.doc.length, (from, to, value) => {
      entries.push({ from, to, className: value.spec.class, index: value.spec.widget?.index })
    })
  }
  return entries
}

function expectFreshDecorations(state: EditorState) {
  expect(decorations(state)).toEqual(decorations(decorate(state.doc.toString())))
}

const variedDocument = "prefix 中文🙂\n".repeat(8) + conflict +
  "middle ordinary\n".repeat(8) +
  "<<<<<<< second\nsecond ours\n||||||| base\nold base\n=======\nsecond theirs\n>>>>>>> other\n" +
  "suffix ordinary\n".repeat(8)

it("matches full reconstruction for edits at every line boundary and interior", () => {
  const initial = decorate(variedDocument)
  for (let number = 1; number <= initial.doc.lines; number++) {
    const line = initial.doc.line(number)
    const edits: ChangeSpec[] = [
      { from: line.from, insert: "x" },
      { from: line.to, insert: "漢🙂" },
      { from: Math.floor((line.from + line.to) / 2), insert: " text " },
      { from: line.from, to: Math.min(line.to, line.from + 1) },
      { from: line.from, to: line.to, insert: "replacement" },
      { from: line.from, to: line.to },
      { from: line.from, insert: "\n" }
    ]
    if (line.to < initial.doc.length) edits.push({ from: line.to, to: line.to + 1 })
    for (const changes of edits) expectFreshDecorations(initial.update({ changes }).state)
  }
})

it("matches full reconstruction for deterministic mixed and multi-range edits", () => {
  let state = decorate(variedDocument), seed = 157
  const next = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed }
  const inserts = ["x", "", "漢🙂", "\n", "<<<<<<< new", "=======", "||||||| base", ">>>>>>> end", "\n" + conflict]
  for (let step = 0; step < 400; step++) {
    const line = state.doc.line(1 + next() % state.doc.lines)
    const from = line.from + next() % (line.length + 1)
    const to = Math.min(line.to, from + next() % 4)
    state = state.update({ changes: { from, to, insert: inserts[next() % inserts.length] } }).state
    expectFreshDecorations(state)
    if (step % 20 === 0 && state.doc.lines > 2) {
      const first = state.doc.line(1), last = state.doc.line(state.doc.lines)
      state = state.update({ changes: [{ from: first.to, insert: "Q" }, { from: last.from, insert: "R" }] }).state
      expectFreshDecorations(state)
    }
  }
})

it("keeps marker labels and marker type replacements consistent with reconstruction", () => {
  const state = decorate(variedDocument)
  for (let number = 1; number <= state.doc.lines; number++) {
    const line = state.doc.line(number)
    if (!/^(<<<<<<<|=======|>>>>>>>|\|\|\|\|\|\|\|)/.test(line.text)) continue
    for (const insert of [line.text, line.text.slice(0, 7) + " different label", "<<<<<<< replaced", "=======", ">>>>>>> replaced", "||||||| replaced", "ordinary text"]) {
      expectFreshDecorations(state.update({ changes: { from: line.from, to: line.to, insert } }).state)
    }
  }
})

it("keeps mapped ranges correct through undo, redo, and current-block resolution", () => {
  let state = EditorState.create({ doc: variedDocument, extensions: [conflictMarkers(), history()] })
  for (const changes of [
    { from: 3, insert: "before" },
    { from: variedDocument.indexOf("ours text") + 4 + 6, insert: " current " },
    { from: 0, insert: "\n" }
  ]) {
    state = state.update({ changes, annotations: isolateHistory.of("full") }).state
    expectFreshDecorations(state)
  }
  for (let i = 0; i < 3; i++) {
    expect(undo({ state, dispatch: transaction => { state = transaction.state } })).toBe(true)
    expectFreshDecorations(state)
  }
  expect(state.doc.toString()).toBe(variedDocument)
  for (let i = 0; i < 3; i++) {
    expect(redo({ state, dispatch: transaction => { state = transaction.state } })).toBe(true)
    expectFreshDecorations(state)
  }
  const view = new EditorView({ state })
  try {
    view.dispatch({ changes: { from: view.state.doc.toString().indexOf("second ours") + 7, insert: "updated " } })
    applyResolution(view, 1, "current")
    expect(view.state.doc.toString()).toContain("second updated ours")
    expect(view.state.doc.toString()).not.toContain("second theirs")
    expectFreshDecorations(view.state)
  } finally { view.destroy() }
})

it.runIf(probe)("counts full-document flattening during ordinary conflict edits", () => {
  for (const where of ["before", "inside", "after", "marker"] as const) {
    let state = decorate("ordinary text\n".repeat(30_000) + conflict + "trailing text\n".repeat(30_000))
    const index = where === "before" ? 5 : where === "inside" ? 30_000 * "ordinary text\n".length + "<<<<<<< HEAD\n".length + 4
      : where === "marker" ? 30_000 * "ordinary text\n".length + 8 : state.doc.length - 5
    const proto = Object.getPrototypeOf(state.doc)
    const original = proto.toString
    let calls = 0, codeUnits = 0
    const spy = vi.spyOn(proto, "toString").mockImplementation(function (this: { length: number }) {
      calls += 1; codeUnits += this.length
      return Reflect.apply(original, this, [])
    })
    try {
      for (let i = 0; i < 100; i++) state = state.update({ changes: { from: index, insert: "x" } }).state
    } finally { spy.mockRestore() }
    expect(decorations(state)).toEqual(decorations(decorate(state.doc.toString())))
    console.log("CONFLICT_FLATTEN", JSON.stringify({ where, updates: 100, calls, codeUnits, characters: state.doc.length }))
  }
})

type Scenario = "clear" | "before" | "inside" | "after" | "mixed" | "marker" | "marker-prefix" | "many" | "selection"
const scenarios: Scenario[] = ["clear", "before", "inside", "after", "mixed", "marker", "marker-prefix", "many", "selection"]
function workload(targetCharacters: number, scenario: Scenario) {
  const line = "ordinary source text\n", blocks = scenario === "many" ? 16 : 1
  const gap = line.repeat(Math.max(2, Math.floor((targetCharacters - blocks * conflict.length) / ((blocks + 1) * line.length))))
  const text = scenario === "clear" ? line.repeat(Math.ceil(targetCharacters / line.length))
    : gap + Array.from({ length: blocks }, () => conflict + gap).join("")
  const position = scenario === "inside" || scenario === "mixed" ? text.indexOf("ours text") + 4
    : scenario === "marker" ? text.indexOf("<<<<<<<") + 8
      : scenario === "marker-prefix" ? text.indexOf("<<<<<<<")
        : scenario === "after" ? text.length - 5 : 5
  return { text, position }
}

function edit(state: EditorState, scenario: Scenario, position: number, index: number) {
  if (scenario === "selection") return state.update({ selection: { anchor: (index * 11) % state.doc.length } }).state
  const newline = scenario === "mixed" && index % 10 < 2
  return state.update({ changes: index % 2 === 0
    ? { from: position, insert: newline ? "\n" : "x" }
    : { from: position, to: position + 1 }
  }).state
}

it.runIf(benchmark)("measures conflict transaction costs and structural controls", () => {
  const cpuMs = () => { const time = processApi!.cpuUsage(); return (time.user + time.system) / 1000 }
  for (const targetCharacters of [4096, 131072, 1048576]) for (const scenario of scenarios) {
    const { text, position } = workload(targetCharacters, scenario)
    let state = decorate(text)
    for (let batch = 0; batch < 10; batch++) {
      const cpu = cpuMs(), latency: number[] = []
      for (let index = 0; index < 50; index++) {
        const started = performance.now()
        state = edit(state, scenario, position, index)
        latency.push(performance.now() - started)
      }
      const parentCpuMs = cpuMs() - cpu
      expect(state.doc.toString()).toBe(text)
      expectFreshDecorations(state)
      console.log("CONFLICT_BENCH", JSON.stringify({ role, scenario, targetCharacters, characters: text.length, batch, warmup: batch < 3, iterations: 50, parentCpuMs, latency, decorationCount: decorations(state).length }))
    }
  }
})

it.runIf(benchmark)("checks repeated conflict view update and destruction lifecycle", () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"] })
  const host = document.createElement("div")
  document.body.appendChild(host)
  const text = conflict + "ordinary source text\n".repeat(3000)
  const position = "<<<<<<< HEAD\n".length + 4
  const timerBaseline = vi.getTimerCount()
  const resources: { cycle: number; timers: number; domChildren: number; rss: number; heapUsed: number }[] = []
  try {
    for (let cycle = 0; cycle < 110; cycle++) {
      const view = new EditorView({ state: decorate(text), parent: host })
      try {
        expect(view.dom.querySelector(".cm-conflict-actions")).not.toBeNull()
        for (let i = 0; i < 8; i++) view.dispatch({ changes: i % 2 === 0 ? { from: position, insert: "x" } : { from: position, to: position + 1 } })
        expectFreshDecorations(view.state)
      } finally { view.destroy() }
      expect(host.childElementCount).toBe(0)
      expect(vi.getTimerCount()).toBe(timerBaseline)
      if (cycle >= 10 && (cycle + 1) % 10 === 0) {
        const memory = processApi!.memoryUsage()
        resources.push({ cycle: cycle - 9, timers: vi.getTimerCount() - timerBaseline, domChildren: host.childElementCount, rss: memory.rss, heapUsed: memory.heapUsed })
      }
    }
    console.log("CONFLICT_LIFECYCLE", JSON.stringify({ role, warmup: 10, cycles: 100, resources }))
  } finally {
    host.remove()
    vi.useRealTimers()
  }
})

it.runIf(Boolean(control))("measures an isolated conflict control after extended warmup", () => {
  const [size, name] = control!.split("/")
  const targetCharacters = Number(size), scenario = name as Scenario
  expect([4096, 131072, 1048576]).toContain(targetCharacters)
  expect(scenarios).toContain(scenario)
  const { text, position } = workload(targetCharacters, scenario)
  let state = decorate(text)
  const iterations = targetCharacters === 4096 || scenario === "clear" || scenario === "selection" ? 1000 : 100
  const cpuMs = () => { const time = processApi!.cpuUsage(); return (time.user + time.system) / 1000 }
  for (let batch = 0; batch < 12; batch++) {
    const cpu = cpuMs(), latency: number[] = []
    for (let index = 0; index < iterations; index++) {
      const started = performance.now()
      state = edit(state, scenario, position, index)
      latency.push(performance.now() - started)
    }
    const parentCpuMs = cpuMs() - cpu
    expect(state.doc.toString()).toBe(text)
    expectFreshDecorations(state)
    console.log("CONFLICT_CONTROL", JSON.stringify({ role, scenario, targetCharacters, batch, warmup: batch < 5, iterations, parentCpuMs, latency }))
  }
})
