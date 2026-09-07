import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { bindWorkspaceRoot, projectWorkspaceRoots } from "./herdrWorkspaceRoots"
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
