/** Read-only production-module probes. Run: bun <this-file> channels|documents|workspaces
 * Native IPC/filesystem responses are simulated; real app modules and Channel run unchanged.
 * Counts demonstrate retained ownership, not a measurement of native RSS or heap bytes.
 */
import { mock } from 'bun:test'
import { resolve } from 'node:path'
import { strict as assert } from 'node:assert'
const root = resolve(import.meta.dir, '../../..')
const source = (path: string) => resolve(root, 'src', path)
const results: Record<string, unknown> = {}
const storage = new Map<string, string>()
;(globalThis as any).window = globalThis
;(globalThis as any).localStorage = { getItem: (k: string) => storage.get(k) ?? null, setItem: (k: string, v: string) => storage.set(k, v), removeItem: (k: string) => storage.delete(k) }
mock.module(source('lib/i18n/index.ts'), () => ({ default: { t: (s: string) => s, language: 'en' } }))
mock.module(source('state/runtimePreferencesStore.ts'), () => ({ useRuntimePreferencesStore: { getState: () => ({ wslEnabled: true }) } }))
const mode = process.argv[2]
if (mode === 'channels') {
  const callbacks = new Map<number, (message: any) => void>()
  let nextCallback = 0, nextStream = 0, closeCalls = 0
  const nativeStreams = new Map<string, any>()
  ;(globalThis as any).__TAURI_INTERNALS__ = {
    transformCallback: (fn: any) => { const id = ++nextCallback; callbacks.set(id, fn); return id },
    unregisterCallback: (id: number) => callbacks.delete(id),
    invoke: async (command: string, args: any) => {
      if (command === 'host_stream_open') {
        const streamId = String(++nextStream)
        nativeStreams.set(streamId, args.onEvent)
        return { streamId, value: { mode: 'control', role: 'controller', takeover: true, target: 'terminal' } }
      }
      if (command === 'host_stream_close') {
        closeCalls++
        const channel = nativeStreams.get(args.streamId)
        // Emulate the real Rust Channel's Drop/end signal for the channel it owns.
        if (channel) callbacks.get(channel.id)?.({ end: true, index: 0 })
        nativeStreams.delete(args.streamId)
        return null
      }
      throw new Error(`Unexpected native operation ${command}`)
    }
  }
  const provider = await import(source('lib/herdrProvider.ts'))
  const ipc = await import(source('lib/herdrIpc.ts'))
  const host = (generation = 1) => ({ owner: { hostId: 'audit', generation }, hello: { protocol: 1, version: 'fixture', os: 'linux', arch: 'arm64', home: '/tmp', methods: [] } })
  const sessionName = JSON.stringify(['audit', 'default'])
  provider.registerRuntimeHost(host(), '/herdr', 'Audit')
  for (let i = 0; i < 100; i++) {
    const opened = await ipc.herdrTerminalOpen({ target: 'terminal', cols: 80, rows: 24, sessionName, onEvent: () => {} })
    await ipc.herdrTerminalRelease(opened.sessionId)
  }
  results.terminalCycles = 100
  results.callbacksAfterTerminalClose = callbacks.size
  for (let i = 0; i < 100; i++) {
    const id = await ipc.herdrEventsSubscribe({ sessionName, onEvent: () => {} })
    await ipc.herdrEventsRelease(id)
  }
  results.subscriptionCycles = 100
  results.callbacksAfterAllCloses = callbacks.size
  results.nativeStreamsAfterAllCloses = nativeStreams.size
  assert.equal(callbacks.size, 200, 'Observed one leaked wrapper Channel per open')
  assert.equal(nativeStreams.size, 0)
  let staleReleaseFailures = 0
  const beforeDisconnectCloses = closeCalls
  for (let generation = 2; generation < 27; generation++) {
    const current = host(generation)
    provider.registerRuntimeHost(current, '/herdr', 'Audit')
    const id = await ipc.herdrEventsSubscribe({ sessionName, onEvent: () => {} })
    const innerId = JSON.parse(id)[2]
    const channel = nativeStreams.get(innerId)
    provider.unregisterRuntimeHost(current.owner)
    // Native disconnect sends closed, then the Channel end notification.
    channel.onmessage({ type: 'closed', owner: current.owner, streamId: innerId, reason: 'host-disconnected' })
    callbacks.get(channel.id)?.({ end: true, index: 0 })
    nativeStreams.delete(innerId)
    try { await ipc.herdrEventsRelease(id) } catch (error) {
      assert.ok(error instanceof provider.StaleRuntimeResponse)
      staleReleaseFailures++
    }
  }
  results.disconnectCycles = 25
  results.staleEntriesStillFoundByRelease = staleReleaseFailures
  results.nativeCloseCallsFromStaleRelease = closeCalls - beforeDisconnectCloses
  assert.equal(staleReleaseFailures, 25, 'Absent stream would return successfully; stale entries remain in map')
} else if (mode === 'documents') {
  let reads = 0
  let resolveRead: ((value: any) => void) | null = null
  let defer = false
  mock.module(source('lib/ipc.ts'), () => ({ openFileSnapshot: async (path: string) => {
    reads++
    const snapshot = { result: { kind: 'full', content: path + 'x'.repeat(65536), lineEnding: 'lf' }, accept() {} }
    if (defer) return new Promise(done => { resolveRead = () => done(snapshot) })
    return snapshot
  } }))
  const { useWorkspaceStore } = await import(source('state/workspaceStore.ts'))
  const docs = await import(source('editor/documentRegistry.ts'))
  useWorkspaceStore.getState().setWorkspace('/audit')
  for (let i = 0; i < 25; i++) {
    const path = `/audit/file-${i}.txt`
    useWorkspaceStore.getState().splitRight()
    useWorkspaceStore.getState().openTabInGroup(path, 1)
    await docs.getDocument(path)
    useWorkspaceStore.getState().closeSplit()
  }
  results.splitCloseCycles = 25
  results.openTabsAfterClose = useWorkspaceStore.getState().groups.flatMap((g: any) => g.tabs).length
  const readsBeforeProbe = reads
  for (let i = 0; i < 25; i++) await docs.getDocument(`/audit/file-${i}.txt`)
  results.newReadsWhenProbingClosedFiles = reads - readsBeforeProbe
  results.closedFilesStillCached = 25 - (reads - readsBeforeProbe)
  assert.equal(results.openTabsAfterClose, 0)
  assert.equal(results.closedFilesStillCached, 25)
  docs.clearAll()
  defer = true
  const pending = docs.getDocument('/audit/late.txt')
  docs.dropDocument('/audit/late.txt')
  resolveRead!(null)
  await pending
  defer = false
  const beforeLateProbe = reads
  await docs.getDocument('/audit/late.txt')
  results.newReadsAfterCloseDuringLoad = reads - beforeLateProbe
  assert.equal(results.newReadsAfterCloseDuringLoad, 0, 'Late read repopulates a dropped document')
} else if (mode === 'workspaces') {
  const capabilities = new Map<string, string>()
  let next = 0, closes = 0
  mock.module(source('lib/hostIpc.ts'), () => ({ requestHost: async (_owner: any, operation: any) => {
    if (operation.method === 'workspaceOpen') {
      if (capabilities.size >= 128) throw new Error('too-many-workspaces')
      const capabilityId = `workspace-${++next}`
      capabilities.set(capabilityId, operation.params.path)
      return { canonicalPath: operation.params.path, capabilityId }
    }
    if (operation.method === 'workspaceClose') { closes++; capabilities.delete(operation.params.workspace); return null }
    throw new Error(`Unexpected host operation ${operation.method}`)
  } }))
  mock.module(source('lib/ipc.ts'), () => ({ invoke: async () => null, sftpListDir: async () => ({ entries: [] }) }))
  mock.module(source('state/sshStore.ts'), () => ({ useSshStore: { getState: () => ({ sessions: {} }) } }))
  mock.module(source('state/remoteWorkspaceRegistry.ts'), () => ({ loadRemoteWorkspaces: () => ({}), rememberRemoteWorkspace: () => {} }))
  const { registerRuntimeWorkspace } = await import(source('lib/remoteFiles.ts'))
  const { useWorkspaceStore } = await import(source('state/workspaceStore.ts'))
  for (let i = 0; i < 128; i++) {
    const uri = await registerRuntimeWorkspace({ hostId: 'audit', generation: 1 }, `/workspaces/${i}`, () => true)
    useWorkspaceStore.getState().setWorkspace(uri)
  }
  let errorMessage = ''
  try { await registerRuntimeWorkspace({ hostId: 'audit', generation: 1 }, '/workspaces/128', () => true) } catch (error) { errorMessage = String(error) }
  results.visitedWorkspaces = 128
  results.retainedHostCapabilities = capabilities.size
  results.workspaceCloseCalls = closes
  results.nextWorkspaceError = errorMessage
  assert.equal(closes, 0)
  assert.match(errorMessage, /too-many-workspaces/)
} else throw new Error('Choose channels, documents, or workspaces')
console.log(JSON.stringify({ mode, method: 'real modules with simulated I/O; no browser', results }, null, 2))
