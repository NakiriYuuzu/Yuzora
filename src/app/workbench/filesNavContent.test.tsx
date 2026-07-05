import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

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
  logUserAction: vi.fn(async () => undefined),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))
vi.mock("@/editor/documentRegistry", () => ({ clearAll: vi.fn() }))
vi.mock("@/workbench/FileTree", () => ({ FileTree: () => null }))

const { FilesNavContent } = await import("@/app/workbench/FilesNavContent")

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
  useWorkspaceStore.setState({ workspacePath: null })
})

describe("FilesNavContent context menu", () => {
  it("右鍵檔案面板（空狀態）開啟 explorer 選單", () => {
    render(<FilesNavContent />)
    fireEvent.contextMenu(screen.getByText("No files yet"))
    expect(useContextMenuStore.getState().kind).toBe("explorer")
  })
})

describe("FilesNavContent search", () => {
  it("clearing the query cancels the running search via an empty search (m6)", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    render(<FilesNavContent />)
    const input = screen.getByPlaceholderText("Search in workspace")
    fireEvent.change(input, { target: { value: "foo" } })
    await vi.advanceTimersByTimeAsync(250)
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "foo", false, expect.any(Function))
    searchWorkspace.mockClear()
    // Clearing the box fires an empty search to bump the Rust generation, which
    // stops the still-running query (front-end cancellation, no new command).
    fireEvent.change(input, { target: { value: "" } })
    expect(searchWorkspace).toHaveBeenCalledWith("/w", "", false, expect.any(Function))
    vi.useRealTimers()
  })

  it("switching to a new non-empty query clears old results immediately (T18)", async () => {
    vi.useFakeTimers()
    useWorkspaceStore.setState({ workspacePath: "/w" })
    searchWorkspace.mockImplementation((_r, _q, _cs, cb) => {
      cb({ type: "match", path: "/w/src/a.ts", matches: [{ line: 1, col: 0, preview: "foo" }] })
      cb({ type: "done", truncated: false, fileCount: 1 })
      return Promise.resolve()
    })
    render(<FilesNavContent />)
    const input = screen.getByPlaceholderText("Search in workspace")
    fireEvent.change(input, { target: { value: "foo" } })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(250)
    })
    expect(screen.getByText("a.ts")).toBeInTheDocument()
    // Typing a new query must clear old results at once, before the 250ms debounce.
    fireEvent.change(input, { target: { value: "bar" } })
    expect(screen.queryByText("a.ts")).not.toBeInTheDocument()
    vi.useRealTimers()
  })
})
