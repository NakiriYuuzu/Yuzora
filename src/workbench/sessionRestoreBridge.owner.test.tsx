import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { clearAll } from "@/editor/documentRegistry"
import { SpaceAgentTree } from "@/app/workbench/SpaceAgentTree"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import type { HerdrSessionRuntime, HerdrSnapshot } from "@/lib/herdrTypes"
import type { OpenFileResult } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useKeyboardSettingsStore } from "@/state/keyboardSettingsStore"
import { useAgentMruStore } from "@/state/agentMruStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { loadWorkspaceSession, loadWorkspaceSessionEntry, saveWorkspaceSession } from "@/state/workspaceSession"
import { SessionRestoreBridge } from "./SessionRestoreBridge"

vi.mock("@/app/workbench/HerdrLauncher", () => ({ HerdrLauncher: () => null }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (reason: Error) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const workspaceInitial = useWorkspaceStore.getState()
const herdrInitial = useHerdrStore.getState()
const full: OpenFileResult = { kind: "full", content: "fixture", size: 7, lineEnding: "lf" }
let file: ReturnType<typeof deferred<OpenFileResult>>
let opening: ReturnType<typeof deferred<{ canonicalPath: string; capabilityId: string }>>
let pauseAt: "file" | "open"
let calls: { command: string; args: Record<string, unknown> }[]
let splash: HTMLDivElement

beforeEach(() => {
  const values = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value) },
    removeItem: (key: string) => { values.delete(key) },
    clear: () => values.clear(), key: (index: number) => [...values.keys()][index] ?? null,
    get length() { return values.size },
  } })
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
  clearAll()
  useWorkspaceStore.setState({ ...workspaceInitial, workspacePath: null, sessionRestoreReady: false }, true)
  useUiStore.setState(uiInitialState)
  useKeyboardSettingsStore.setState({ overrides: {} })
  useAgentMruStore.setState({ keys: [] })
  useRecentWorkspacesStore.setState({ list: [], presentations: {} })
  localStorage.setItem("yuzora.sidebar.view", "agents")
  const snapshot = {
    herdrSessionId: "s1", protocol: 22, version: "0.9.1",
    spaces: ["a", "b"].map(id => ({ id: `space-${id}`, label: id.toUpperCase(), path: `/${id}`, order: id === "a" ? 0 : 1, focused: false })),
    agents: ["a", "b"].map(id => ({ id, name: "codex", title: `Agent ${id.toUpperCase()}`, status: "idle", workspaceId: `space-${id}`, terminalId: `term-${id}`, paneId: `pane-${id}` })),
    tabs: [], terminals: [], raw: {},
  } as unknown as HerdrSnapshot
  useHerdrStore.setState({ ...herdrInitial, ...herdrInitialState,
    sessions: [{ name: "s1", running: true, default: true, sessionDir: "/fixture", socketPath: "/fixture/sock" }],
    selectedSessionName: "s1",
    runtimesBySession: { s1: { connectionState: "ready", errorMessage: null, worktreeInventory: null,
      capabilities: { server: { running: true }, api: { workspaceFocus: true, tabFocus: true } } as HerdrSessionRuntime["capabilities"], snapshot,
    } },
  }, true)
  saveWorkspaceSession({ workspacePath: "/b", tabs: ["/b/keep.ts"], activePath: "/b/keep.ts" })
  saveWorkspaceSession({ workspacePath: "/a", tabs: ["/a/first.ts", "/a/second.ts"], activePath: "/a/second.ts" })
  file = deferred(); opening = deferred(); pauseAt = "file"; calls = []
  mockIPC((command, raw) => {
    const args = (raw ?? {}) as Record<string, unknown>
    calls.push({ command, args })
    if (command === "open_workspace") return args.path === "/a" && pauseAt === "open"
      ? opening.promise : { canonicalPath: args.path, capabilityId: `cap-${args.path}` }
    if (command === "open_file") return args.path === "/a/first.ts" ? file.promise : full
    if (command === "workspace_canonical_path") return args.path
    if (["allow_workspace_asset_scope", "start_watch", "herdr_workspace_focus"].includes(command)) return undefined
    throw new Error(`Unexpected fixture IPC: ${command}`)
  }, { shouldMockEvents: true })
  splash = document.createElement("div")
  splash.id = "yz-splash"; splash.setAttribute("role", "status")
  document.body.append(splash)
})
afterEach(() => { cleanup(); clearAll(); clearMocks(); splash.remove(); vi.restoreAllMocks() })

async function mountPending() {
  const mounted = render(<><SessionRestoreBridge /><SpaceAgentTree /></>)
  await waitFor(() => expect(calls.some(call => call.command === (pauseAt === "file" ? "open_file" : "open_workspace") && call.args.path === (pauseAt === "file" ? "/a/first.ts" : "/a"))).toBe(true))
  expect(splash).toBeInTheDocument()
  expect(splash).not.toHaveClass("yz-splash-leave")
  expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(false)
  return mounted
}
async function selectAgent(name: "A" | "B") {
  const row = screen.getAllByRole("treeitem").find(item => item.textContent?.includes(`Agent ${name}`))
  const shortcut = row?.getAttribute("aria-keyshortcuts")
  if (!shortcut) throw new Error(`Missing Agent ${name} shortcut`)
  const digit = shortcut.slice(-1)
  // Physical Option+digit still works under the splash, just as in the
  // production sidebar; no activation/store action is mocked or called here.
  fireEvent.keyDown(window, { key: digit, code: `Digit${digit}`, altKey: true })
  await waitFor(() => expect(useWorkspaceStore.getState().workspacePath).toBe(`/${name.toLowerCase()}`))
  await waitFor(() => expect(useHerdrStore.getState().selectedSpaceId).toBe(`space-${name.toLowerCase()}`))
  await waitFor(() => expect(useWorkspaceStore.getState().groups[0].tabs.some(tab => tab.kind === "herdr-terminal" && tab.path.includes(`term-${name.toLowerCase()}`))).toBe(true))
}
const readSecond = () => calls.filter(call => call.command === "open_file" && call.args.path === "/a/second.ts")

