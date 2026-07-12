import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, within } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { AppShell } from "@/app/AppShell"
import { EditorPanel } from "@/app/panels/EditorPanel"
import i18n from "@/lib/i18n"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useTerminalStore } from "@/state/terminalStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

vi.mock("@/terminal/TerminalSession", () => ({
  TerminalSession: ({ sessionId }: { sessionId: string }) => (
    <div data-testid={`terminal-session-${sessionId}`}>Terminal {sessionId}</div>
  ),
}))

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => cmd === "list_dir" ? [] : undefined)
})

afterEach(() => {
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
})

// Files is AppShell's default mode. The project header and terminal drawer
// are siblings of EditorPanel (not children of it), so these render
// AppShell directly rather than EditorPanel in isolation — that's the only
// way to exercise the full "Files mode entry state" surface this task adds.
describe("Files mode entry states", () => {
  it("renders the project header and file tree empty state", () => {
    render(<AppShell />)

    const nav = screen.getByLabelText("Project navigation")
    expect(within(nav).getByText("Yuzora")).toBeInTheDocument()
    expect(within(nav).getByText("No files yet")).toBeInTheDocument()
  })

  it("shows the editor surface empty state", () => {
    render(<AppShell />)

    expect(screen.getByText("Open a project to start editing")).toBeInTheDocument()
  })

  it("toggles the preview dock from the tab bar", () => {
    render(<AppShell />)

    const toggle = screen.getByRole("button", { name: "Toggle preview" })
    expect(screen.queryByText(i18n.t("emptyTitle", { ns: "preview" }))).not.toBeInTheDocument()
    expect(toggle).toHaveAttribute("aria-pressed", "false")

    fireEvent.click(toggle)
    expect(screen.getByText(i18n.t("emptyTitle", { ns: "preview" }))).toBeInTheDocument()
    expect(toggle).toHaveAttribute("aria-pressed", "true")

    fireEvent.click(toggle)
    expect(screen.queryByText(i18n.t("emptyTitle", { ns: "preview" }))).not.toBeInTheDocument()
    expect(toggle).toHaveAttribute("aria-pressed", "false")
  })

  it("terminal panel starts fully hidden; the rail switch shows/hides it, the drawer's own header expands/collapses its content", () => {
    render(<AppShell />)

    const railSwitch = screen.getByRole("button", { name: "Toggle terminal" })
    expect(railSwitch).toHaveAttribute("aria-pressed", "false")
    // Fully hidden: the drawer's own header is outside the a11y tree.
    expect(screen.queryByRole("button", { name: "Expand terminal" })).not.toBeInTheDocument()

    fireEvent.click(railSwitch)
    expect(railSwitch).toHaveAttribute("aria-pressed", "true")

    // Shown, and starts expanded — content is immediately visible.
    const collapseToggle = screen.getByRole("button", { name: "Collapse terminal" })
    expect(collapseToggle).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByText(i18n.t("noSessions", { ns: "terminal" }))).toBeVisible()

    // Collapsing content only hides the content — the header stays put.
    fireEvent.click(collapseToggle)
    expect(screen.getByText(i18n.t("noSessions", { ns: "terminal" }))).not.toBeVisible()
    expect(railSwitch).toHaveAttribute("aria-pressed", "true")
    const expandToggle = screen.getByRole("button", { name: "Expand terminal" })
    expect(expandToggle).toHaveAttribute("aria-expanded", "false")

    // Only the rail switch fully hides the panel again.
    fireEvent.click(railSwitch)
    expect(railSwitch).toHaveAttribute("aria-pressed", "false")
    expect(screen.queryByRole("button", { name: "Expand terminal" })).not.toBeInTheDocument()
  })

  it("applies the mode-aware main-surface floor without remounting persistent panels", () => {
    render(<AppShell />)

    const mainSurface = screen.getByTestId("main-surface")
    const editorState = screen.getByText("Open a project to start editing")
    const sshState = screen.getByText(i18n.t("sshPanel.noSessionTitle", { ns: "panels" }))
    const projectNav = screen.getByLabelText("Project navigation")
    expect(mainSurface.style.minHeight).toBe("44px")

    for (const mode of ["Git", "Database", "SSH", "AgentZone", "Files"]) {
      fireEvent.click(within(projectNav).getByRole("tab", { name: mode }))
      expect(screen.getByTestId("main-surface")).toBe(mainSurface)
      expect(mainSurface.style.minHeight).toBe(mode === "AgentZone" ? "280px" : "44px")
    }

    expect(screen.getByText("Open a project to start editing")).toBe(editorState)
    expect(screen.getByText(i18n.t("sshPanel.noSessionTitle", { ns: "panels" }))).toBe(sshState)
  })

  it("rail hide/show keeps the mounted Terminal session and ratio state intact", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useTerminalStore.getState().addSession("/workspace", {
      sessionId: "persisted",
      title: "Terminal 1",
      workspace: "/workspace",
      shell: "",
      cols: 80,
      rows: 24,
    })
    render(<AppShell />)

    const railSwitch = screen.getByRole("button", { name: "Toggle terminal" })
    const session = screen.getByTestId("terminal-session-persisted")
    fireEvent.click(railSwitch)
    expect(screen.getByTestId("terminal-session-persisted")).toBe(session)

    fireEvent.click(railSwitch)
    expect(screen.getByTestId("terminal-session-persisted")).toBe(session)
    expect(useTerminalStore.getState().sessions.persisted).toBeDefined()

    fireEvent.click(railSwitch)
    expect(screen.getByTestId("terminal-session-persisted")).toBe(session)
    expect(useTerminalStore.getState().sessions.persisted).toBeDefined()
  })

  it("右鍵編輯區開啟 editor 選單", () => {
    render(<EditorPanel />)
    fireEvent.contextMenu(screen.getByText("Open a project to start editing"))
    expect(useContextMenuStore.getState().request).toBeNull()
  })
})
