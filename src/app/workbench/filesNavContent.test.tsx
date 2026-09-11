import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { useContextMenuStore } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const search = vi.hoisted(() => ({ files: [] as { name: string; path: string; isDir: boolean }[], loading: false, incomplete: false, error: null as string | null }))
const searchHook = vi.hoisted(() => vi.fn(() => search))
vi.mock("@/workbench/search/useFileNameSearch", () => ({ useFileNameSearch: searchHook }))
vi.mock("@/lib/ipc", () => ({ openWorkspace: vi.fn(async (p: string) => p), startWatch: vi.fn(async () => undefined) }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))
vi.mock("@/editor/documentRegistry", () => ({ clearAll: vi.fn() }))
vi.mock("@/workbench/FileTree", () => ({ FileTree: () => <div data-testid="file-tree">File tree</div> }))

const { FilesNavContent } = await import("@/app/workbench/FilesNavContent")

beforeEach(() => {
  Object.assign(search, { files: [], loading: false, incomplete: false, error: null })
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.restoreAllMocks()
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
})

describe("FilesNavContent", () => {
  it("keeps the empty-workspace hint without duplicating the card toolbar", () => {
    render(<FilesNavContent />)
    fireEvent.contextMenu(screen.getByText("No files yet"))
    expect(useContextMenuStore.getState().request).toBeNull()
    expect(screen.getByText("Select a Space in the sidebar, or use its + button to open a workspace.")).toBeInTheDocument()
    expect(screen.queryByRole("toolbar")).not.toBeInTheDocument()
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Open workspace" })).not.toBeInTheDocument()
  })

  it("preserves the file tree DOM while entering and clearing a filename filter", () => {
    useWorkspaceStore.setState({ workspacePath: "/w" })
    const { rerender } = render(<FilesNavContent />)
    const tree = screen.getByTestId("file-tree")
    const scrollArea = tree.closest('[data-slot="scroll-area"]')
    rerender(<FilesNavContent filterQuery="readme" />)
    expect(screen.getByTestId("file-tree")).toBe(tree)
    expect(scrollArea).toHaveAttribute("inert")
    expect(scrollArea).toHaveClass("hidden")
    rerender(<FilesNavContent />)
    expect(screen.getByTestId("file-tree")).toBe(tree)
    expect(scrollArea).not.toHaveAttribute("inert")
    expect(scrollArea).not.toHaveClass("hidden")
  })

  it("opens a matching file and offers its precise file context menu", () => {
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0 })
    const openTab = vi.spyOn(useWorkspaceStore.getState(), "openTab").mockImplementation(() => {})
    search.files = [{ name: "readme.md", path: "/w/docs/readme.md", isDir: false }]
    render(<FilesNavContent filterQuery="readme" />)
    const result = screen.getByRole("button", { name: "docs/readme.md" })
    fireEvent.click(result)
    expect(openTab).toHaveBeenCalledWith("/w/docs/readme.md")
    fireEvent.contextMenu(result)
    expect(useContextMenuStore.getState().request).toMatchObject({ kind: "file", workspacePath: "/w", path: "/w/docs/readme.md", isDirectory: false, sourceGroupIndex: 0 })
  })

  it("passes active scope/revision to search and distinguishes incomplete results from no matches", () => {
    useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 27 })
    search.incomplete = true
    const { rerender } = render(<FilesNavContent filterQuery="xyz" active={false} />)
    expect(searchHook).toHaveBeenCalledWith("/w", "xyz", 27, false)
    expect(screen.getByRole("alert")).toHaveTextContent("Some folders could not be searched")
    expect(screen.getByRole("status")).toHaveTextContent("No matching files")
    search.loading = true
    rerender(<FilesNavContent filterQuery="xyz" />)
    expect(screen.getByRole("status")).toHaveTextContent("Finding files")
  })

  it("reports workspace access failure without claiming no matches", () => {
    useWorkspaceStore.setState({ workspacePath: "/w" })
    search.error = "permission denied"
    render(<FilesNavContent filterQuery="xyz" />)
    expect(screen.getByRole("alert")).toHaveTextContent("Could not search this workspace")
    expect(screen.queryByText("No matching files")).not.toBeInTheDocument()
  })
})
