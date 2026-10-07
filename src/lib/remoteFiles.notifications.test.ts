import { afterEach, describe, expect, it, vi } from "vitest"

const boundary = vi.hoisted(() => ({
  sequence: 0,
  capabilities: new Set<string>(),
  streams: new Set<string>(),
  channel: null as { onmessage: (message: unknown) => void } | null,
  emitted: 0,
  last: null as { workspaceRoot: string; paths: string[] } | null,
}))
vi.mock("@tauri-apps/api/core", () => ({ Channel: class { onmessage = (_message: unknown) => {} } }))
vi.mock("@tauri-apps/api/event", () => ({ emit: async (name: string, payload: typeof boundary.last) => {
  if (name !== "fs:external-change") throw new Error("Unexpected event")
  boundary.emitted++
  boundary.last = payload
} }))
vi.mock("@/state/sshStore", () => ({ useSshStore: { getState: () => ({ sessions: {} }) } }))
vi.mock("@/state/workspaceStore", () => ({ useWorkspaceStore: { getState: () => ({ workspacePath: null }) } }))
vi.mock("./ipc", () => ({
  sftpListDir: async () => { throw new Error("Unexpected SFTP request") },
  invoke: async (command: string, args: Record<string, unknown>) => {
    if (command === "host_stream_open") {
      const streamId = `stream-${++boundary.sequence}`
      boundary.streams.add(streamId)
      boundary.channel = args.onEvent as typeof boundary.channel
      return { streamId }
    }
    if (command === "host_stream_close") {
      if (!boundary.streams.delete(args.streamId as string)) throw new Error("Unknown stream")
      return
    }
    if (command !== "host_request") throw new Error(`Unexpected command ${command}`)
    const operation = args.operation as { method: string; params: { path?: string; workspace?: string } }
    if (operation.method === "workspaceOpen") {
      const capabilityId = `capability-${++boundary.sequence}`
      boundary.capabilities.add(capabilityId)
      return { canonicalPath: operation.params.path, capabilityId }
    }
    if (operation.method === "workspaceClose") {
      if (!boundary.capabilities.delete(operation.params.workspace!)) throw new Error("Unknown workspace")
      return
    }
    if (operation.method === "filesRead") {
      if (!boundary.capabilities.has(operation.params.workspace!)) throw new Error("Closed workspace")
      return { file: { kind: "full", content: "fixture", size: 7, lineEnding: "lf" }, revision: `revision:${operation.params.path}` }
    }
    throw new Error(`Unexpected operation ${operation.method}`)
  },
}))

import { readRemoteFile, registerRuntimeWorkspace, releaseRemoteWorkspace, startRemoteWatch, stopRemoteWatch } from "./remoteFiles"
import { remoteFilePath } from "./runtimeIdentity"

const registered = new Set<string>()
const measurement = import.meta.env.VITE_YUZORA_PERF_MEASURE === "1"
type CpuTime = { user: number; system: number }
// Vitest's manual probe runs in Node; keep Node ambient types out of the app.
function cpuUsage(previous?: CpuTime): CpuTime {
  const runtime = globalThis as typeof globalThis & { process: { cpuUsage: (previous?: CpuTime) => CpuTime } }
  return runtime.process.cpuUsage(previous)
}
async function workspace(hostId: string, hostPath: string, files: string[]) {
  const owner = { hostId, generation: 1 }
  const uri = await registerRuntimeWorkspace(owner, hostPath, () => true)
  registered.add(uri)
  const documents = files.map(relative => ({ relative, absolute: `${hostPath}/${relative}`, uri: remoteFilePath(hostId, `${hostPath}/${relative}`, hostPath) }))
  for (const document of documents) await readRemoteFile(document.uri)
  return { owner, hostPath, uri, documents }
}
type Fixture = Awaited<ReturnType<typeof workspace>>
function message(fixture: Fixture, paths: string[]) {
  return { type: "frame", frame: { version: 1, owner: fixture.owner, payload: { type: "files", workspaceRoot: fixture.hostPath, paths } } }
}
function notify(fixture: Fixture, paths: string[]) {
  if (!boundary.channel) throw new Error("No watcher")
  boundary.channel.onmessage(message(fixture, paths))
}
async function close(fixture: Fixture) {
  await stopRemoteWatch()
  await releaseRemoteWorkspace(fixture.uri)
  registered.delete(fixture.uri)
}
afterEach(async () => {
  await stopRemoteWatch()
  for (const uri of registered) await releaseRemoteWorkspace(uri)
  registered.clear()
  expect(boundary.streams.size).toBe(0)
  expect(boundary.capabilities.size).toBe(0)
  boundary.channel = null
  boundary.last = null
  vi.restoreAllMocks()
})

