import { afterEach, expect, it, vi } from "vitest"
import type { HerdrSnapshot, HerdrWorktreeInventory } from "@/lib/herdrTypes"
import { buildWorktreeInventory } from "@/lib/herdrWorktree"
import { herdrPagePath } from "@/lib/herdrPages"
import { herdrInitialState, useHerdrStore } from "./herdrStore"
import { useWorkspaceStore } from "./workspaceStore"

vi.mock("@tauri-apps/api/core", async original => ({
  ...await original<typeof import("@tauri-apps/api/core")>(),
  invoke: vi.fn(async () => { throw new Error("Unexpected native IPC in scope fixture") })
}))

const initialWorkspace = useWorkspaceStore.getState()
const processApi = (globalThis as unknown as {
  process?: { env: Record<string, string | undefined>; cpuUsage(): { user: number; system: number } }
}).process
const probe = processApi?.env.YUZORA_SCOPE_PROBE === "1"
const benchmark = processApi?.env.YUZORA_SCOPE_BENCH === "1"
const role = processApi?.env.YUZORA_SCOPE_ROLE ?? "candidate"

afterEach(() => {
  useHerdrStore.setState(herdrInitialState)
  useWorkspaceStore.setState(initialWorkspace, true)
})

function makeSnapshot(count: number, scope = "default"): HerdrSnapshot {
  return {
    herdrSessionId: scope, protocol: 19, version: "0.8.0", raw: {},
    focusedWorkspaceId: `${scope}-w0`, focusedTabId: `${scope}-t0a`, focusedPaneId: `${scope}-p0`,
    spaces: Array.from({ length: count }, (_, i) => ({
      id: `${scope}-w${i}`, label: `Workspace 中文 ${i}`, order: i, focused: i === 0,
      path: `/fixture/${scope}/worktree-${i}`, repoKey: `${scope}-repo-${i}`,
      repoRoot: `/fixture/${scope}/repo-${i}`, isLinkedWorktree: i % 2 === 1
    })),
    agents: Array.from({ length: count }, (_, i) => ({
      id: `${scope}-a${i}`, name: "Agent", workspaceId: `${scope}-w${i}`,
      tabId: `${scope}-t${i}a`, paneId: `${scope}-p${i}`, status: "blocked", title: "Input needed"
    })),
    tabs: Array.from({ length: count }, (_, i) => ["a", "b"].map((part, order) => ({
      id: `${scope}-t${i}${part}`, label: `Tab ${part}`, order, workspaceId: `${scope}-w${i}`,
      paneCount: 1, status: "blocked" as const, active: order === 0, focused: i === 0 && order === 0
    }))).flat(),
    terminals: []
  }
}

function inventoryFor(snapshot: HerdrSnapshot) {
  return buildWorktreeInventory(snapshot.herdrSessionId, snapshot.spaces.map(space => ({
    source: { repoKey: space.repoKey!, repoName: "fixture", repoRoot: space.repoRoot!, sourceCheckoutPath: space.repoRoot!, sourceWorkspaceId: space.id },
    worktrees: [{ path: space.path!, branch: "main", isBare: false, isDetached: false, isPrunable: false, isLinkedWorktree: space.isLinkedWorktree!, label: space.label, openWorkspaceId: space.id }]
  })))
}

function seedInventory(scope: string, inventory: HerdrWorktreeInventory) {
  useHerdrStore.setState(state => ({ runtimesBySession: {
    ...state.runtimesBySession,
    [scope]: { ...state.runtimesBySession[scope], worktreeInventory: inventory }
  } }))
}

