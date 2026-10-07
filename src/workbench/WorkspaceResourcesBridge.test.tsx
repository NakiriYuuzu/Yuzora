import { act, render, cleanup } from "@testing-library/react"
import { afterEach, expect, it } from "vitest"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import { WorkspaceResourcesBridge } from "./WorkspaceResourcesBridge"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useSvgPreviewStore } from "@/state/svgPreviewStore"
import { clearAll, getDocument, updateBuffer, documentGeneration } from "@/editor/documentRegistry"
afterEach(() => { cleanup(); clearAll(); clearMocks(); useWorkspaceStore.setState({ workspacePath: null, groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 }); useSvgPreviewStore.getState().reset() })
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

it("retires the document and SVG preview state replaced by a preview-mode tab", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "<svg/>", size: 6, lineEnding: "lf" } } })
  useWorkspaceStore.getState().setWorkspace("/repo")
  render(<WorkspaceResourcesBridge />)
  act(() => useWorkspaceStore.getState().openTab("/repo/a.svg", undefined, { transient: true }))
  await getDocument("/repo/a.svg")
  act(() => useSvgPreviewStore.getState().toggle("/repo/a.svg"))
  expect(useSvgPreviewStore.getState().isOpen("/repo/a.svg")).toBe(false)

  act(() => useWorkspaceStore.getState().openTab("/repo/b.ts", undefined, { transient: true }))

  expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual(["/repo/b.ts"])
  expect(useSvgPreviewStore.getState().closedPaths).toEqual({})
  const before = reads
  await getDocument("/repo/a.svg")
  expect(reads - before).toBe(1)
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

it("preserves unsaved documents and SVG state across cloned metadata and focus updates", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "disk", size: 4, lineEnding: "lf" } } })
  const path = "/repo/metadata.svg"
  useWorkspaceStore.setState({ workspacePath: "/repo", groups: [{ tabs: [{ path, name: "metadata.svg", kind: "file", dirty: false, externallyModified: false }], activePath: path }] })
  render(<WorkspaceResourcesBridge />)
  await getDocument(path)
  updateBuffer(path, "unsaved", documentGeneration(path))
  useSvgPreviewStore.getState().toggle(path)
  for (let i = 0; i < 100; i++) {
    act(() => useWorkspaceStore.setState(s => ({ groups: s.groups.map(g => ({ ...g, activePath: i % 2 ? path : null, tabs: g.tabs.map(tab => ({ ...tab, name: `title-${i}`, dirty: Boolean(i % 2), externallyModified: Boolean(i % 3), pinned: Boolean(i % 4) })) })) })))
    expect((await getDocument(path)).result).toMatchObject({ content: "unsaved" })
    expect(useSvgPreviewStore.getState().isOpen(path)).toBe(false)
  }
  expect(reads).toBe(1)
})

it("retires a file when its unchanged path becomes a non-file tab", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "disk", size: 4, lineEnding: "lf" } } })
  const path = "/repo/kind.svg"
  useWorkspaceStore.setState({ workspacePath: "/repo", groups: [{ tabs: [{ path, name: "kind.svg", kind: "file", dirty: false, externallyModified: false }], activePath: path }] })
  render(<WorkspaceResourcesBridge />)
  await getDocument(path)
  updateBuffer(path, "unsaved", documentGeneration(path))
  useSvgPreviewStore.getState().toggle(path)
  act(() => useWorkspaceStore.setState(s => ({ groups: s.groups.map(g => ({ ...g, tabs: g.tabs.map(tab => ({ ...tab, kind: "markdown-preview" as const })) })) })))
  expect(useSvgPreviewStore.getState().closedPaths).toEqual({})
  expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
  expect(reads).toBe(2)
})

it("keeps a duplicate file owner until the final file tab disappears", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "disk", size: 4, lineEnding: "lf" } } })
  const path = "/repo/shared.ts", tab = { path, name: "shared.ts", kind: "file" as const, dirty: false, externallyModified: false }
  useWorkspaceStore.setState({ workspacePath: "/repo", groups: [{ tabs: [tab], activePath: path }, { tabs: [{ ...tab }], activePath: path }] })
  render(<WorkspaceResourcesBridge />)
  await getDocument(path)
  updateBuffer(path, "unsaved", documentGeneration(path))
  act(() => useWorkspaceStore.setState(s => ({ groups: [{ ...s.groups[0], tabs: [{ ...tab, kind: "markdown-preview" }] }, s.groups[1]] })))
  expect((await getDocument(path)).result).toMatchObject({ content: "unsaved" })
  expect(reads).toBe(1)
  act(() => useWorkspaceStore.setState(s => ({ groups: [s.groups[0], { tabs: [], activePath: null }] })))
  expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
  expect(reads).toBe(2)
})

it("retires workspace-scoped entries even when every tab reference is unchanged", async () => {
  let reads = 0
  mockIPC(command => { if (command === "open_file") { reads++; return { kind: "full", content: "disk", size: 4, lineEnding: "lf" } } })
  const path = "/repo/sub/shared.ts", groups = [{ tabs: [{ path, name: "shared.ts", kind: "file" as const, dirty: false, externallyModified: false }], activePath: path }]
  useWorkspaceStore.setState({ workspacePath: "/repo", groups })
  render(<WorkspaceResourcesBridge />)
  await getDocument(path)
  updateBuffer(path, "unsaved old workspace", documentGeneration(path))
  act(() => useWorkspaceStore.setState({ workspacePath: "/repo/sub", groups }))
  expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
  act(() => useWorkspaceStore.setState({ workspacePath: "/repo", groups }))
  expect((await getDocument(path)).result).toMatchObject({ content: "disk" })
  expect(reads).toBe(3)
})

it("keeps metadata-only pending reads but invalidates a pending removed owner", async () => {
  const path = "/repo/pending.ts"
  let finish: ((value: unknown) => void) | undefined
  mockIPC(command => command === "open_file" ? new Promise(resolve => { finish = resolve }) : null)
  const tab = { path, name: "pending.ts", kind: "file" as const, dirty: false, externallyModified: false }
  useWorkspaceStore.setState({ workspacePath: "/repo", groups: [{ tabs: [tab], activePath: path }] })
  render(<WorkspaceResourcesBridge />)
  const reading = getDocument(path)
  act(() => useWorkspaceStore.setState(s => ({ groups: s.groups.map(g => ({ ...g, tabs: g.tabs.map(t => ({ ...t, name: "renamed title" })) })) })))
  await Promise.resolve()
  finish!({ kind: "full", content: "kept", size: 4, lineEnding: "lf" })
  expect((await reading).result).toMatchObject({ content: "kept" })
  const other = "/repo/removed.ts"
  act(() => useWorkspaceStore.getState().openTab(other))
  const pending = getDocument(other)
  const rejected = expect(pending).rejects.toThrow("Document workspace changed")
  act(() => useWorkspaceStore.getState().closeTab(0, other))
  await Promise.resolve()
  finish!({ kind: "full", content: "late", size: 4, lineEnding: "lf" })
  await rejected
})
