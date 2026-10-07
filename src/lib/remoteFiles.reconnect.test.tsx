import { act, cleanup, render } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearAll } from "@/editor/documentRegistry"
import { useSshStore } from "@/state/sshStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { WorkspaceResourcesBridge } from "@/workbench/WorkspaceResourcesBridge"
import { readRemoteFile, readRemoteFileSnapshot, reconnectRemoteWorkspaces, registerRuntimeWorkspace, registerSftpWorkspace, releaseRemoteWorkspace, saveRemoteFile } from "./remoteFiles"
import { remoteFilePath } from "./runtimeIdentity"

const events = vi.hoisted(() => ({ count: 0, last: null as { workspaceRoot: string; paths: string[] } | null }))
vi.mock("@tauri-apps/api/event", () => ({ emit: async (_name: string, payload: typeof events.last) => { events.count++; events.last = payload } }))

type Provider = "runtime" | "sftp"
type Scenario = "closed" | "refreshed" | "unchanged" | "discarded"
const providers = ["runtime", "sftp"] as const
const scenarios = ["closed", "refreshed", "unchanged"] as const
const initialWorkspace = useWorkspaceStore.getState()
const initialSsh = useSshStore.getState()
const measurement = import.meta.env.VITE_YUZORA_PERF_MEASURE === "1"
let subscriptions = 0

beforeEach(() => {
  vi.useFakeTimers()
  subscriptions = 0
  const subscribe = useWorkspaceStore.subscribe
  vi.spyOn(useWorkspaceStore, "subscribe").mockImplementation(listener => {
    subscriptions++
    const unsubscribe = subscribe(listener)
    let active = true
    return () => { unsubscribe(); if (active) { active = false; subscriptions-- } }
  })
})
afterEach(async () => {
  cleanup()
  useWorkspaceStore.setState({ workspacePath: null })
  clearAll()
  clearMocks()
  useWorkspaceStore.setState(initialWorkspace, true)
  useSshStore.setState(initialSsh, true)
  expect(subscriptions).toBe(0)
  vi.restoreAllMocks()
  vi.useRealTimers()
})

async function settle() { for (let index = 0; index < 20; index++) await Promise.resolve() }