function setup(count: number, loaded: boolean, sessionCount = 1) {
  useHerdrStore.setState({ ...herdrInitialState, selectedSessionName: "default" })
  useWorkspaceStore.setState(initialWorkspace, true)
  const snapshots = Array.from({ length: sessionCount }, (_, i) => makeSnapshot(count, i === 0 ? "default" : `work-${i}`))
  for (const snapshot of snapshots) {
    useHerdrStore.getState().applySnapshot(snapshot.herdrSessionId, structuredClone(snapshot))
    if (loaded) {
      seedInventory(snapshot.herdrSessionId, inventoryFor(snapshot))
      useHerdrStore.getState().applySnapshot(snapshot.herdrSessionId, structuredClone(snapshot))
    }
    useHerdrStore.getState().setEventsHealth(snapshot.herdrSessionId, true, `sub-${snapshot.herdrSessionId}`)
  }
  useWorkspaceStore.setState({ groups: snapshots.slice(0, 2).map(snapshot => ({
    activePath: herdrPagePath(snapshot.herdrSessionId, snapshot.tabs[0].id),
    tabs: snapshot.tabs.slice(0, 2).map(tab => ({
      path: herdrPagePath(snapshot.herdrSessionId, tab.id), name: tab.label, dirty: false, externallyModified: false,
      kind: "herdr-terminal" as const, herdrSessionId: snapshot.herdrSessionId, herdrWorkspaceId: tab.workspaceId, herdrTabId: tab.id
    }))
  })) })
  return snapshots
}

it.runIf(probe)("counts topology serialization separately from timing", () => {
  for (const loaded of [false, true]) for (const changed of [false, true]) {
    const [snapshot] = setup(8, loaded)
    const stringify = JSON.stringify
    let calls = 0, codeUnits = 0
    const spy = vi.spyOn(JSON, "stringify").mockImplementation((...args) => {
      const result = Reflect.apply(stringify, JSON, args)
      const value = args[0]
      if (Array.isArray(value) && value.length === 8 && value.every(row => Array.isArray(row) && row.length === 5)) {
        calls += 1; codeUnits += result.length
      }
      return result
    })
    try {
      for (let i = 0; i < 100; i++) {
        const next = structuredClone(snapshot)
        if (changed) next.agents[0].status = i % 2 === 0 ? "working" : "blocked"
        useHerdrStore.getState().applySnapshot("default", next)
      }
    } finally { spy.mockRestore() }
    console.log("HERDR_SCOPE_COUNTS", JSON.stringify({ loaded, changed, updates: 100, calls, codeUnits }))
  }
})

it("keeps first snapshots valid without inventory and retains inventory on status changes", () => {
  const [snapshot] = setup(8, false)
  expect(useHerdrStore.getState().runtimesBySession.default.worktreeInventory).toBeNull()
  seedInventory("default", inventoryFor(snapshot))
  useHerdrStore.getState().applySnapshot("default", structuredClone(snapshot))
  const inventory = useHerdrStore.getState().runtimesBySession.default.worktreeInventory
  const next = structuredClone(snapshot)
  next.agents[0].status = "working"
  useHerdrStore.getState().applySnapshot("default", next)
  const runtime = useHerdrStore.getState().runtimesBySession.default
  expect(runtime.worktreeInventory).toBe(inventory)
  expect(runtime.snapshot?.spaces[0].branch).toBe("main")
  expect(runtime.snapshot?.agents[0].status).toBe("working")
})

it.each(["id", "path", "repoKey", "repoRoot", "isLinkedWorktree"] as const)("invalidates inventory when topology field %s changes", field => {
  const [snapshot] = setup(8, true)
  const next = structuredClone(snapshot)
  if (field === "isLinkedWorktree") next.spaces[0][field] = !next.spaces[0][field]
  else next.spaces[0][field] = `${next.spaces[0][field]}-changed`
  if (field === "id") {
    next.focusedWorkspaceId = next.spaces[0].id
    next.agents[0].workspaceId = next.spaces[0].id
    for (const tab of next.tabs.slice(0, 2)) tab.workspaceId = next.spaces[0].id
  }
  useHerdrStore.getState().applySnapshot("default", next)
  const runtime = useHerdrStore.getState().runtimesBySession.default
  expect(runtime.worktreeInventory).toBeNull()
  expect(runtime.snapshot?.spaces[0].branch).toBeUndefined()
  expect(runtime.baseSnapshot?.spaces[0][field]).toEqual(next.spaces[0][field])
})