it.each(["/project 中文", "C:/Work 中文"])("preserves notification order, path boundaries and workspace identity for %s", async root => {
  const host = `identity-${++boundary.sequence}`
  const main = await workspace(host, root, ["src/deep/a.ts", "src/b.ts", "src-extra/c.ts", "文 字/%name.ts"])
  await workspace(host, `${root}/src`, ["deep/a.ts"])
  await workspace(`${host}-other`, root, ["src/deep/a.ts"])
  await startRemoteWatch(main.uri)
  const paths = [`${root}/src/`, `${root}/src/deep`, `${root}/src/`, main.documents[3].absolute]
  const before = boundary.emitted
  notify(main, paths)
  await Promise.resolve()
  expect(boundary.emitted).toBe(before + 1)
  expect(boundary.last).toEqual({ workspaceRoot: main.uri, paths: [
    remoteFilePath(host, paths[0], root), remoteFilePath(host, paths[1], root),
    main.documents[3].uri, main.documents[0].uri, main.documents[1].uri,
  ] })
  notify(main, [])
  await Promise.resolve()
  expect(boundary.last).toEqual({ workspaceRoot: main.uri, paths: [] })
})

it("releases each watcher and capability across repeated notification lifecycles", async () => {
  let measuredEvents = 0
  for (let cycle = 0; cycle < 110; cycle++) {
    const fixture = await workspace("notification-lifecycle", "/lifecycle", ["a.ts", "dir/b.ts", "dir/c.ts"])
    const before = boundary.emitted
    await startRemoteWatch(fixture.uri)
    notify(fixture, [fixture.hostPath])
    await Promise.resolve()
    expect(boundary.last).toEqual({ workspaceRoot: fixture.uri, paths: [fixture.uri, ...fixture.documents.map(x => x.uri)] })
    const channel = boundary.channel!
    await close(fixture)
    expect(boundary.streams.size).toBe(0)
    expect(boundary.capabilities.size).toBe(0)
    const after = boundary.emitted
    channel.onmessage(message(fixture, [fixture.hostPath]))
    expect(boundary.emitted).toBe(after)
    await expect(readRemoteFile(fixture.documents[0].uri)).rejects.toThrow("Reconnect")
    if (cycle >= 10) measuredEvents += after - before
  }
  if (measurement) console.log("REMOTE_NOTIFICATION_LIFECYCLE", JSON.stringify({ warmup: 10, cycles: 100, measuredEvents, streams: boundary.streams.size, capabilities: boundary.capabilities.size }))
})

