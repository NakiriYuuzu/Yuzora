import { act, cleanup, render } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { useStore } from "zustand"
import { herdrPagePath } from "@/lib/herdrPages"
import type { HerdrSnapshot, HerdrTabInfo } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "./herdrStore"
import { useWorkspaceStore } from "./workspaceStore"

vi.mock("@tauri-apps/api/core", async (original) => ({
  ...await original<typeof import("@tauri-apps/api/core")>(),
  invoke: vi.fn(async () => { throw new Error("Unexpected native IPC in snapshot unit fixture") })
}))

const initialWorkspace = useWorkspaceStore.getState()
const processApi = (globalThis as unknown as {
  process?: {
    env: Record<string, string | undefined>
    cpuUsage(): { user: number; system: number }
  }
}).process
const probe = processApi?.env.YUZORA_HERDR_GROUP_PROBE === "1"
const benchmark = processApi?.env.YUZORA_HERDR_GROUP_BENCH === "1"
const role = processApi?.env.YUZORA_HERDR_GROUP_ROLE ?? "candidate"
const extended = processApi?.env.YUZORA_HERDR_GROUP_EXTENDED === "1"

afterEach(() => {
  cleanup()
  useWorkspaceStore.setState(initialWorkspace, true)
  useHerdrStore.setState(herdrInitialState)
})

function setupGroups(tabCount: number, rightSession = "work") {
  const groups = ["default", rightSession].map((session, group) => ({
    activePath: `/fixture/${group}/keep.ts`,
    tabs: [
      { path: `/fixture/${group}/keep.ts`, name: "keep.ts", dirty: false, externallyModified: false },
      ...Array.from({ length: tabCount }, (_, i) => ({
        path: herdrPagePath(session, `g${group}-t${i}`),
        name: `Tab ${i}`,
        kind: "herdr-terminal" as const,
        dirty: false,
        externallyModified: false,
        herdrSessionId: session,
        herdrWorkspaceId: `ws-${group}`,
        herdrTabId: `g${group}-t${i}`
      }))
    ]
  }))
  useWorkspaceStore.setState({ groups, activeGroupIndex: 0 })
  useHerdrStore.setState({ ...herdrInitialState, selectedSessionName: "default" })
  return groups
}

function snapshot(tabCount: number, reverse: boolean): HerdrSnapshot {
  const tabs: HerdrTabInfo[] = Array.from({ length: tabCount }, (_, position) => {
    const i = reverse ? tabCount - position - 1 : position
    return { id: `g0-t${i}`, label: `Tab ${i}`, order: position, workspaceId: "ws-0", paneCount: 1, status: "idle", active: i === 0, focused: i === 0 }
  })
  return {
    herdrSessionId: "default", protocol: 19, version: "0.8.0",
    spaces: [{ id: "ws-0", label: "Left", order: 0, focused: true, path: "/fixture/0" }],
    tabs, agents: [], terminals: [], raw: {}, focusedWorkspaceId: "ws-0"
  }
}

it.runIf(probe)("records group invalidations through actual snapshot application", () => {
  for (const tabCount of [8, 64, 256]) {
    const groups = setupGroups(tabCount)
    useHerdrStore.getState().applySnapshot("default", snapshot(tabCount, false))
    const renders = [0, 0], invalidations = [0, 0]
    const recordRender = (index: number) => { renders[index] += 1 }
    function GroupWitness({ index, onRender }: { index: number; onRender(index: number): void }) {
      const group = useWorkspaceStore(state => state.groups[index])
      onRender(index)
      return <span data-testid={`group-${index}`}>{group.tabs.map(tab => tab.name).join("|")}</span>
    }
    const unlisten = useWorkspaceStore.subscribe((state, previous) => {
      for (let i = 0; i < 2; i++) if (state.groups[i] !== previous.groups[i]) invalidations[i] += 1
    })
    const view = render(<><GroupWitness index={0} onRender={recordRender} /><GroupWitness index={1} onRender={recordRender} /></>)
    renders.fill(0)
    for (let cycle = 0; cycle < 100; cycle++) {
      act(() => useHerdrStore.getState().applySnapshot("default", snapshot(tabCount, cycle % 2 === 0)))
      expect(useWorkspaceStore.getState().groups[1].tabs).toEqual(groups[1].tabs)
      expect(useWorkspaceStore.getState().groups.map(group => group.activePath)).toEqual(groups.map(group => group.activePath))
    }
    console.log("HERDR_GROUP_PROBE", JSON.stringify({ tabCount, updates: 100, invalidations, renders }))
    view.unmount()
    unlisten()
  }
})

type Scenario = "left-foreign" | "left-hidden" | "right" | "both" | "unchanged"
type WorkspaceGroup = typeof initialWorkspace.groups[number]