async function exercise(provider: Provider, scenario: Scenario) {
  const hostId = `reconnect-${provider}`
  const hostPath = "/reconnect"
  const names = Array.from({ length: 16 }, (_, index) => `file-${index}.ts`)
  const serverRevisions = new Map(names.map(name => [name, `original:${name}`]))
  const writes: Array<{ name: string; revision: string }> = []
  const reads = { seed: 0, reconnect: 0, fresh: 0 }
  let phase: "seed" | "reconnect" | "fresh" | "verify" = "seed"
  let generation = 1
  let pending = 0
  let releaseFirst: (() => void) | undefined
  let firstStarted = false
  let opens = 0
  let closes = 0
  const nameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1)
  const snapshot = (revision: string) => ({ file: { kind: "full", content: revision, size: revision.length, lineEnding: "lf" }, revision })
  const read = async (path: string) => {
    const name = nameOf(path)
    if (phase === "verify") throw new Error("Unexpected verification read")
    const startedPhase = phase
    reads[startedPhase]++
    pending++
    try {
      if (startedPhase === "reconnect") {
        if (!firstStarted) {
          firstStarted = true
          await new Promise<void>(resolve => { releaseFirst = resolve })
        } else {
          await new Promise<void>(resolve => setTimeout(resolve, 25))
        }
      }
      const revision = serverRevisions.get(name)
      if (!revision) throw new Error(`Unknown fixture file ${name}`)
      return snapshot(revision)
    } finally { pending-- }
  }
  const write = (path: string, revision: string) => {
    const name = nameOf(path)
    expect(revision).toBe(serverRevisions.get(name))
    writes.push({ name, revision })
    const saved = `saved:${name}`
    serverRevisions.set(name, saved)
    return saved
  }
  mockIPC((command, payload) => {
    const args = payload as Record<string, unknown>
    if (command === "sftp_list_dir") return { cwd: hostPath, entries: [] }
    if (command === "sftp_open_file") return read(args.path as string)
    if (command === "sftp_save_file") return write(args.path as string, args.expectedRevision as string)
    if (command !== "host_request") throw new Error(`Unexpected command ${command}`)
    const operation = args.operation as { method: string; params: Record<string, unknown> }
    if (operation.method === "workspaceOpen") return { canonicalPath: hostPath, capabilityId: `cap-${++opens}` }
    if (operation.method === "workspaceClose") { closes++; return null }
    if (operation.method === "filesRead") return read(operation.params.path as string)
    if (operation.method === "filesWrite") return { revision: write(operation.params.path as string, operation.params.revision as string) }
    throw new Error(`Unexpected operation ${operation.method}`)
  })
  const setSshSession = (sessionId: string) => useSshStore.setState({ sessions: {
    [hostId]: { hostId, sessionId, status: "connected", fingerprint: null, knownHost: true, error: null },
  } })
  setSshSession("fixture-1")
  const root = provider === "runtime"
    ? await registerRuntimeWorkspace({ hostId, generation: 1 }, hostPath, () => generation === 1)
    : await registerSftpWorkspace(hostId, hostPath)
  const files = names.map(name => remoteFilePath(hostId, `${hostPath}/${name}`, hostPath))
  for (const file of files) await readRemoteFile(file)
  useWorkspaceStore.setState({ workspacePath: root, groups: [{
    tabs: files.map((path, index) => ({ path, name: names[index], dirty: index === 0, externallyModified: false })),
    activePath: files[0],
  }], activeGroupIndex: 0 })
  const view = render(<WorkspaceResourcesBridge />)
  expect(subscriptions).toBe(1)
  generation = 2
  setSshSession("fixture-2")
  serverRevisions.set(names.at(-1)!, "externally-changed")
  phase = "reconnect"
  const beforeEvents = events.count
  const reconnect = provider === "runtime"
    ? reconnectRemoteWorkspaces({ hostId, generation: 2 }, () => generation === 2)
    : registerSftpWorkspace(hostId, hostPath)
  await settle()
  expect(firstStarted).toBe(true)
  expect(pending).toBe(1)
  const changedFiles = files.slice(1, -1)
  if (scenario === "closed") {
    // Exercise the real close action and Bridge, not direct revision deletion.
    act(() => useWorkspaceStore.getState().closeTabsByPath(changedFiles))
    expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual([files[0], files.at(-1)])
  } else if (scenario === "refreshed" || scenario === "discarded") {
    phase = "fresh"
    for (let index = 1; index < files.length - 1; index++) {
      serverRevisions.set(names[index], `fresh:${names[index]}`)
      if (scenario === "refreshed") await readRemoteFile(files[index])
      else await readRemoteFileSnapshot(files[index])
    }
    phase = "reconnect"
  }
  const started = Date.now()
  releaseFirst!()
  await settle()
  await vi.runAllTimersAsync()
  await reconnect
  const modeledWaitMs = Date.now() - started
  expect(pending).toBe(0)
  expect(vi.getTimerCount()).toBe(0)
  expect(events.count - beforeEvents).toBe(1)
  expect(events.last).toEqual({ workspaceRoot: root, paths: [root, ...(scenario === "closed" ? [files[0], files.at(-1)!] : files)] })
  expect(writes).toEqual([])
  phase = "verify"
  // Still-owned unchanged data is rebound. Changed data still requires Compare.
  await saveRemoteFile(files[0], "owned edit")
  expect(writes[0]).toEqual({ name: names[0], revision: `original:${names[0]}` })
  await expect(saveRemoteFile(files.at(-1)!, "unsafe overwrite")).rejects.toThrow("Compare")
  if (scenario === "refreshed") {
    await saveRemoteFile(files[1], "new snapshot edit")
    expect(writes[1]).toEqual({ name: names[1], revision: `fresh:${names[1]}` })
  } else if (scenario === "discarded") {
    await expect(saveRemoteFile(files[1], "discarded read is not accepted")).rejects.toThrow("Compare")
  }
  expect(writes).toHaveLength(scenario === "refreshed" ? 2 : 1)
  act(() => {
    useWorkspaceStore.getState().closeAllTabs(0)
    useWorkspaceStore.setState({ workspacePath: null })
  })
  await settle()
  await releaseRemoteWorkspace(root)
  view.unmount()
  expect(subscriptions).toBe(0)
  expect(pending).toBe(0)
  expect(vi.getTimerCount()).toBe(0)
  await expect(readRemoteFile(files[0])).rejects.toThrow("Reconnect")
  expect(closes).toBe(provider === "runtime" ? 1 : 0)
  return { provider, scenario, documents: files.length, changedDocuments: scenario === "closed" || scenario === "refreshed" ? changedFiles.length : 0,
    seedReads: reads.seed, reconnectReads: reads.reconnect, explicitFreshReads: reads.fresh,
    modeledWaitMs, modeledDelayPerQueuedReadMs: 25, events: events.count - beforeEvents,
    pendingReads: pending, timers: vi.getTimerCount(), subscriptions,
  }
}

for (const provider of providers) {
  it.each(["closed", "refreshed"] as const)(`${provider} skips obsolete queued reads for %s documents`, async scenario => {
    const result = await exercise(provider, scenario)
    expect(result.reconnectReads).toBe(2)
    expect(result.modeledWaitMs).toBe(25)
  })

  it(`${provider} still validates records after an unaccepted read`, async () => {
    const result = await exercise(provider, "discarded")
    expect(result.reconnectReads).toBe(16)
    expect(result.explicitFreshReads).toBe(14)
    expect(result.modeledWaitMs).toBe(375)
  })

  it.each(scenarios)(`${provider} preserves accepted revisions while measuring %s documents`, async scenario => {
    const result = await exercise(provider, scenario)
    if (measurement) console.log("RECONNECT_READ_METRICS", JSON.stringify(result))
  })

  it(`${provider} releases pending reads and subscriptions across repeated reconnect and close cycles`, async () => {
    let reconnectReads = 0
    let modeledWaitMs = 0
    for (let cycle = 0; cycle < 110; cycle++) {
      const result = await exercise(provider, "closed")
      if (cycle >= 10) { reconnectReads += result.reconnectReads; modeledWaitMs += result.modeledWaitMs }
    }
    if (measurement) console.log("RECONNECT_READ_LIFECYCLE", JSON.stringify({ provider, warmup: 10, cycles: 100, reconnectReads, modeledWaitMs, pendingReads: 0, timers: vi.getTimerCount(), subscriptions }))
  }, 20_000)
}