it("still repairs attention and local tab order for an identical snapshot", () => {
  const snapshots = setup(8, true, 2), current = snapshots[1]
  useHerdrStore.getState().markAttentionSeen("default", "default-p0")
  const marker = useHerdrStore.getState().attentionItems("default").find(item => item.paneId === "default-p0")
  expect(marker?.seen).toBe(true)
  const inventory = useHerdrStore.getState().runtimesBySession["work-1"].worktreeInventory
  const backwards = structuredClone(current)
  backwards.tabs = [backwards.tabs[1], backwards.tabs[0], ...backwards.tabs.slice(2)]
  useWorkspaceStore.getState().reconcileHerdrPagesFromSnapshot(backwards)
  useHerdrStore.getState().applySubscriptionEvent("work-1", { type: "pane_exited", subscriptionId: "sub-work-1", paneId: "work-1-p0", workspaceId: "work-1-w0" })
  expect(useHerdrStore.getState().attentionItems("work-1").some(item => item.paneId === "work-1-p0")).toBe(false)
  useHerdrStore.getState().applySnapshot("work-1", structuredClone(current))
  expect(useHerdrStore.getState().attentionItems("work-1").some(item => item.paneId === "work-1-p0")).toBe(true)
  expect(useHerdrStore.getState().attentionItems("default").find(item => item.paneId === "default-p0")).toBe(marker)
  expect(useHerdrStore.getState().runtimesBySession["work-1"].worktreeInventory).toBe(inventory)
  expect(useWorkspaceStore.getState().groups[1].tabs.map(tab => tab.herdrTabId)).toEqual(["work-1-t0a", "work-1-t0b"])
})

it("invalidates inventory on workspace order or count changes", () => {
  for (const change of ["order", "count"] as const) {
    const [snapshot] = setup(8, true), next = structuredClone(snapshot)
    if (change === "order") next.spaces.reverse()
    else {
      const removed = next.spaces.pop()!.id
      next.agents = next.agents.filter(agent => agent.workspaceId !== removed)
      next.tabs = next.tabs.filter(tab => tab.workspaceId !== removed)
    }
    useHerdrStore.getState().applySnapshot("default", next)
    expect(useHerdrStore.getState().runtimesBySession.default.worktreeInventory).toBeNull()
  }
})

it("retains inventory across equivalent nullish scope fields and rejects a missing base", () => {
  const [snapshot] = setup(8, true), inventory = inventoryFor(snapshot)
  const missing = structuredClone(snapshot)
  for (const key of ["path", "repoKey", "repoRoot", "isLinkedWorktree"] as const) missing.spaces[0][key] = undefined
  useHerdrStore.getState().applySnapshot("default", missing)
  seedInventory("default", inventory)
  useHerdrStore.getState().applySnapshot("default", structuredClone(missing))
  const explicit = structuredClone(missing)
  for (const key of ["path", "repoKey", "repoRoot", "isLinkedWorktree"] as const) explicit.spaces[0][key] = null
  useHerdrStore.getState().applySnapshot("default", explicit)
  expect(useHerdrStore.getState().runtimesBySession.default.worktreeInventory).toBe(inventory)
  useHerdrStore.setState(state => ({ runtimesBySession: {
    ...state.runtimesBySession,
    default: { ...state.runtimesBySession.default, baseSnapshot: undefined }
  } }))
  useHerdrStore.getState().applySnapshot("default", structuredClone(explicit))
  expect(useHerdrStore.getState().runtimesBySession.default.worktreeInventory).toBeNull()
})

type Mode = "none-equal" | "none-status" | "loaded-equal" | "loaded-status" | "loaded-topology"
const modes: Mode[] = ["none-equal", "none-status", "loaded-equal", "loaded-status", "loaded-topology"]
const cases = [1, 8, 32, 128].flatMap(spaces => modes.map(mode => ({ spaces, sessions: 1, mode })))
  .concat(modes.filter(mode => mode !== "loaded-topology").map(mode => ({ spaces: 8, sessions: 8, mode })))