const cases = [
  { name: "empty", count: 0, mode: "root", iterations: 4000 },
  { name: "small-root", count: 8, mode: "root", iterations: 2000 },
  { name: "medium-root", count: 64, mode: "root", iterations: 512 },
  { name: "large-root", count: 256, mode: "root", iterations: 128 },
  { name: "many-root", count: 1024, mode: "root", iterations: 32 },
  { name: "directories", count: 256, mode: "directories", iterations: 128 },
  { name: "direct-files", count: 256, mode: "direct", iterations: 32 },
  { name: "unrelated", count: 1024, mode: "unrelated", iterations: 128 },
]
describe.skipIf(!measurement)("manual remote notification CPU probe", () => {
  it.each(cases)("$name", async spec => {
    const fixture = await workspace(`notification-${spec.name}`, "/project", Array.from({ length: spec.count }, (_, index) => `d${index % 4}/file-${index}.ts`))
    await startRemoteWatch(fixture.uri)
    const paths = spec.mode === "direct" ? fixture.documents.map(x => x.absolute)
      : spec.mode === "directories" ? ["/project/d0", "/project/d2"]
        : spec.mode === "unrelated" ? ["/project/unrelated"] : [fixture.hostPath]
    const expected = {
      workspaceRoot: fixture.uri,
      paths: spec.mode === "direct" ? fixture.documents.map(x => x.uri)
        : [...paths.map(path => remoteFilePath(fixture.owner.hostId, path, fixture.hostPath)),
          ...fixture.documents.filter(x => spec.mode === "root" || spec.mode === "directories" && (x.relative.startsWith("d0/") || x.relative.startsWith("d2/"))).map(x => x.uri)],
    }
    const channel = boundary.channel!
    const frame = message(fixture, paths)
    const samples = []
    for (let sample = 0; sample < 12; sample++) {
      const beforeEvents = boundary.emitted
      const beforeCpu = cpuUsage()
      const beforeWall = performance.now()
      for (let iteration = 0; iteration < spec.iterations; iteration++) {
        channel.onmessage(frame)
        await Promise.resolve()
      }
      const wallMs = performance.now() - beforeWall
      const cpu = cpuUsage(beforeCpu)
      expect(boundary.emitted - beforeEvents).toBe(spec.iterations)
      expect(boundary.last).toEqual(expected)
      if (sample >= 5) samples.push({ cpuMs: (cpu.user + cpu.system) / 1000, wallMs })
    }
    console.log("REMOTE_NOTIFICATION_METRICS", JSON.stringify({ ...spec, samples }))
  })

  it.each(cases)("$name per-notification latency", async spec => {
    const fixture = await workspace(`latency-${spec.name}`, "/project", Array.from({ length: spec.count }, (_, index) => `d${index % 4}/file-${index}.ts`))
    await startRemoteWatch(fixture.uri)
    const paths = spec.mode === "direct" ? fixture.documents.map(x => x.absolute)
      : spec.mode === "directories" ? ["/project/d0", "/project/d2"]
        : spec.mode === "unrelated" ? ["/project/unrelated"] : [fixture.hostPath]
    const channel = boundary.channel!
    const frame = message(fixture, paths)
    const latencyMs = []
    const beforeEvents = boundary.emitted
    for (let iteration = 0; iteration < 288; iteration++) {
      const started = performance.now()
      channel.onmessage(frame)
      await Promise.resolve()
      if (iteration >= 32) latencyMs.push(performance.now() - started)
    }
    expect(boundary.emitted - beforeEvents).toBe(288)
    console.log("REMOTE_NOTIFICATION_LATENCY", JSON.stringify({ name: spec.name, count: spec.count, warmup: 32, operations: 256, latencyMs }))
  })

  // Patching an iterator can invalidate engine fast paths for the rest of the
  // process. Run this observer only after every timing case, never between them.
  it.each([0, 8, 64, 256, 1024])("counts root expansion for %i documents after CPU measurement", async count => {
    const fixture = await workspace(`notification-count-${count}`, "/project", Array.from({ length: count }, (_, index) => `file-${index}.ts`))
    await startRemoteWatch(fixture.uri)
    // Count collection iteration separately from timing. Only the notification's
    // changed Set contains this workspace root; the observer stores no payloads.
    let iterators = 0
    let entries = 0
    const iterate = Set.prototype[Symbol.iterator]
    const spy = vi.spyOn(Set.prototype, Symbol.iterator).mockImplementation(function (this: Set<unknown>) {
      if (this.has(fixture.uri)) { iterators++; entries += this.size }
      return iterate.call(this)
    })
    try { notify(fixture, [fixture.hostPath]) } finally { spy.mockRestore() }
    await Promise.resolve()
    expect(boundary.last).toEqual({ workspaceRoot: fixture.uri, paths: [fixture.uri, ...fixture.documents.map(x => x.uri)] })
    console.log("REMOTE_NOTIFICATION_ITERATION", JSON.stringify({ count, rootExpansion: { iterators, entries } }))
  })
})
