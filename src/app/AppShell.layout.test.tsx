import { useEffect } from "react"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { AppShell } from "./AppShell"
import { saveAppearanceSettings } from "@/app/workbench/settingsStorage"
import { uiInitialState, useUiStore } from "@/state/uiStore"

const lifecycle = vi.hoisted(() => ({ editorMount: vi.fn(), editorUnmount: vi.fn(), spacesMount: vi.fn(), spacesUnmount: vi.fn(), editorRender: vi.fn(), spacesRender: vi.fn(), toolsRender: vi.fn(), settingsRender: vi.fn(), remote: vi.fn() }))
vi.mock("@/lib/platform", () => ({ isTauri: () => false, showsNativeTrafficLights: () => false, isWindowsPlatform: () => false, isMacPlatform: () => false, shortcutLabel: () => "Ctrl+K" }))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))
vi.mock("@/lib/unsavedGuard", () => ({ confirmDiscardingUnsaved: vi.fn() }))
vi.mock("@/state/sftpStore", () => ({ useSftpStore: { getState: () => ({ setPanelOpen: lifecycle.remote }) } }))
vi.mock("@/app/workbench/settingsStorage", () => ({ loadAppearanceSettings: () => ({ theme: "light", accent: "lime", leftSidebarBackground: true, rightSidebarBackground: true, botAnimations: true }), saveAppearanceSettings: vi.fn() }))
vi.mock("@/app/panels/EditorPanel", () => ({ EditorPanel: () => { lifecycle.editorRender(); useEffect(() => { lifecycle.editorMount(); return lifecycle.editorUnmount }, []); return <input aria-label="Editor buffer" defaultValue="unsaved draft" /> } }))
vi.mock("@/app/workbench/SpaceAgentSidebar", () => ({ SpaceAgentSidebar: () => { lifecycle.spacesRender(); useEffect(() => { lifecycle.spacesMount(); return lifecycle.spacesUnmount }, []); return <button>Space leaf</button> } }))
vi.mock("@/app/panels/GitPanel", () => ({ GitPanel: () => <div>Git graph surface</div> }))
vi.mock("@/app/panels/DatabasePanel", () => ({ DatabasePanel: () => <div>Database query surface</div> }))
vi.mock("@/app/workbench/DatabaseNavContent", () => ({ DatabaseNavContent: () => null }))
vi.mock("@/app/workbench/WorkspaceToolsPanel", () => ({ WorkspaceToolsPanel: ({ onOpenGraph }: { onOpenGraph: () => void }) => { lifecycle.toolsRender(); return <button onClick={onOpenGraph}>Tool leaf</button> } }))
vi.mock("@/app/workbench/SettingsDialog", () => ({ SettingsDialog: ({ open, botAnimations, onBotAnimationsChange }: { open: boolean; botAnimations: boolean; onBotAnimationsChange: (enabled: boolean) => void }) => { lifecycle.settingsRender(); return open ? <div role="dialog" aria-label="Settings dialog"><button onClick={() => onBotAnimationsChange(!botAnimations)}>Toggle bot animations</button></div> : null } }))
vi.mock("@/app/workbench/CommandPalette", () => ({ CommandPalette: ({ open }: { open: boolean }) => open ? <div role="dialog" aria-label="Command search" /> : null }))
vi.mock("@/app/workbench/ContextMenu", () => ({ ContextMenu: () => null }))
vi.mock("@/workbench/git/DiffModal", () => ({ DiffModal: () => null }))
vi.mock("@/app/workbench/ProjectEditorPopover", () => ({ ProjectEditorPopover: () => null }))
vi.mock("@/app/workbench/StatusBar", () => ({ StatusBar: () => null }))

function resize(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width })
  fireEvent(window, new Event("resize"))
}
function leftToggle() { return document.querySelector<HTMLButtonElement>('button[aria-controls="workbench-spaces"]')! }
function rightToggle() { return document.querySelector<HTMLButtonElement>('button[aria-controls="workbench-tools"]')! }
beforeEach(() => {
  vi.clearAllMocks()
  useUiStore.setState({ ...uiInitialState, mode: "ade" })
  resize(1440)
})
afterEach(cleanup)

