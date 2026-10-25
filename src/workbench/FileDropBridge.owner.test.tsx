import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { clearAll, getCachedDocument } from "@/editor/documentRegistry"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import type { OpenFileResult } from "@/lib/types"
import { useUiStore, uiInitialState } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { FileDropBridge } from "./FileDropBridge"

const events = vi.hoisted(() => ({
  drop: null as ((event: { payload: { type: "drop"; paths: string[]; position: { x: number; y: number } } }) => void) | null,
  preview: null as ((event: { payload: { paths: string[] } }) => void) | null,
  error: vi.fn(),
  log: vi.fn(),
}))
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({
  onDragDropEvent: (callback: typeof events.drop) => {
    events.drop = callback
    return Promise.resolve(() => { events.drop = null })
  },
}) }))
vi.mock("@tauri-apps/api/event", () => ({ listen: (_name: string, callback: typeof events.preview) => {
  events.preview = callback
  return Promise.resolve(() => { events.preview = null })
} }))
vi.mock("@/lib/platform", () => ({ isTauri: () => true }))
vi.mock("@/lib/actionFeedback", () => ({ showActionError: (...args: unknown[]) => events.error(...args) }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: (...args: unknown[]) => events.log(...args) }))

const initial = useWorkspaceStore.getState()
const result: OpenFileResult = { kind: "full", content: "owned draft", size: 11, lineEnding: "lf" }
let resolveSlow: (value: OpenFileResult) => void
let rejectSlow: (error: Error) => void
let slow: Promise<OpenFileResult>

beforeEach(() => {
  clearAll()
  useWorkspaceStore.setState(initial, true)
  useWorkspaceStore.getState().setWorkspace("/a", "cap-a")
  useUiStore.setState(uiInitialState)
  useUiStore.getState().setMode("database")
  events.error.mockClear()
  events.log.mockClear()
  slow = new Promise((resolve, reject) => { resolveSlow = resolve; rejectSlow = reject })
  mockIPC((command, args) => {
    const path = (args as { path?: string } | undefined)?.path
    if (command === "open_file") return path === "/outside/slow.txt" ? slow : result
    if (command === "open_workspace") return { canonicalPath: path, capabilityId: `cap-${String(path).slice(1)}` }
    if (command === "allow_workspace_asset_scope" || command === "start_watch") return undefined
    throw new Error(`Unexpected fixture IPC: ${command}`)
  })
})
afterEach(() => { cleanup(); clearAll(); clearMocks() })

async function drop(preview = false) {
  const mounted = render(<FileDropBridge />)
  await waitFor(() => expect(events.drop).not.toBeNull())
  act(() => {
    const paths = ["/outside/fast.txt", "/outside/slow.txt"]
    if (preview) events.preview!({ payload: { paths } })
    else events.drop!({ payload: { type: "drop", paths, position: { x: 20, y: 20 } } })
  })
  await waitFor(() => expect(getCachedDocument("/outside/fast.txt")).toBeDefined())
  return mounted
}

function expectNoPublication() {
  expect(useWorkspaceStore.getState().groups.flatMap(group => group.tabs)).toEqual([])
  expect(useUiStore.getState().mode).toBe("database")
  expect(events.error).not.toHaveBeenCalled()
  expect(events.log.mock.calls.filter(([name]) => name === "open_dropped_files")).toEqual([])
}

it.each([false, true])("does not publish an old drop after a workspace switch (ABA=%s)", async aba => {
  await drop(aba)
  await act(async () => {
    await openWorkspaceAtPath("/b", { restoreSessionTabs: false })
    if (aba) await openWorkspaceAtPath("/a", { restoreSessionTabs: false })
  })
  expect(getCachedDocument("/outside/fast.txt")).toBeUndefined()
  await act(async () => { resolveSlow(result) })
  expectNoPublication()
})

it("does not retarget a delayed drop into a replacement split", async () => {
  useWorkspaceStore.getState().splitRight()
  useWorkspaceStore.getState().setActiveGroup(1)
  const original = useWorkspaceStore.getState().groups[1].id
  await drop()
  act(() => {
    useWorkspaceStore.getState().closeSplit()
    useWorkspaceStore.getState().splitRight()
  })
  expect(useWorkspaceStore.getState().groups[1].id).not.toBe(original)
  await act(async () => { resolveSlow(result) })
  expectNoPublication()
})

it("does not publish a delayed drop after its bridge unmounts", async () => {
  const mounted = await drop()
  mounted.unmount()
  await act(async () => { resolveSlow(result) })
  expectNoPublication()
})

it("keeps partial successes and failure feedback for the current workspace", async () => {
  await drop()
  await act(async () => { rejectSlow(new Error("fixture read failed")) })
  expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual(["/outside/fast.txt"])
  expect(useUiStore.getState().mode).toBe("files")
  expect(events.error).toHaveBeenCalledTimes(1)
  expect(events.log).toHaveBeenCalledWith("open_dropped_files", expect.any(String), { count: 1 })
})

it("keeps the original surviving target when focus and its tabs change during a drop", async () => {
  useWorkspaceStore.getState().splitRight()
  useWorkspaceStore.getState().setActiveGroup(1)
  await drop(true)
  act(() => {
    useWorkspaceStore.getState().openTab("/a/concurrent.txt", 1)
    useWorkspaceStore.getState().setActiveGroup(0)
  })
  await act(async () => { resolveSlow(result) })
  expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
  expect(useWorkspaceStore.getState().groups[1].tabs.map(tab => tab.path)).toEqual([
    "/a/concurrent.txt", "/outside/fast.txt", "/outside/slow.txt",
  ])
  expect(events.error).not.toHaveBeenCalled()
})
