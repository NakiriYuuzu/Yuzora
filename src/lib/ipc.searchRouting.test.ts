import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
vi.mock("./remoteFiles", () => ({ runtimeWorkspaceService: vi.fn() }))
import { runtimeWorkspaceService } from "./remoteFiles"
import { searchWorkspace } from "./ipc"
import { stopRemoteSearch } from "./remoteSearch"
import { remoteFilePath } from "./runtimeIdentity"
import type { SearchEvent } from "./types"

const owner = { hostId: "owned-search-routing", generation: 1 }
const root = remoteFilePath(owner.hostId, "/project")
const done: SearchEvent = { type: "done", truncated: false, fileCount: 0 }
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(ok => { resolve = ok })
  return { promise, resolve }
}

beforeEach(() => {
  vi.mocked(runtimeWorkspaceService).mockReturnValue({ owner, root: "/project", uri: root, capabilityId: "owned-cap", assertCurrent: () => {} })
})
afterEach(async () => {
  await stopRemoteSearch()
  clearMocks()
  vi.clearAllMocks()
})

it.each(["query", "clear", "remote"] as const)("a delayed local search cannot overtake a newer %s request", async replacement => {
  const closing = deferred<void>()
  const closed = deferred<void>()
  const local: { query: string; onEvent: { onmessage: (event: SearchEvent) => void } }[] = []
  const remote: { config: { query: string }; onEvent: { onmessage: (event: unknown) => void } }[] = []
  const closedIds: string[] = []
  mockIPC((command, payload) => {
    if (command === "host_stream_open") {
      const args = payload as (typeof remote)[number]
      remote.push(args)
      return { streamId: args.config.query }
    }
    if (command === "host_stream_close") {
      const { streamId } = payload as { streamId: string }
      closedIds.push(streamId)
      if (streamId === "remote-start") { closing.resolve(); return closed.promise }
      return
    }
    if (command === "search_workspace") local.push(payload as (typeof local)[number])
  })
  await searchWorkspace(root, "remote-start", false, vi.fn())
  const staleEvents = vi.fn()
  const stale = searchWorkspace("/local", "old", false, staleEvents)
  await closing.promise
  const currentEvents = vi.fn()
  const query = replacement === "clear" ? "" : "current"
  await searchWorkspace(replacement === "remote" ? root : "/local", query, true, currentEvents)
  closed.resolve()
  await stale

  // Native search increments its singleton generation on each dispatch. An
  // obsolete dispatch here would cancel the current query after its UI started.
  expect(local.map(call => call.query)).toEqual(replacement === "remote" ? [] : [query])
  expect(closedIds).toEqual(["remote-start"])
  if (replacement === "remote") {
    expect(remote.map(call => call.config.query)).toEqual(["remote-start", "current"])
    remote[1].onEvent.onmessage({ type: "frame", frame: { version: 1, owner, payload: { type: "search", event: done } } })
  } else local[0].onEvent.onmessage(done)
  expect(currentEvents).toHaveBeenCalledExactlyOnceWith(done)
  expect(staleEvents).not.toHaveBeenCalled()
})

it("only routes the latest request when calls overlap before the module import settles", async () => {
  const queries: string[] = []
  mockIPC((command, args) => {
    if (command === "search_workspace") queries.push((args as { query: string }).query)
  })
  await Promise.all([
    searchWorkspace("/local", "old", false, vi.fn()),
    searchWorkspace("/local", "current", false, vi.fn()),
  ])
  expect(queries).toEqual(["current"])
})

it("preserves failure of the current native dispatch", async () => {
  mockIPC(command => {
    if (command === "search_workspace") throw new Error("owned-native-failed")
  })
  await expect(searchWorkspace("/local", "current", false, vi.fn())).rejects.toThrow("owned-native-failed")
})