it("applies and saves the global bot animation switch without remounting work surfaces", () => {
  const app = render(<AppShell />)
  const editor = screen.getByRole("textbox", { name: "Editor buffer" })
  expect(document.documentElement.dataset.botAnimations).toBe("true")
  fireEvent.click(screen.getByRole("button", { name: "Settings" }))
  fireEvent.click(screen.getByRole("button", { name: "Toggle bot animations" }))
  expect(document.documentElement.dataset.botAnimations).toBe("false")
  expect(saveAppearanceSettings).toHaveBeenLastCalledWith(expect.objectContaining({ botAnimations: false }))
  expect(screen.getByRole("textbox", { name: "Editor buffer" })).toBe(editor)
  expect(lifecycle.editorMount).toHaveBeenCalledTimes(1)
  expect(lifecycle.spacesMount).toHaveBeenCalledTimes(1)
  fireEvent.click(screen.getByRole("button", { name: "Toggle bot animations" }))
  expect(document.documentElement.dataset.botAnimations).toBe("true")
  app.unmount()
  expect(document.documentElement.dataset.botAnimations).toBeUndefined()
})

it("does not rerender unrelated work surfaces when toggling or resizing sidebars", () => {
  render(<AppShell />)
  const renders = [lifecycle.editorRender, lifecycle.spacesRender, lifecycle.toolsRender, lifecycle.settingsRender]
  renders.forEach(render => render.mockClear())
  for (let i = 0; i < 3; i++) {
    fireEvent.click(leftToggle())
    fireEvent.click(rightToggle())
  }
  fireEvent.click(leftToggle())
  fireEvent.click(rightToggle())
  fireEvent.keyDown(screen.getByRole("separator", { name: "Resize Spaces and Agents sidebar" }), { key: "ArrowRight" })
  fireEvent.keyDown(screen.getByRole("separator", { name: "Resize workspace tools sidebar" }), { key: "ArrowLeft" })
  renders.forEach(render => expect(render).not.toHaveBeenCalled())
  fireEvent.click(screen.getByRole("button", { name: "Settings" }))
  expect(screen.getByRole("dialog", { name: "Settings dialog" })).toBeVisible()
})

it("keeps edge controls reachable while collapsed sidebars give back their full width", () => {
  render(<AppShell />)
  const left = screen.getByRole("complementary", { name: "Sidebar navigation" })
  const right = screen.getByRole("complementary", { name: "Workspace tools" })
  expect(screen.queryByRole("banner")).not.toBeInTheDocument()
  expect(within(left).getByText("Yuzora")).toBeInTheDocument()
  const leftControl = leftToggle()
  const rightControl = rightToggle()
  const editor = screen.getByRole("textbox", { name: "Editor buffer" })
  fireEvent.click(leftToggle())
  fireEvent.click(rightToggle())
  expect(left).toHaveAttribute("data-collapsed", "true")
  expect(right).toHaveAttribute("data-collapsed", "true")
  expect(left).toHaveStyle({ width: "0px" })
  expect(right).toHaveStyle({ width: "0px" })
  expect(leftToggle()).toBe(leftControl)
  expect(rightToggle()).toBe(rightControl)
  expect(leftToggle()).toBeVisible()
  expect(rightToggle()).toBeVisible()
  expect(leftToggle().closest("[inert]")).toBeNull()
  expect(rightToggle().closest("[inert]")).toBeNull()
  expect(screen.queryByRole("separator", { name: /Resize .* sidebar/ })).not.toBeInTheDocument()
  for (const handle of document.querySelectorAll('.workbench-resize-handle')) {
    expect(handle).toHaveAttribute("tabindex", "-1")
    expect(handle).toHaveAttribute("inert")
  }
  expect(screen.queryByRole("button", { name: "Space leaf" })).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Tool leaf" })).not.toBeInTheDocument()
  expect(screen.getByRole("textbox", { name: "Editor buffer" })).toBe(editor)
  fireEvent.click(leftToggle())
  fireEvent.click(rightToggle())
  expect(screen.getByRole("button", { name: "Space leaf" })).toBeVisible()
  expect(screen.getByRole("button", { name: "Tool leaf" })).toBeVisible()
})

