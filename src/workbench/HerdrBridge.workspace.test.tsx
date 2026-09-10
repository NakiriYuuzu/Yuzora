import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("@/lib/platform", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/lib/platform")>(),
  isWindowsPlatform: vi.fn()
}))
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...await importOriginal<typeof import("@/lib/ipc")>(),
  openWorkspace: vi.fn(async (path: string) => ({ canonicalPath: path, capabilityId: "test" })),
  allowWorkspaceAssetScope: vi.fn(async () => undefined),
  startWatch: vi.fn(async () => undefined)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))

import { openWorkspace } from "@/lib/ipc"
import { isWindowsPlatform } from "@/lib/platform"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { useFolderPickerStore } from "@/state/folderPickerStore"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useUiStore } from "@/state/uiStore"
import { HerdrBridge } from "./HerdrBridge"

const initialWorkspace = useWorkspaceStore.getState()
const initialHerdr = useHerdrStore.getState()
const initialUi = useUiStore.getState()
const initialPicker = useFolderPickerStore.getState()
const snapshot: HerdrSnapshot = {
  herdrSessionId: "default", protocol: 20, version: "0.8.2",
  spaces: [{ id: "project", label: "Existing HERDR project", path: "/outside-yuzora", order: 0, focused: true }],
  tabs: [{ id: "tab", workspaceId: "project", terminalId: "terminal", label: "Shell", order: 0, paneCount: 1, status: "idle", active: true, focused: true }],
  agents: [], terminals: [], focusedWorkspaceId: "project", focusedTabId: "tab", raw: {}
}

beforeEach(() => {
  vi.clearAllMocks()
  useWorkspaceStore.setState({ ...initialWorkspace, sessionRestoreReady: true, workspacePath: null, groups: [{ tabs: [], activePath: null }] }, true)
  useFolderPickerStore.setState(initialPicker, true)
  useHerdrStore.setState({
    ...initialHerdr, ...herdrInitialState,
    sessions: [{ name: "default", default: true, running: true, sessionDir: "/tmp/default", socketPath: "/tmp/default.sock" }],
    selectedSessionName: "default",
    runtimesBySession: { default: { capabilities: null, snapshot, worktreeInventory: null, connectionState: "ready", errorMessage: null } },
    refreshSessions: vi.fn(async () => undefined),
    refreshSnapshot: vi.fn(async () => true),
    releaseAllAttachments: vi.fn(async () => undefined)
  }, true)
})

afterEach(async () => {
  cleanup()
  await act(async () => { useFolderPickerStore.getState().finish?.(null) })
  useFolderPickerStore.setState(initialPicker, true)
  useWorkspaceStore.setState(initialWorkspace, true)
  useHerdrStore.setState(initialHerdr, true)
  useUiStore.setState(initialUi, true)
  vi.restoreAllMocks()
})

it.each([false, true])("keeps runtime-only projects passive across repeated snapshots (Windows: %s)", async (windows) => {
  vi.mocked(isWindowsPlatform).mockReturnValue(windows)
  let pickerOpens = 0
  const stop = useFolderPickerStore.subscribe((state, previous) => {
    if (state.open && !previous.open) pickerOpens++
  })
  try {
    render(<HerdrBridge />)
    for (let revision = 1; revision <= 3; revision++) {
      await act(async () => { await Promise.resolve() })
      // Cancelling an unsolicited picker must not let a later snapshot reopen it.
      await act(async () => { useFolderPickerStore.getState().finish?.(null) })
      await act(async () => {
        const runtime = useHerdrStore.getState().runtimesBySession.default!
        useHerdrStore.setState({ runtimesBySession: { default: { ...runtime, snapshot: { ...snapshot, raw: { revision } } } } })
      })
    }
    expect(pickerOpens).toBe(0)
    expect(openWorkspace).not.toHaveBeenCalled()
    expect(useWorkspaceStore.getState().workspacePath).toBeNull()
    expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
  } finally {
    stop()
  }
})
