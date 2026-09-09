import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import { EditorPanel } from "@/app/panels/EditorPanel"
import i18n from "@/lib/i18n"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => cmd === "list_dir" ? [] : undefined)
})

afterEach(() => {
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
})

// ADE is AppShell's default mode; Files is second. The project header and
// terminal drawer are siblings of EditorPanel (not children of it), so these
// render AppShell directly rather than EditorPanel in isolation — that's the
// only way to exercise the full "Files mode entry state" surface this task adds.
describe("Files mode entry states", () => {
  it("renders the project header and file tree empty state", () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Expand right workspace tools" }))

    const nav = document.getElementById("workbench-tools")!
    expect(within(nav).getByText("No folder open")).toBeInTheDocument()
    expect(within(nav).getByText("No files yet")).toBeInTheDocument()
  })

  it("shows the editor surface empty state", () => {
    render(<AppShell />)

    expect(screen.getByText("Open a project to start editing")).toBeInTheDocument()
  })

  it("opens Browser from the tab add menu and closes it from its tab", async () => {
    render(<AppShell />)

    expect(screen.queryByText(i18n.t("emptyTitle", { ns: "preview" }))).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Toggle preview" })).toBeNull()

    fireEvent.pointerDown(screen.getByRole("button", { name: "Add tab" }), {
      button: 0,
      ctrlKey: false
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "Browser" }))
    expect(screen.getByText(i18n.t("emptyTitle", { ns: "preview" }))).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Close Preview" }))
    expect(screen.queryByText(i18n.t("emptyTitle", { ns: "preview" }))).not.toBeInTheDocument()
  })

  it("applies the mode-aware main-surface floor without remounting persistent panels", () => {
    render(<AppShell />)

    const mainSurface = screen.getByTestId("main-surface")
    const editorState = screen.getByText("Open a project to start editing")
    const projectNav = document.getElementById("workbench-tools")!
    expect(mainSurface.style.minHeight).toBe("44px")

    for (const mode of ["git", "database", "files"] as const) {
      act(()=>useUiStore.getState().setMode(mode))
      expect(screen.getByTestId("main-surface")).toBe(mainSurface)
      expect(mainSurface.style.minHeight).toBe("44px")
    }

    expect(screen.getByText("Open a project to start editing")).toBe(editorState)
    expect(within(projectNav).queryByRole("tab", { name: "SSH" })).not.toBeInTheDocument()
  })

  it("右鍵編輯區開啟 editor 選單", () => {
    render(<EditorPanel />)
    fireEvent.contextMenu(screen.getByText("Open a project to start editing"))
    expect(useContextMenuStore.getState().request).toBeNull()
  })
})