function prepareCase(tabCount: number, scenario: Scenario) {
  const sessions = scenario === "right" ? ["work", "default"]
    : scenario === "both" || scenario === "left-hidden" ? ["default", "default"]
      : ["default", "work"]
  const groups: WorkspaceGroup[] = sessions.map((session, group) => {
    const tabs: WorkspaceGroup["tabs"] = [{ path: `/fixture/${group}/keep.ts`, name: "keep.ts", dirty: true, externallyModified: false }]
    for (let i = 0; i < tabCount; i++) {
      tabs.push({ path: herdrPagePath(session, `g${group}-t${i}`), name: `Tab 中文 ${i}`, kind: "herdr-terminal", dirty: false, externallyModified: false, herdrSessionId: session, herdrWorkspaceId: `ws-${group}`, herdrTabId: `g${group}-t${i}` })
      if (i === 0) {
        tabs.push({ path: herdrPagePath(session, `hidden-${group}`), name: "Hidden Space", kind: "herdr-terminal", dirty: false, externallyModified: false, herdrSessionId: session, herdrWorkspaceId: `hidden-${group}`, herdrTabId: `hidden-${group}` })
        tabs.push({ path: `/fixture/${group}/middle.ts`, name: "middle.ts", dirty: false, externallyModified: true })
      }
    }
    return { tabs, activePath: tabs[0].path }
  })
  const targets = scenario === "right" ? [1] : scenario === "both" ? [0, 1] : [0]
  const makeSnapshot = (reverse: boolean): HerdrSnapshot => ({
    herdrSessionId: "default", protocol: 19, version: "0.8.0", agents: [], terminals: [], raw: {},
    focusedWorkspaceId: `ws-${targets[0]}`,
    spaces: targets.map(group => ({ id: `ws-${group}`, label: `Space ${group}`, order: group, focused: group === targets[0], path: `/fixture/${group}` })),
    tabs: targets.flatMap(group => Array.from({ length: tabCount }, (_, position) => {
      const i = reverse ? tabCount - position - 1 : position
      return { id: `g${group}-t${i}`, label: `Tab 中文 ${i}`, order: position, workspaceId: `ws-${group}`, paneCount: 1, status: "idle" as const, active: i === 0, focused: i === 0 && group === targets[0] }
    }))
  })
  useWorkspaceStore.setState({ groups, activeGroupIndex: 0 })
  useHerdrStore.setState({ ...herdrInitialState, selectedSessionName: "default" })
  const forward = makeSnapshot(false), backward = makeSnapshot(true)
  useHerdrStore.getState().applySnapshot("default", forward)
  return { groups, targets, forward, backward }
}

function GroupRenderWitness({ index, onRender }: { index: number; onRender(index: number): void }) {
  // The same group selector used by the actual TabBar.
  const group = useWorkspaceStore(state => state.groups[index])
  onRender(index)
  return <span data-testid={`group-${index}`}>{group.tabs.map(tab => tab.name).join("|")}</span>
}

it("preserves the unaffected split, file slots, hidden Spaces and active selection", () => {
  for (const scenario of ["left-foreign", "left-hidden", "right", "both"] as const) {
    const { groups, targets, backward } = prepareCase(8, scenario)
    useHerdrStore.getState().applySnapshot("default", backward)
    const after = useWorkspaceStore.getState().groups
    for (let group = 0; group < 2; group++) {
      if (!targets.includes(group)) {
        expect(after[group]).toBe(groups[group])
        continue
      }
      expect(after[group]).not.toBe(groups[group])
      expect(after[group].activePath).toBe(groups[group].activePath)
      for (const index of [0, 2, 3]) expect(after[group].tabs[index]).toBe(groups[group].tabs[index])
      expect(after[group].tabs.filter(tab => tab.herdrWorkspaceId === `ws-${group}`).map(tab => tab.herdrTabId))
        .toEqual(Array.from({ length: 8 }, (_, i) => `g${group}-t${7 - i}`))
    }
  }
})

it("still repairs local tab order when the incoming HERDR snapshot is unchanged", () => {
  const { backward } = prepareCase(8, "left-foreign")
  useHerdrStore.getState().applySnapshot("default", backward)
  const runtime = useHerdrStore.getState().runtimesBySession.default
  // A local order change must still be reconciled against recovery truth.
  useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot(snapshot(8, false))
  const before = useWorkspaceStore.getState()
  useHerdrStore.getState().applySnapshot("default", structuredClone(backward))
  expect(useHerdrStore.getState().runtimesBySession.default).toBe(runtime)
  expect(useWorkspaceStore.getState()).not.toBe(before)
  expect(useWorkspaceStore.getState().groups[1]).toBe(before.groups[1])
  expect(useWorkspaceStore.getState().groups[0].tabs[1].herdrTabId).toBe("g0-t7")
})