it("ends resizing after pointer cancellation or lost capture, then restores the chosen widths", () => {
  render(<AppShell />)
  const body = document.querySelector(".workbench-body")!
  for (const [name, side, toggle, stop] of [
    ["Resize Spaces and Agents sidebar", "spaces", leftToggle, fireEvent.pointerCancel],
    ["Resize workspace tools sidebar", "tools", rightToggle, fireEvent.lostPointerCapture],
  ] as const) {
    const handle = screen.getByRole("separator", { name })
    fireEvent.keyDown(handle, { key: "End" })
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 100 })
    expect(body).toHaveAttribute("data-resizing", side)
    stop(handle, { pointerId: 1 })
    expect(body).not.toHaveAttribute("data-resizing")
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 200 })
    expect(handle).toHaveAttribute("aria-valuenow", "420")
    fireEvent.click(toggle())
    fireEvent.click(toggle())
    expect(handle).toHaveAttribute("aria-valuenow", "420")
    expect(document.getElementById(`workbench-${side}`)).toHaveStyle({ width: "420px" })
    expect(toggle()).toHaveFocus()
  }
})

it("preserves editor buffers and Space navigation mounts across all work surfaces", () => {
  render(<AppShell />)
  const editor = screen.getByRole("textbox", { name: "Editor buffer" })
  const space = screen.getByRole("button", { name: "Space leaf" })
  fireEvent.change(editor, { target: { value: "working draft" } })
  for (const mode of ["files", "git", "database", "ade"] as const) {
    act(() => useUiStore.getState().setMode(mode))
    expect(screen.getByRole("button", { name: "Space leaf" })).toBe(space)
    expect(editor).toBeInTheDocument()
    expect(editor).toHaveValue("working draft")
  }
  expect(lifecycle.editorMount).toHaveBeenCalledTimes(1)
  expect(lifecycle.spacesMount).toHaveBeenCalledTimes(1)
  expect(lifecycle.editorUnmount).not.toHaveBeenCalled()
  expect(lifecycle.spacesUnmount).not.toHaveBeenCalled()
})

