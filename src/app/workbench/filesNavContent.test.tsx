import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { SearchEvent } from "@/lib/types"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const searchWorkspace = vi.fn(
  (_root: string, _query: string, _cs: boolean, _cb: (e: SearchEvent) => void) => Promise.resolve()
)

vi.mock("@/lib/ipc", () => ({
  searchWorkspace: (...args: Parameters<typeof searchWorkspace>) => searchWorkspace(...args),
  openWorkspace: vi.fn(async (p: string) => p),
  startWatch: vi.fn(async () => undefined),
}))

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))
vi.mock("@/editor/documentRegistry", () => ({ clearAll: vi.fn() }))
vi.mock("@/workbench/FileTree", () => ({ FileTree: () => null }))

const { FilesNavContent } = await import("@/app/workbench/FilesNavContent")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
})

describe("FilesNavContent context menu", () => {
  it("無 workspace 時不開啟空白 explorer 選單", () => {
    render(<FilesNavContent />)
    fireEvent.contextMenu(screen.getByText("No files yet"))
    expect(useContextMenuStore.getState().request).toBeNull()
  })
})

describe("FilesNavContent search entry removed", () => {
  it("不再渲染重複的工作區搜尋 UI（搜尋改由 ⌘K palette 提供）", () => {
    useWorkspaceStore.setState({ workspacePath: "/w" })
    render(<FilesNavContent />)

    // The standalone search box/button are gone — search now lives in the palette.
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
    expect(screen.queryByPlaceholderText("Search in workspace")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /search in workspace/i })).not.toBeInTheDocument()
    expect(searchWorkspace).not.toHaveBeenCalled()
  })
})
