import { normalizeHerdrSnapshot } from "./herdrNormalize"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { bindWorkspaceRoot, directoryForSelection, projectWorkspaceRoots } from "./herdrWorkspaceRoots"
import type { HerdrSnapshot } from "./herdrTypes"
import { remoteFilePath, runtimeKey } from "./runtimeIdentity"

beforeEach(() => {
  const storage = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value)
  })
})
afterEach(() => vi.unstubAllGlobals())

it("isolates explicit roots across hosts even with identical session and Space IDs", () => {
  const a = runtimeKey({hostId: "a", sessionName: "default"})
  const b = runtimeKey({hostId: "b", sessionName: "default"})
  const snapshot: HerdrSnapshot = { protocol:20, version:"0.8.2", herdrSessionId: a, spaces: [{id:"ws",label:"Space",order:0,focused:true,path:null}],agents:[],tabs:[],terminals:[],raw:{} }
  bindWorkspaceRoot(a,"ws",remoteFilePath("a","/project"))
  expect(projectWorkspaceRoots(a,snapshot).spaces[0].path).toBe(remoteFilePath("a","/project"))
  expect(projectWorkspaceRoots(b,snapshot).spaces[0].path).toBeNull()
  expect(() => bindWorkspaceRoot(b,"ws",remoteFilePath("a","/project"))).toThrow("host mismatch")
})

it("preserves a chosen root when a later snapshot has no root metadata", () => {
  bindWorkspaceRoot("default","ws","/selected")
  const snapshot: HerdrSnapshot = { protocol:20, version:"0.8.2",herdrSessionId:"default",spaces:[{id:"ws",label:"Space",order:0,focused:true,path:null}],agents:[],tabs:[],terminals:[],raw:{agents:[{cwd:"/plugin"}]}}
  expect(projectWorkspaceRoots("default",snapshot).spaces[0].path).toBe("/selected")
})

it("keeps non-Git pane directories in the selected WSL host identity", () => {
  const scope = runtimeKey({ hostId: "wsl:ubuntu", sessionName: "default" })
  const snapshot = normalizeHerdrSnapshot({ protocol: 22, version: "0.9.0", snapshot: {
    workspaces: [{ workspace_id: "ws", active_tab_id: "t" }],
    panes: [{ workspace_id: "ws", tab_id: "t", pane_id: "p", terminal_id: "term", cwd: "/home/me/plain" }]
  } }, scope)
  expect(snapshot.spaces[0].path).toBeNull()
  expect(directoryForSelection(snapshot, "ws", "p")).toBe(remoteFilePath("wsl:ubuntu", "/home/me/plain"))
  expect(directoryForSelection(snapshot, "ws", "missing-pane")).toBeNull()
})

it("keeps an explicit workspace root ahead of a pane that has changed directory", () => {
  const snapshot = normalizeHerdrSnapshot({ protocol: 22, version: "0.9.0", snapshot: {
    workspaces: [{ workspace_id: "ws", path: "/project" }],
    panes: [{ workspace_id: "ws", pane_id: "p", terminal_id: "term", cwd: "/tmp" }]
  } }, "default")
  expect(directoryForSelection(snapshot, "ws", "p")).toBe("/project")
})
