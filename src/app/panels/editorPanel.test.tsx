import { afterEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, within } from "@testing-library/react"

import { AppShell } from "@/app/AppShell"
import { EditorPanel } from "@/app/panels/EditorPanel"
import { useContextMenuStore } from "@/state/contextMenuStore"

vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

afterEach(() => {
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
})

// Files is AppShell's default mode. The project header and terminal drawer
// are siblings of EditorPanel (not children of it), so these render
// AppShell directly rather than EditorPanel in isolation — that's the only
// way to exercise the full "Files mode entry state" surface this task adds.
describe("Files mode entry states", () => {
  it("renders the project header and file tree empty state", () => {
    render(<AppShell />)

    const nav = screen.getByLabelText("Project navigation")
    expect(within(nav).getByText("yuzora")).toBeInTheDocument()
    expect(within(nav).getByText("No files yet")).toBeInTheDocument()
  })

  it("shows the editor surface empty state", () => {
    render(<AppShell />)

    expect(screen.getByText("Open a project to start editing")).toBeInTheDocument()
  })

  it("toggles the preview dock from the tab bar", () => {
    render(<AppShell />)

    const toggle = screen.getByRole("button", { name: "Toggle preview" })
    expect(screen.queryByText("啟動或連接 dev server")).not.toBeInTheDocument()
    expect(toggle).toHaveAttribute("aria-pressed", "false")

    fireEvent.click(toggle)
    expect(screen.getByText("啟動或連接 dev server")).toBeInTheDocument()
    expect(toggle).toHaveAttribute("aria-pressed", "true")

    fireEvent.click(toggle)
    expect(screen.queryByText("啟動或連接 dev server")).not.toBeInTheDocument()
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
    expect(screen.getByText("尚無終端機工作階段")).toBeVisible()

    // Collapsing content only hides the content — the header stays put.
    fireEvent.click(collapseToggle)
    expect(screen.getByText("尚無終端機工作階段")).not.toBeVisible()
    expect(railSwitch).toHaveAttribute("aria-pressed", "true")
    const expandToggle = screen.getByRole("button", { name: "Expand terminal" })
    expect(expandToggle).toHaveAttribute("aria-expanded", "false")

    // Only the rail switch fully hides the panel again.
    fireEvent.click(railSwitch)
    expect(railSwitch).toHaveAttribute("aria-pressed", "false")
    expect(screen.queryByRole("button", { name: "Expand terminal" })).not.toBeInTheDocument()
  })

  it("右鍵編輯區開啟 editor 選單", () => {
    render(<EditorPanel previewOpen={false} />)
    fireEvent.contextMenu(screen.getByText("Open a project to start editing"))
    expect(useContextMenuStore.getState().kind).toBe("editor")
  })
})