it.each([false, true])("abandons local file restoration after a real Agent shortcut changes its workspace (ABA=%s)", async aba => {
  await mountPending()
  await selectAgent("B")
  if (aba) await selectAgent("A")
  const selected = useWorkspaceStore.getState().groups[0].activePath
  await act(async () => { file.resolve(full) })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  expect(readSecond()).toEqual([])
  expect(useWorkspaceStore.getState().groups[0].activePath).toBe(selected)
  expect(loadWorkspaceSession()?.workspacePath).toBe(aba ? "/a" : "/b")
  expect(loadWorkspaceSessionEntry("/b")?.tabs).toEqual(["/b/keep.ts"])
})

it.each(["resolve", "reject"] as const)("preserves a newer Agent workspace when the old initial open later %s", async outcome => {
  pauseAt = "open"
  await mountPending()
  await selectAgent("B")
  await act(async () => {
    if (outcome === "resolve") opening.resolve({ canonicalPath: "/a", capabilityId: "cap-/a" })
    else opening.reject(new Error("owned old open failure"))
    file.resolve(full)
  })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  expect(useWorkspaceStore.getState().workspacePath).toBe("/b")
  expect(calls.filter(call => call.command === "open_file")).toEqual([])
  expect(loadWorkspaceSession()?.workspacePath).toBe("/b")
  expect(loadWorkspaceSessionEntry("/b")?.tabs).toEqual(["/b/keep.ts"])
})

it("keeps a same-workspace Agent selection made while the initial file read is pending", async () => {
  await mountPending()
  await selectAgent("A")
  const selected = useWorkspaceStore.getState().groups[0].activePath
  await act(async () => { file.resolve(full) })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  expect(readSecond()).toHaveLength(1)
  expect(useWorkspaceStore.getState().groups[0].tabs.filter(tab => !tab.kind).map(tab => tab.path)).toEqual(["/a/first.ts", "/a/second.ts"])
  expect(useWorkspaceStore.getState().groups[0].activePath).toBe(selected)
})

it("continues past a missing file while the original restore still owns its workspace", async () => {
  await mountPending()
  await act(async () => { file.reject(new Error("owned missing file")) })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  expect(readSecond()).toHaveLength(1)
  expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual(["/a/second.ts"])
  expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/a/second.ts")
  expect(splash).not.toBeInTheDocument()
})

it("restores first-group tabs without stealing a newly selected empty split", async () => {
  await mountPending()
  act(() => {
    useWorkspaceStore.getState().splitRight()
    useWorkspaceStore.getState().setActiveGroup(1)
  })
  await act(async () => { file.resolve(full) })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual(["/a/first.ts", "/a/second.ts"])
  expect(useWorkspaceStore.getState().groups[1].tabs).toEqual([])
  expect(useWorkspaceStore.getState().activeGroupIndex).toBe(1)
  expect(loadWorkspaceSessionEntry("/a")?.tabs).toEqual(["/a/first.ts", "/a/second.ts"])
})

it("stops before reading another saved file when a failed read settles after unmount", async () => {
  const mounted = await mountPending()
  mounted.unmount()
  await act(async () => { file.reject(new Error("owned read after unmount")) })
  expect(readSecond()).toEqual([])
})

it.each(["agent", "empty split"] as const)("keeps a new %s selection when the last saved file is missing", async selection => {
  saveWorkspaceSession({ workspacePath: "/a", tabs: ["/a/second.ts", "/a/first.ts"],
    activePath: "/a/second.ts", pinnedPaths: ["/a/second.ts"] })
  await mountPending()
  if (selection === "agent") await selectAgent("A")
  else act(() => {
    useWorkspaceStore.getState().splitRight()
    useWorkspaceStore.getState().setActiveGroup(1)
  })
  const before = useWorkspaceStore.getState()
  const selectedPath = before.groups[before.activeGroupIndex].activePath
  await act(async () => { file.reject(new Error("owned last file missing")) })
  await waitFor(() => expect(useWorkspaceStore.getState().sessionRestoreReady).toBe(true))
  const after = useWorkspaceStore.getState()
  expect(after.activeGroupIndex).toBe(before.activeGroupIndex)
  expect(after.groups[after.activeGroupIndex].activePath).toBe(selectedPath)
  expect(after.groups[0].tabs.filter(tab => !tab.kind).map(tab => tab.path)).toEqual(["/a/second.ts"])
  expect(loadWorkspaceSessionEntry("/a")).toMatchObject({
    tabs: ["/a/second.ts"], pinnedPaths: ["/a/second.ts"],
    activePath: selection === "agent" ? null : "/a/second.ts",
  })
  expect(loadWorkspaceSessionEntry("/b")?.tabs).toEqual(["/b/keep.ts"])
  if (selection === "empty split") expect(after.groups[1].tabs).toEqual([])
  expect(splash).not.toBeInTheDocument()
})
