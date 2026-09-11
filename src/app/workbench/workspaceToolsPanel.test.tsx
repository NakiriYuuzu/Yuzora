import { beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
const actions = vi.hoisted(() => ({ create: vi.fn(), run: vi.fn(), revealAllowed: true, refresh: vi.fn(async () => {}), branches: vi.fn(async () => {}) }))
vi.mock("@/state/contextMenuStore", () => ({ executeLegacyContextMenuAction: actions.create, runContextMenuAction: actions.run }))
vi.mock("./contextMenuDefs", () => ({ commandFor: (_request: unknown, id: string) => ({ id, availability: () => ({ visible: true, enabled: id !== "cmReveal" || actions.revealAllowed }) }) }))
vi.mock("@/state/gitStore", () => ({ useGitStore: { getState: () => ({ refresh: actions.refresh, loadBranches: actions.branches }) } }))
vi.mock("@/workbench/WorkspaceHostBadge", () => ({ WorkspaceHostBadge: () => <span>Local host</span> }))
vi.mock("./FilesNavContent", () => ({ FilesNavContent: ({ filterQuery, active }: { filterQuery: string; active: boolean }) => <div data-testid="files-pane" data-active={active}>{filterQuery}</div> }))
vi.mock("./GitNavContent", () => ({ GitNavContent: ({ filterQuery }: { filterQuery: string }) => <div data-testid="git-pane">{filterQuery}</div> }))
import { WorkspaceToolsPanel } from "./WorkspaceToolsPanel"
import { useWorkspaceStore } from "@/state/workspaceStore"
import i18n from "@/lib/i18n"

beforeEach(async () => {
  cleanup()
  vi.clearAllMocks()
  vi.restoreAllMocks()
  actions.revealAllowed = true
  actions.run.mockImplementation((request, command) => actions.create(request, command.id))
  await i18n.changeLanguage("en")
  useWorkspaceStore.setState({ workspacePath: "/projects/NetZero-Tough" })
})

it("keeps per-tab filename queries and clears them on a workspace change without unmounting panes", () => {
  const props = { onToolChange: vi.fn() }
  const view = render(<WorkspaceToolsPanel {...props} tool="files" />)
  const files = screen.getByTestId("files-pane")
  const git = screen.getByTestId("git-pane")
  expect(screen.getByText("NetZero-Tough")).toHaveAttribute("title", "/projects/NetZero-Tough")
  fireEvent.change(screen.getByRole("textbox", { name: "Find files" }), { target: { value: "readme" } })
  view.rerender(<WorkspaceToolsPanel {...props} tool="git" />)
  expect(screen.getByRole("textbox")).toHaveValue("")
  expect(files.closest('[role="tabpanel"]')).toHaveAttribute("hidden")
  expect(files.closest('[role="tabpanel"]')).toHaveAttribute("inert")
  expect(files).toHaveAttribute("data-active", "false")
  fireEvent.change(screen.getByRole("textbox"), { target: { value: "config" } })
  view.rerender(<WorkspaceToolsPanel {...props} tool="files" />)
  expect(screen.getByRole("textbox")).toHaveValue("readme")
  expect(screen.getByTestId("files-pane")).toBe(files)
  expect(screen.getByTestId("git-pane")).toBe(git)
  expect(git).toHaveTextContent("config")
  act(() => useWorkspaceStore.setState({ workspacePath: "/another" }))
  expect(screen.getByRole("textbox")).toHaveValue("")
  expect(git).toHaveTextContent("")
})

it("routes header actions and the more menu to existing operations", () => {
  render(<WorkspaceToolsPanel tool="files" onToolChange={vi.fn()} />)
  fireEvent.click(screen.getByRole("button", { name: "New file" }))
  fireEvent.click(screen.getByRole("button", { name: "New folder" }))
  expect(actions.create).toHaveBeenNthCalledWith(1, { kind: "explorer", workspacePath: "/projects/NetZero-Tough" }, "cmNewFile")
  expect(actions.create).toHaveBeenNthCalledWith(2, { kind: "explorer", workspacePath: "/projects/NetZero-Tough" }, "cmNewFolder")
  const revision = useWorkspaceStore.getState().treeRevision
  fireEvent.click(screen.getByRole("button", { name: "Refresh files" }))
  expect(useWorkspaceStore.getState().treeRevision).toBe(revision + 1)
  fireEvent.pointerDown(screen.getByRole("button", { name: "More workspace tools" }), { button: 0, ctrlKey: false })
  expect(screen.queryByRole("menuitem", { name: "SFTP" })).not.toBeInTheDocument()
  expect(screen.queryByRole("menuitem", { name: "History and branch graph" })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("menuitem", { name: "Copy full folder path" }))
  expect(actions.create).toHaveBeenLastCalledWith({ kind: "file", workspacePath: "/projects/NetZero-Tough", path: "/projects/NetZero-Tough", isDirectory: true, sourceGroupIndex: 0 }, "cmCopyFullPath")
  fireEvent.pointerDown(screen.getByRole("button", { name: "More workspace tools" }), { button: 0, ctrlKey: false })
  fireEvent.click(screen.getByRole("menuitem", { name: "Open in file manager" }))
  expect(actions.create).toHaveBeenLastCalledWith(expect.objectContaining({ path: "/projects/NetZero-Tough", isDirectory: true }), "cmReveal")
})

it("uses uppercase GIT and refreshes status and branches without mutation", () => {
  render(<WorkspaceToolsPanel tool="git" onToolChange={vi.fn()} />)
  expect(screen.getByRole("tab", { name: "GIT" })).toHaveAttribute("aria-selected", "true")
  fireEvent.click(screen.getByRole("button", { name: "Refresh files" }))
  expect(actions.refresh).toHaveBeenCalledOnce()
  expect(actions.branches).toHaveBeenCalledOnce()
})

it("disables workspace actions and search without a folder", () => {
  useWorkspaceStore.setState({ workspacePath: null })
  render(<WorkspaceToolsPanel tool="files" onToolChange={vi.fn()} />)
  for (const name of ["New file", "New folder", "Refresh files"]) expect(screen.getByRole("button", { name })).toBeDisabled()
  expect(screen.getByRole("textbox", { name: "Find files" })).toBeDisabled()
  fireEvent.pointerDown(screen.getByRole("button", { name: "More workspace tools" }), { button: 0, ctrlKey: false })
  expect(screen.getByRole("menuitem", { name: "Copy full folder path" })).toHaveAttribute("aria-disabled", "true")
  expect(screen.getByRole("menuitem", { name: "Open in file manager" })).toHaveAttribute("aria-disabled", "true")
  expect(actions.create).not.toHaveBeenCalled()
})

it.each([["Macintosh", "Open in Finder"], ["Windows", "Open in Explorer"], ["Linux", "Open in file manager"]])("uses platform-specific directory labels on %s", (platform, label) => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(platform)
  render(<WorkspaceToolsPanel tool="files" onToolChange={vi.fn()} />)
  fireEvent.pointerDown(screen.getByRole("button", { name: "More workspace tools" }), { button: 0, ctrlKey: false })
  expect(screen.getByRole("menuitem", { name: label })).toBeInTheDocument()
})

it("keeps copying available when the typed reveal command rejects a remote host", () => {
  actions.revealAllowed = false
  render(<WorkspaceToolsPanel tool="files" onToolChange={vi.fn()} />)
  fireEvent.pointerDown(screen.getByRole("button", { name: "More workspace tools" }), { button: 0, ctrlKey: false })
  expect(screen.getByRole("menuitem", { name: "Open in file manager" })).toHaveAttribute("aria-disabled", "true")
  fireEvent.click(screen.getByRole("menuitem", { name: "Copy full folder path" }))
  expect(actions.create).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "file" }), "cmCopyFullPath")
})