it.runIf(benchmark)("measures complete snapshot application across inventory and session states", () => {
  const cpuMs = () => { const t = processApi!.cpuUsage(); return (t.user + t.system) / 1000 }
  for (const { spaces, sessions, mode } of cases) {
    const loaded = mode.startsWith("loaded")
    const snapshots = setup(spaces, loaded, sessions)
    const inventories = snapshots.map(snapshot => useHerdrStore.getState().runtimesBySession[snapshot.herdrSessionId].worktreeInventory)
    for (let batch = 0; batch < 12; batch++) {
      const cpu = cpuMs(), latency: number[] = []
      for (let i = 0; i < 256; i++) {
        const index = i % sessions, snapshot = snapshots[index], scope = snapshot.herdrSessionId
        const next = structuredClone(snapshot)
        if (mode.endsWith("status")) next.agents[0].status = Math.floor(i / sessions) % 2 === 0 ? "working" : "blocked"
        if (mode === "loaded-topology") {
          seedInventory(scope, inventoryFor(useHerdrStore.getState().runtimesBySession[scope].baseSnapshot!))
          if (i % 2 === 0) next.spaces[0].path += "/moved"
        }
        const started = performance.now()
        useHerdrStore.getState().applySnapshot(scope, next)
        latency.push(performance.now() - started)
      }
      const parentCpuMs = cpuMs() - cpu
      expect(Object.keys(useHerdrStore.getState().runtimesBySession)).toHaveLength(sessions)
      expect(useHerdrStore.getState().attentionByKey.size).toBe(spaces * sessions)
      for (let index = 0; index < sessions; index++) {
        const runtime = useHerdrStore.getState().runtimesBySession[snapshots[index].herdrSessionId]
        if (mode === "loaded-topology" || !loaded) expect(runtime.worktreeInventory).toBeNull()
        else expect(runtime.worktreeInventory).toBe(inventories[index])
        expect(runtime.snapshot?.spaces).toHaveLength(spaces)
      }
      console.log("HERDR_SCOPE_BENCH", JSON.stringify({ role, spaces, sessions, mode, batch, warmup: batch < 5, iterations: 256, parentCpuMs, latency }))
    }
  }
})

it.runIf(benchmark)("checks repeated subscription and attention recovery lifecycles", () => {
  const snapshots = setup(8, true, 8)
  useHerdrStore.getState().markAttentionSeen("default", "default-p0")
  const marker = useHerdrStore.getState().attentionItems("default").find(item => item.paneId === "default-p0")
  expect(marker?.seen).toBe(true)
  let active = 0, peak = 0, recoveries = 0
  for (let cycle = 0; cycle < 110; cycle++) {
    const snapshot = snapshots[1 + cycle % 7], scope = snapshot.herdrSessionId, paneId = `${scope}-p0`
    let notifications = 0
    const stop = useHerdrStore.subscribe(() => { notifications += 1 })
    active += 1; peak = Math.max(peak, active)
    useHerdrStore.getState().applySubscriptionEvent(scope, { type: "pane_exited", subscriptionId: `sub-${scope}`, paneId, workspaceId: `${scope}-w0` })
    expect(useHerdrStore.getState().attentionByKey.size).toBe(63)
    useHerdrStore.getState().applySnapshot(scope, structuredClone(snapshot))
    expect(useHerdrStore.getState().attentionByKey.size).toBe(64)
    expect(useHerdrStore.getState().attentionItems("default").find(item => item.paneId === "default-p0")).toBe(marker)
    expect(notifications).toBeGreaterThan(0)
    stop(); active -= 1
    const before = notifications
    useHerdrStore.getState().markAttentionSeen(scope, paneId)
    expect(notifications).toBe(before)
    expect(active).toBe(0)
    if (cycle >= 10) recoveries += 1
  }
  expect(Object.keys(useHerdrStore.getState().runtimesBySession)).toHaveLength(8)
  console.log("HERDR_SCOPE_LIFECYCLE", JSON.stringify({ role, warmup: 10, cycles: 100, recoveries, attentionItems: 64, runtimes: 8, activeSubscriptions: active, peakSubscriptions: peak }))
})