it("keeps named shared Settings, SSH/SFTP, search and Database entries reachable", () => {
  render(<AppShell />)
  const sidebar = within(screen.getByRole("complementary", { name: "Sidebar navigation" }))
  expect(screen.queryByRole("banner")).not.toBeInTheDocument()
  for (const name of ["Settings", "SSH / SFTP", "Search files and commands", "Database"]) {
    expect(sidebar.getByRole("button", { name })).toBeVisible()
  }
  fireEvent.click(screen.getByRole("button", { name: "Settings" }))
  expect(screen.getByRole("dialog", { name: "Settings dialog" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "SSH / SFTP" }))
  expect(lifecycle.remote).toHaveBeenCalledWith(true)
  fireEvent.click(screen.getByRole("button", { name: "Search files and commands" }))
  expect(screen.getByRole("dialog", { name: "Command search" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Database" }))
  expect(useUiStore.getState().mode).toBe("database")
  expect(screen.getByText("Database query surface")).toBeVisible()
  expect(screen.getByRole("button", { name: "Database" })).toHaveAttribute("aria-pressed", "true")
  fireEvent.click(screen.getByRole("button", { name: "Back to workspace" }))
  expect(useUiStore.getState().mode).toBe("ade")
})

it("restores shared navigation after collapsing and reopening the sidebar", () => {
  render(<AppShell />)
  const search = screen.getByRole("button", { name: "Search files and commands" })
  fireEvent.click(leftToggle())
  expect(screen.queryByRole("button", { name: "Search files and commands" })).not.toBeInTheDocument()
  expect(leftToggle()).toHaveFocus()
  fireEvent.click(leftToggle())
  expect(screen.getByRole("button", { name: "Search files and commands" })).toBe(search)
  fireEvent.click(search)
  expect(screen.getByRole("dialog", { name: "Command search" })).toBeVisible()
})

it("offers bounded keyboard resizing in the correct direction on both edges", () => {
  render(<AppShell />)
  const left = screen.getByRole("separator", { name: "Resize Spaces and Agents sidebar" })
  const right = screen.getByRole("separator", { name: "Resize workspace tools sidebar" })
  for (const [handle, minimum, maximum, grow, shrink] of [[left, 256, 420, "ArrowRight", "ArrowLeft"], [right, 224, 420, "ArrowLeft", "ArrowRight"]] as const) {
    fireEvent.keyDown(handle, { key: "Home" })
    expect(handle).toHaveAttribute("aria-valuenow", String(minimum))
    fireEvent.keyDown(handle, { key: shrink })
    expect(handle).toHaveAttribute("aria-valuenow", String(minimum))
    fireEvent.keyDown(handle, { key: grow })
    expect(handle).toHaveAttribute("aria-valuenow", String(minimum + 16))
    fireEvent.keyDown(handle, { key: "End" })
    fireEvent.keyDown(handle, { key: grow })
    expect(handle).toHaveAttribute("aria-valuenow", String(maximum))
  }
})

it("returns focus after manual panel close and keeps narrow layouts mutually exclusive", () => {
  render(<AppShell />)
  fireEvent.click(rightToggle())
  expect(rightToggle()).toHaveFocus()
  expect(rightToggle()).toHaveAttribute("aria-expanded", "false")
  resize(800)
  expect(leftToggle()).toHaveAttribute("aria-expanded", "false")
  fireEvent.click(leftToggle())
  expect(leftToggle()).toHaveFocus()
  expect(leftToggle()).toHaveAttribute("aria-expanded", "true")
  fireEvent.click(rightToggle())
  expect(rightToggle()).toHaveFocus()
  expect(rightToggle()).toHaveAttribute("aria-expanded", "true")
  expect(leftToggle()).toHaveAttribute("aria-expanded", "false")
})

it("moves focus out of sidebars before automatic narrow-window collapse", () => {
  render(<AppShell />)
  screen.getByRole("button", { name: "Tool leaf" }).focus()
  resize(1100)
  expect(rightToggle()).toHaveFocus()
  screen.getByRole("button", { name: "Space leaf" }).focus()
  resize(800)
  expect(leftToggle()).toHaveFocus()
})

it("hides workspace tools in Database and restores the working surface from its toggle", () => {
  render(<AppShell />)
  const tool = screen.getByRole("button", { name: "Tool leaf" })
  fireEvent.click(screen.getByRole("button", { name: "Database" }))
  expect(document.getElementById("workbench-tools-content")).toHaveAttribute("aria-hidden", "true")
  expect(tool).toBeInTheDocument()
  expect(rightToggle()).toHaveAttribute("aria-expanded", "false")
  fireEvent.click(rightToggle())
  expect(["ade", "files"]).toContain(useUiStore.getState().mode)
  expect(rightToggle()).toHaveAttribute("aria-expanded", "true")
  expect(rightToggle()).toHaveFocus()
  expect(screen.getByRole("button", { name: "Tool leaf" })).toBe(tool)
})

it("opens commit history from the graph entry after viewing local changes", () => {
  useUiStore.setState({ mode: "files", gitPanelTab: "local" })
  render(<AppShell />)
  fireEvent.click(screen.getByRole("button", { name: "Tool leaf" }))
  expect(useUiStore.getState().mode).toBe("git")
  expect(useUiStore.getState().gitPanelTab).toBe("log")
  expect(screen.getByText("Git graph surface")).toBeVisible()
})
