import type { ComponentProps } from "react"
import { afterEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import type { FileNode, GitStatus } from "@/lib/types"
import { FileTree } from "./FileTree"
import { clearGitSnapshots, initialGitState, useGitStore } from "@/state/gitStore"
import { useFileTreeStore } from "@/state/fileTreeStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useDiffModalStore } from "@/state/diffModalStore"

const witness = vi.hoisted(() => {
  const enabled = (globalThis as unknown as { process?: { env: Record<string, string | undefined> } }).process?.env.YUZORA_FILE_TREE_GIT_PROBE === "1"
  const renders = new Map<string, number>()
  return {
    enabled,
    renders,
    record(name: string) { renders.set(name, (renders.get(name) ?? 0) + 1) },
    reset() { renders.clear() }
  }
})

const subscriptions = vi.hoisted(() => {
  const enabled = (globalThis as unknown as { process?: { env: Record<string, string | undefined> } }).process?.env.YUZORA_FILE_TREE_GIT_LIFECYCLE === "1"
  let active = 0, peak = 0
  let workspaceActive = 0, workspacePeak = 0
  return {
    enabled,
    active: () => active,
    peak: () => peak,
    workspaceActive: () => workspaceActive,
    workspacePeak: () => workspacePeak,
    beginWorkspace() {
      workspaceActive += 1
      workspacePeak = Math.max(workspacePeak, workspaceActive)
      let live = true
      return () => { if (live) { live = false; workspaceActive -= 1 } }
    },
    resetPeak() { peak = active },
    begin() {
      active += 1
      peak = Math.max(peak, active)
      let live = true
      return () => { if (live) { live = false; active -= 1 } }
    }
  }
})

vi.mock("@/state/gitStore", async importOriginal => {
  const actual = await importOriginal<typeof import("@/state/gitStore")>()
  if (!subscriptions.enabled) return actual
  const { useStore } = await import("zustand")
  type State = ReturnType<typeof actual.useGitStore.getState>
  const observedApi = {
    ...actual.useGitStore,
    subscribe(listener: (state: State, previous: State) => void) {
      const finish = subscriptions.begin()
      const stop = actual.useGitStore.subscribe(listener)
      return () => { stop(); finish() }
    }
  }
  const useObserved = (selector: (state: State) => unknown = state => state) => useStore(observedApi, selector)
  return { ...actual, useGitStore: Object.assign(useObserved, observedApi) as typeof actual.useGitStore }
})

vi.mock("@/state/workspaceStore", async importOriginal => {
  const actual = await importOriginal<typeof import("@/state/workspaceStore")>()
  if (!subscriptions.enabled) return actual
  const { useStore } = await import("zustand")
  type State = ReturnType<typeof actual.useWorkspaceStore.getState>
  const observedApi = {
    ...actual.useWorkspaceStore,
    subscribe(listener: (state: State, previous: State) => void) {
      const finish = subscriptions.beginWorkspace()
      const stop = actual.useWorkspaceStore.subscribe(listener)
      return () => { stop(); finish() }
    }
  }
  const useObserved = (selector: (state: State) => unknown = state => state) => useStore(observedApi, selector)
  return { ...actual, useWorkspaceStore: Object.assign(useObserved, observedApi) as typeof actual.useWorkspaceStore }
})

const processApi = (globalThis as unknown as {
  process?: {
    env: Record<string, string | undefined>
    cpuUsage(): { user: number; system: number }
    memoryUsage(): { rss: number; heapUsed: number }
  }
}).process
const benchmark = processApi?.env.YUZORA_FILE_TREE_GIT_BENCH === "1"
const role = processApi?.env.YUZORA_FILE_TREE_GIT_ROLE ?? "candidate"

vi.mock("../lib/fileIcons", async importOriginal => {
  const actual = await importOriginal<typeof import("../lib/fileIcons")>()
  if (!witness.enabled) return actual
  return {
    ...actual,
    FileIcon: (props: ComponentProps<typeof actual.FileIcon>) => {
      witness.record(props.fileName)
      return <actual.FileIcon {...props} />
    }
  }
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  clearGitSnapshots()
  useGitStore.setState(initialGitState)
  useFileTreeStore.setState({ trees: {}, preciseRevision: null })
  useWorkspaceStore.setState({ workspacePath: null, treeRevision: 0, groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
  clearMocks()
  witness.reset()
})

async function fixture(count: number) {
  clearGitSnapshots()
  const root = "/tree-git-probe", folder = `${root}/folder`
  const files: FileNode[] = Array.from({ length: count }, (_, i) => ({ name: `file-${i}.ts`, path: `${folder}/file-${i}.ts`, isDir: false }))
  const roots: FileNode[] = [{ name: "folder", path: folder, isDir: true }]
  const initial: GitStatus = {
    branch: "main", headOid: "1".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
    staged: [], unstaged: files.map(file => ({ path: `folder/${file.name}`, origPath: null, status: "M" })), untracked: [], conflicted: [], inProgress: null
  }
  let response = initial, reads = 0
  mockIPC((command, args) => {
    if (command === "list_dir") return (args as { path: string }).path === root ? roots : files
    if (command === "git_status_cmd") { reads += 1; return response }
    if (command === "log_event") return null
    throw new Error(`Unexpected IPC in read-only fixture: ${command}`)
  })
  useFileTreeStore.setState({ trees: {}, preciseRevision: null })
  useWorkspaceStore.setState({ workspacePath: root, treeRevision: 0, groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
  useGitStore.setState({ ...initialGitState, environment: { status: "ready", root, version: "2.50.1" }, status: initial, snapshotStale: false })
  await useFileTreeStore.getState().ensureTree(root)
  await useFileTreeStore.getState().toggleDir(root, folder)
  const view = render(<FileTree />)
  await act(async () => { await useFileTreeStore.getState().ensureTree(root) })
  expect(view.container.querySelectorAll("li")).toHaveLength(count + 1)
  return {
    view, initial, files, root,
    reads: () => reads,
    publish: async (next: GitStatus) => {
      response = next
      await act(async () => { await useGitStore.getState().refreshQuiet() })
    }
  }
}

it("updates staged priority, deletion, untracked and conflict decorations, then clears them", async () => {
  const f = await fixture(4)
  const color = (index: number) => f.view.getByText(`file-${index}.ts`).style.color
  const weight = (index: number) => f.view.getByText(`file-${index}.ts`).style.fontWeight
  expect(color(0)).toBe("var(--git-file-modified)")
  await f.publish({
    ...f.initial,
    staged: [{ path: "folder/file-0.ts", origPath: null, status: "M" }],
    unstaged: [
      { path: "folder/file-0.ts", origPath: null, status: "D" },
      { path: "folder/file-1.ts", origPath: null, status: "D" }
    ],
    untracked: ["folder/file-2.ts"],
    conflicted: [{ path: "folder/file-3.ts", origPath: null, status: "U" }]
  })
  expect(color(0)).toBe("var(--git-file-staged)")
  expect(color(1)).toBe("var(--git-file-deleted)")
  expect(weight(1)).toBe("400")
  expect(color(2)).toBe("var(--git-file-untracked)")
  expect(color(3)).toBe("var(--git-file-conflict)")
  expect(useGitStore.getState().statusRevision).toBe(1)
  act(() => { useGitStore.setState({ status: null }) })
  for (let index = 0; index < 4; index++) {
    expect(color(index)).toBe("")
    expect(f.view.queryByRole("button", { name: `Open diff file-${index}.ts` })).toBeNull()
  }
})

it("opens the latest unstaged diff when a partially staged row keeps the same badge", async () => {
  const f = await fixture(2)
  const status = { ...f.initial, staged: [{ ...f.initial.unstaged[0] }] }
  await f.publish(status)
  const latest = structuredClone(status)
  latest.unstaged[0].status = "D"
  latest.untracked.push("outside/late.ts")
  await f.publish(latest)
  const open = vi.spyOn(useDiffModalStore.getState(), "openWorktree").mockImplementation(() => {})
  fireEvent.click(f.view.getByRole("button", { name: "Open diff file-0.ts" }))
  expect(open).toHaveBeenCalledTimes(1)
  const [root, files, selected] = open.mock.calls[0]
  expect(root).toBe(f.root)
  expect(files).toEqual([
    { path: "folder/file-0.ts", origPath: null, status: "M", staged: true },
    { path: "folder/file-0.ts", origPath: null, status: "D", staged: false },
    { path: "folder/file-1.ts", origPath: null, status: "M", staged: false },
    { path: "outside/late.ts", origPath: null, status: "?", staged: false }
  ])
  expect(selected).toEqual({ path: "folder/file-0.ts", staged: false })
  expect(useGitStore.getState().statusRevision).toBe(2)
})

it("reprojects visible rows when the repository root changes without replacing the status", async () => {
  const f = await fixture(2)
  const status = useGitStore.getState().status
  expect(f.view.getByRole("button", { name: "Open diff file-0.ts" })).toBeTruthy()
  act(() => { useGitStore.setState({ environment: { status: "ready", root: "/other-repository", version: "2.50.1" } }) })
  expect(f.view.queryByRole("button", { name: "Open diff file-0.ts" })).toBeNull()
  expect(f.view.getByText("file-0.ts").style.color).toBe("")
  act(() => { useGitStore.setState({ environment: { status: "ready", root: f.root, version: "2.50.1" } }) })
  expect(useGitStore.getState().status).toBe(status)
  expect(f.view.getByRole("button", { name: "Open diff file-0.ts" })).toBeTruthy()
  expect(f.view.getByText("file-0.ts").style.color).toBe("var(--git-file-modified)")
})

it("opens the current repository-relative diff when the root changes but the badge stays equal", async () => {
  const f = await fixture(2)
  const status = structuredClone(f.initial)
  status.unstaged.push(...f.initial.unstaged.map(file => ({ ...file, path: file.path.slice("folder/".length) })))
  await f.publish(status)
  const publishedStatus = useGitStore.getState().status
  const open = vi.spyOn(useDiffModalStore.getState(), "openWorktree").mockImplementation(() => {})
  for (const [root, path] of [[f.root + "/folder", "file-0.ts"], [f.root, "folder/file-0.ts"]] as const) {
    act(() => { useGitStore.setState({ environment: { status: "ready", root, version: "2.50.1" } }) })
    expect(useGitStore.getState().status).toBe(publishedStatus)
    expect(f.view.getByText("file-0.ts").style.color).toBe("var(--git-file-modified)")
    fireEvent.click(f.view.getByRole("button", { name: "Open diff file-0.ts" }))
    expect(open).toHaveBeenLastCalledWith(root, expect.any(Array), { path, staged: false })
  }
  expect(open).toHaveBeenCalledTimes(2)
})

it.runIf(witness.enabled)("counts actual file-tree row work during Git status publications", async () => {
  const count = 128
  const f = await fixture(count)
  try {
    for (const mode of ["equal", "one", "all", "unrelated"] as const) {
      witness.reset()
      const revision = useGitStore.getState().statusRevision, reads = f.reads()
      for (let index = 0; index < 100; index++) {
        if (mode === "unrelated") {
          act(() => { useGitStore.getState().setCommitMessage(`draft-${index}`) })
        } else {
          const status = structuredClone(f.initial)
          if (mode === "one") status.unstaged[0].status = index % 2 === 0 ? "A" : "M"
          if (mode === "all") for (const file of status.unstaged) file.status = index % 2 === 0 ? "A" : "M"
          await f.publish(status)
        }
      }
      const delta = mode === "unrelated" ? 0 : 100
      expect(useGitStore.getState().statusRevision - revision).toBe(delta)
      expect(f.reads() - reads).toBe(delta)
      expect(useGitStore.getState().status).toEqual(f.initial)
      console.log("TREE_GIT_RENDERS", JSON.stringify({ count, mode, updates: 100, rowRenders: [...witness.renders.values()].reduce((n, value) => n + value, 0), directoryRenders: witness.renders.get("folder") ?? 0, firstFileRenders: witness.renders.get("file-0.ts") ?? 0, secondFileRenders: witness.renders.get("file-1.ts") ?? 0, revisionDelta: delta, reads: delta }))
    }
    const scopedStatus = structuredClone(f.initial)
    scopedStatus.unstaged.push(...f.initial.unstaged.map(file => ({ ...file, path: file.path.slice("folder/".length) })))
    await f.publish(scopedStatus)
    for (const mode of ["root", "root-equal-badge"] as const) {
      witness.reset()
      const revision = useGitStore.getState().statusRevision, reads = f.reads()
      const alternateRoot = mode === "root" ? "/other-repository" : f.root + "/folder"
      for (let index = 0; index < 100; index++) {
        act(() => { useGitStore.setState({ environment: { status: "ready", root: index % 2 === 0 ? alternateRoot : f.root, version: "2.50.1" } }) })
      }
      expect(useGitStore.getState().statusRevision).toBe(revision)
      expect(f.reads()).toBe(reads)
      expect(f.view.getByText("file-0.ts").style.color).toBe("var(--git-file-modified)")
      console.log("TREE_GIT_RENDERS", JSON.stringify({ count, mode, updates: 100,
        rowRenders: [...witness.renders.values()].reduce((sum, n) => sum + n, 0),
        directoryRenders: witness.renders.get("folder") ?? 0,
        firstFileRenders: witness.renders.get("file-0.ts") ?? 0,
        secondFileRenders: witness.renders.get("file-1.ts") ?? 0,
        revisionDelta: useGitStore.getState().statusRevision - revision, reads: f.reads() - reads }))
    }
  } finally { f.view.unmount() }
}, 120_000)

it.runIf(witness.enabled)("counts tree row work when editor groups change", async () => {
  const f = await fixture(128)
  act(() => {
    useWorkspaceStore.setState({ groups: f.files.slice(0, 2).map(file => ({ tabs: [], activePath: file.path })) })
  })
  witness.reset()
  for (let index = 0; index < 100; index++) {
    const group = (index + 1) % 2
    act(() => { useWorkspaceStore.getState().setActiveGroup(group) })
    expect(f.view.getByText(`file-${group}.ts`).closest("button")!.className).toContain("bg-(--yz-active)")
    expect(f.view.getByText(`file-${1 - group}.ts`).closest("button")!.className).not.toContain("bg-(--yz-active)")
  }
  console.log("TREE_GROUP_RENDERS", JSON.stringify({
    count: 128, switches: 100,
    rowRenders: [...witness.renders.values()].reduce((sum, n) => sum + n, 0),
    directoryRenders: witness.renders.get("folder") ?? 0,
    unrelatedFileRenders: witness.renders.get("file-2.ts") ?? 0
  }))
})

type Scenario = "equal" | "one" | "all" | "unrelated" | "selection" | "root"
const scenarios: Scenario[] = ["equal", "one", "all", "unrelated", "selection", "root"]

async function update(f: Awaited<ReturnType<typeof fixture>>, scenario: Scenario, index: number) {
  if (scenario === "unrelated") {
    act(() => { useGitStore.getState().setCommitMessage(`draft-${index}`) })
  } else if (scenario === "selection") {
    act(() => { useWorkspaceStore.setState(state => ({ groups: [{ ...state.groups[0], activePath: f.files[index % 2].path }] })) })
  } else if (scenario === "root") {
    act(() => { useGitStore.setState({ environment: { status: "ready", root: index % 2 === 0 ? "/other-repository" : f.root, version: "2.50.1" } }) })
  } else {
    const status = structuredClone(f.initial)
    if (scenario === "one") status.unstaged[0].status = index % 2 === 0 ? "A" : "M"
    if (scenario === "all") for (const file of status.unstaged) file.status = index % 2 === 0 ? "A" : "M"
    await f.publish(status)
  }
}

it.runIf(benchmark)("measures actual file-tree update costs without render or subscription instrumentation", async () => {
  expect(witness.enabled).toBe(false)
  expect(subscriptions.enabled).toBe(false)
  const cpuMs = () => { const time = processApi!.cpuUsage(); return (time.user + time.system) / 1000 }
  const isolatedCase = processApi?.env.YUZORA_FILE_TREE_GIT_CASE
  if (isolatedCase) {
    expect([8, 128, 512].flatMap(count => scenarios.map(scenario => `${count}:${scenario}`))).toContain(isolatedCase)
  }
  const batches = isolatedCase ? 30 : 10, warmupBatches = isolatedCase ? 10 : 3
  for (const count of [8, 128, 512]) for (const scenario of scenarios) {
    if (isolatedCase && isolatedCase !== `${count}:${scenario}`) continue
    const f = await fixture(count)
    if (scenario === "selection") {
      act(() => { useWorkspaceStore.setState({ groups: [{ tabs: f.files.slice(0, 2).map(file => ({ path: file.path, name: file.name, dirty: false, externallyModified: false })), activePath: f.files[1].path }] }) })
    }
    try {
      const iterations = isolatedCase && scenario === "unrelated" ? 1024
        : isolatedCase && scenario === "selection" ? 128
        : count === 8 ? 128 : count === 128 ? 32 : 16
      for (let batch = 0; batch < batches; batch++) {
        const revision = useGitStore.getState().statusRevision, reads = f.reads()
        const latency: number[] = [], cpu = cpuMs()
        for (let index = 0; index < iterations; index++) {
          const started = performance.now()
          await update(f, scenario, index)
          latency.push(performance.now() - started)
        }
        const parentCpuMs = cpuMs() - cpu
        const expected = ["equal", "one", "all"].includes(scenario) ? iterations : 0
        expect(f.reads() - reads).toBe(expected)
        expect(useGitStore.getState().statusRevision - revision).toBe(expected)
        expect(useGitStore.getState().status).toEqual(f.initial)
        expect(f.view.container.querySelectorAll("li")).toHaveLength(count + 1)
        expect(f.view.getByText("file-0.ts").style.color).toBe("var(--git-file-modified)")
        console.log("TREE_GIT_BENCH", JSON.stringify({ role, count, scenario, batch, warmup: batch < warmupBatches, iterations, parentCpuMs, latency, reads: expected, revisionDelta: expected }))
      }
    } finally { f.view.unmount(); cleanup(); clearMocks() }
  }
}, 600_000)

it.runIf(subscriptions.enabled)("releases Git subscriptions and scroll listeners through repeated file-tree lifecycles", async () => {
  expect(witness.enabled).toBe(false)
  expect(benchmark).toBe(false)
  const listeners = new Set<EventListenerOrEventListenerObject>()
  const add = HTMLElement.prototype.addEventListener, remove = HTMLElement.prototype.removeEventListener
  const addDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "addEventListener")
  const removeDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "removeEventListener")
  // Thin forwarding observers keep only currently attached scroll callbacks.
  // No mocking library retains historical DOM receivers or listener arguments.
  Object.defineProperty(HTMLElement.prototype, "addEventListener", {
    configurable: true, writable: true,
    value(this: HTMLElement, type: string, listener: EventListenerOrEventListenerObject, options?: boolean | AddEventListenerOptions) {
      if (type === "scroll" && typeof options === "object" && options.passive) listeners.add(listener)
      return add.call(this, type, listener, options)
    }
  })
  Object.defineProperty(HTMLElement.prototype, "removeEventListener", {
    configurable: true, writable: true,
    value(this: HTMLElement, type: string, listener: EventListenerOrEventListenerObject, options?: boolean | EventListenerOptions) {
      if (type === "scroll") listeners.delete(listener)
      return remove.call(this, type, listener, options)
    }
  })
  try {
    const count = 64, baseline = subscriptions.active()
    const workspaceBaseline = subscriptions.workspaceActive()
    const workspaceSubscriptionsPerRow = processApi?.env.YUZORA_FILE_TREE_GROUP_BASELINE === "1" ? 4 : 3
    const subscriptionsPerRow = role === "baseline" ? 2 : 1
    subscriptions.resetPeak()
    const resources = Array.from({ length: 10 }, () => ({ cycle: 0, subscriptions: 0, scrollListeners: 0, domChildren: 0, rss: 0, heapUsed: 0 }))
    // Keep the workspace and stores alive while reopening the same tree. Only
    // release mounted roots between cycles.
    const f = await fixture(count)
    let view = f.view
    for (let cycle = 0; cycle < 110; cycle++) {
      if (cycle > 0) {
        view = render(<FileTree />)
        await act(async () => { await useFileTreeStore.getState().ensureTree(f.root) })
      }
      expect(subscriptions.active() - baseline).toBe(subscriptionsPerRow * (count + 1))
      expect(subscriptions.workspaceActive() - workspaceBaseline).toBe(workspaceSubscriptionsPerRow * (count + 1) + 2)
      expect(listeners.size).toBe(1)
      const reads = f.reads(), revision = useGitStore.getState().statusRevision
      for (let index = 0; index < 4; index++) await update(f, index < 2 ? "equal" : "one", index)
      expect(f.reads() - reads).toBe(4)
      expect(useGitStore.getState().statusRevision - revision).toBe(4)
      view.unmount()
      cleanup()
      expect(subscriptions.active()).toBe(baseline)
      expect(subscriptions.workspaceActive()).toBe(workspaceBaseline)
      expect(listeners.size).toBe(0)
      expect(view.container.childElementCount).toBe(0)
      // Let ordinary event-loop cleanup run; this neither requests nor forces GC.
      await new Promise<void>(resolve => setTimeout(resolve, 0))
      if (cycle >= 10 && (cycle + 1) % 10 === 0) {
        const memory = processApi!.memoryUsage(), measured = cycle - 9
        Object.assign(resources[measured / 10 - 1], { cycle: measured, subscriptions: subscriptions.active() - baseline, scrollListeners: listeners.size, domChildren: view.container.childElementCount, rss: memory.rss, heapUsed: memory.heapUsed })
      }
    }
    expect(subscriptions.peak() - baseline).toBe(subscriptionsPerRow * (count + 1))
    expect(f.reads()).toBe(440)
    console.log("TREE_GIT_LIFECYCLE", JSON.stringify({ role, count, warmup: 10, cycles: 100, publications: f.reads(), persistentStores: true, peakSubscriptions: subscriptions.peak() - baseline, remainingSubscriptions: subscriptions.active() - baseline, peakWorkspaceSubscriptions: subscriptions.workspacePeak() - workspaceBaseline, remainingWorkspaceSubscriptions: subscriptions.workspaceActive() - workspaceBaseline, resources }))
  } finally {
    cleanup()
    if (addDescriptor) Object.defineProperty(HTMLElement.prototype, "addEventListener", addDescriptor)
    else Reflect.deleteProperty(HTMLElement.prototype, "addEventListener")
    if (removeDescriptor) Object.defineProperty(HTMLElement.prototype, "removeEventListener", removeDescriptor)
    else Reflect.deleteProperty(HTMLElement.prototype, "removeEventListener")
  }
}, 120_000)