it("keeps identical snapshots quiet in both stores", () => {
  const { forward } = prepareCase(8, "unchanged")
  const workspace = useWorkspaceStore.getState(), herdr = useHerdrStore.getState()
  const workspaceListener = vi.fn(), herdrListener = vi.fn()
  const a = useWorkspaceStore.subscribe(workspaceListener), b = useHerdrStore.subscribe(herdrListener)
  try {
    useHerdrStore.getState().applySnapshot("default", structuredClone(forward))
    expect(useWorkspaceStore.getState()).toBe(workspace)
    expect(useHerdrStore.getState()).toBe(herdr)
    expect(workspaceListener).not.toHaveBeenCalled()
    expect(herdrListener).not.toHaveBeenCalled()
  } finally { a(); b() }
})

it.runIf(benchmark)("measures snapshot reconciliation and selector renders", () => {
  const cpuMs = () => { const t = processApi!.cpuUsage(); return (t.user + t.system) / 1000 }
  const iterations = extended ? 400 : 100
  const warmupBatches = extended ? 5 : 2
  for (const tabCount of [8, 64, 256]) for (const scenario of ["left-foreign", "left-hidden", "right", "both", "unchanged"] as const) {
    const { groups, forward, backward } = prepareCase(tabCount, scenario)
    const renders = [0, 0], invalidations = [0, 0]
    const recordRender = (index: number) => { renders[index] += 1 }
    let notifications = 0
    const stop = useWorkspaceStore.subscribe((state, previous) => {
      notifications += 1
      for (let i = 0; i < 2; i++) if (state.groups[i] !== previous.groups[i]) invalidations[i] += 1
    })
    const view = render(<><GroupRenderWitness index={0} onRender={recordRender} /><GroupRenderWitness index={1} onRender={recordRender} /></>)
    try {
      for (let batch = 0; batch < warmupBatches + 7; batch++) {
        renders.fill(0); invalidations.fill(0); notifications = 0
        const latency: number[] = [], cpu = cpuMs()
        for (let i = 0; i < iterations; i++) {
          const next = structuredClone(scenario === "unchanged" || i % 2 === 1 ? forward : backward)
          const started = performance.now()
          act(() => useHerdrStore.getState().applySnapshot("default", next))
          latency.push(performance.now() - started)
        }
        const parentCpuMs = cpuMs() - cpu
        expect(useWorkspaceStore.getState().groups.map(g => g.tabs)).toEqual(groups.map(g => g.tabs))
        expect(useWorkspaceStore.getState().groups.map(g => g.activePath)).toEqual(groups.map(g => g.activePath))
        const expected = scenario === "unchanged" ? [0, 0] : scenario === "right" ? [0, iterations]
          : scenario === "both" || role === "baseline" ? [iterations, iterations] : [iterations, 0]
        expect(invalidations).toEqual(expected)
        expect(renders).toEqual(expected)
        expect(notifications).toBe(scenario === "unchanged" ? 0 : iterations)
        console.log("HERDR_GROUP_BENCH", JSON.stringify({ role, tabCount, scenario, batch, warmup: batch < warmupBatches, iterations, parentCpuMs, latency, invalidations, renders, notifications }))
      }
    } finally { view.unmount(); stop() }
  }
})

it.runIf(benchmark)("measures repeated selector mount and unmount lifecycle", () => {
  for (const tabCount of [8, 64, 256]) {
    const { forward, backward } = prepareCase(tabCount, "left-foreign")
    const originalSubscribe = useWorkspaceStore.subscribe
    let active = 0, peak = 0
    // The bound hook closes over its original API; replacing the copied
    // .subscribe property would miss it. Observe real subscriptions explicitly.
    const observedStore = {
      getState: useWorkspaceStore.getState,
      getInitialState: useWorkspaceStore.getInitialState,
      subscribe(listener: Parameters<typeof originalSubscribe>[0]) {
        active += 1; peak = Math.max(peak, active)
        const stop = originalSubscribe(listener)
        let closed = false
        return () => { if (!closed) { closed = true; active -= 1; stop() } }
      }
    }
    const renders = [0, 0], measured = [0, 0]
    const recordRender = (index: number) => { renders[index] += 1 }
    function ObservedGroup({ index, onRender }: { index: number; onRender(index: number): void }) {
      const group = useStore(observedStore, state => state.groups[index])
      onRender(index)
      return <span>{group.tabs.map(tab => tab.name).join("|")}</span>
    }
    try {
      for (let cycle = 0; cycle < 110; cycle++) {
        const view = render(<><ObservedGroup index={0} onRender={recordRender} /><ObservedGroup index={1} onRender={recordRender} /></>)
        renders.fill(0)
        act(() => useHerdrStore.getState().applySnapshot("default", structuredClone(cycle % 2 === 0 ? backward : forward)))
        if (cycle >= 10) for (let i = 0; i < 2; i++) measured[i] += renders[i]
        view.unmount()
        expect(active).toBe(0)
      }
      expect(measured).toEqual(role === "baseline" ? [100, 100] : [100, 0])
      expect(peak).toBe(2)
      console.log("HERDR_GROUP_LIFECYCLE", JSON.stringify({ role, tabCount, warmup: 10, cycles: 100, updateRenders: measured, activeSubscriptions: active, peakSubscriptions: peak }))
    } finally { cleanup() }
  }
})
