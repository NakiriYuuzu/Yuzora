import { act, render, cleanup } from "@testing-library/react"
import { afterEach, expect, it } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import { WorkspaceResourcesBridge } from "./WorkspaceResourcesBridge"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { clearAll, getDocument, updateBuffer, documentGeneration } from "@/editor/documentRegistry"
afterEach(() => { cleanup(); clearAll(); clearMocks(); useWorkspaceStore.setState({ workspacePath: null, groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 }) })
it("drops all closed split documents but preserves a moved or inactive open file", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "disk", size: 4, lineEnding: "lf" } } })
  useWorkspaceStore.getState().setWorkspace("/repo")
  render(<WorkspaceResourcesBridge />)
  act(() => useWorkspaceStore.getState().openTab("/repo/kept.ts"))
  await getDocument("/repo/kept.ts")
  updateBuffer("/repo/kept.ts", "unsaved", documentGeneration("/repo/kept.ts"))
  for (let i = 0; i < 25; i++) {
    act(() => { useWorkspaceStore.getState().splitRight(); useWorkspaceStore.getState().openTabInGroup(`/repo/${i}.ts`, 1) })
    await getDocument(`/repo/${i}.ts`)
    act(() => useWorkspaceStore.getState().closeSplit())
  }
  const before = reads
  for (let i = 0; i < 25; i++) await getDocument(`/repo/${i}.ts`)
  expect(reads - before).toBe(25)
  expect((await getDocument("/repo/kept.ts")).result).toMatchObject({ content: "unsaved" })
  act(() => { useWorkspaceStore.getState().splitRight(); useWorkspaceStore.getState().openTabInGroup("/repo/kept.ts", 1) })
  expect((await getDocument("/repo/kept.ts")).result).toMatchObject({ content: "unsaved" })
})

it("reclaims old remote capabilities across more than the host limit of workspace switches", async () => {
  const { registerRuntimeWorkspace } = await import("@/lib/remoteFiles")
  const capabilities = new Set<string>()
  let opened = 0
  mockIPC((command, payload) => {
    if (command !== "host_request") return null
    const { operation } = payload as any
    if (operation.method === "workspaceOpen") {
      if (capabilities.size >= 128) throw new Error("too-many-workspaces")
      const capabilityId = String(++opened)
      capabilities.add(capabilityId)
      return { canonicalPath: operation.params.path, capabilityId }
    }
    if (operation.method === "workspaceClose") capabilities.delete(operation.params.workspace)
    return null
  })
  render(<WorkspaceResourcesBridge />)
  for (let i = 0; i < 160; i++) {
    const uri = await registerRuntimeWorkspace({ hostId: "switch-lifetime", generation: 1 }, `/repo-${i}`, () => true)
    await act(async () => useWorkspaceStore.getState().setWorkspace(uri))
    expect(capabilities.size).toBe(1)
  }
  await act(async () => useWorkspaceStore.setState({ workspacePath: null }))
  expect(capabilities.size).toBe(0)
})
