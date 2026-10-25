import { afterEach, beforeEach, expect, it } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { ContextMenu } from "@/app/workbench/ContextMenu"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useFileClipboardStore } from "@/state/fileClipboardStore"
import { useFileTreeStore } from "@/state/fileTreeStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { FileTree } from "./FileTree"

let calls: Array<{ command: string; args: unknown }>

beforeEach(() => {
    calls = []
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useFileClipboardStore.setState({ selection: null, clipboard: null })
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useGitStore.setState(initialGitState)
    useWorkspaceStore.setState({
        workspacePath: "/w", workspaceCapabilityId: "tree109-unit", treeRevision: 0,
        groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0,
    })
    mockIPC((command, args) => {
        calls.push({ command, args })
        if (command === "list_dir") return [
            { name: "A.txt", path: "/w/A.txt", isDir: false },
            { name: "B.txt", path: "/w/B.txt", isDir: false },
            { name: "folder", path: "/w/folder", isDir: true },
        ]
        return null
    })
})

afterEach(() => {
    clearMocks()
    useFileClipboardStore.setState({ selection: null, clipboard: null })
    useContextMenuStore.setState({ request: null })
})

it.each([
    ["row", "B.txt"], ["editor", "B.txt"],
    ["row", "folder"], ["editor", "folder"],
])("right-click from %s focuses %s for clipboard events after Escape", async (origin, target) => {
    render(<>
        <ScrollArea focusable><FileTree /></ScrollArea>
        <textarea aria-label="previous editor" />
        <ContextMenu />
    </>)
    const first = (await screen.findByText("A.txt")).closest("button")!
    const row = screen.getByText(target).closest("button")!
    if (origin === "row") fireEvent.click(first)
    else screen.getByRole("textbox", { name: "previous editor" }).focus()
    const tabs = useWorkspaceStore.getState().groups[0].tabs
    fireEvent.contextMenu(row)
    expect(useFileClipboardStore.getState().selection?.path).toBe(`/w/${target}`)
    expect(document.activeElement).toBe(row)
    expect(useWorkspaceStore.getState().groups[0].tabs).toBe(tabs)
    expect(useFileTreeStore.getState().trees["/w"].expandedDirs.size).toBe(0)
    expect(calls.filter(call => call.command === "list_dir")).toHaveLength(1)
    expect(screen.getByRole("menu")).toBeTruthy()

    // Route Escape from actual focus, then the document clipboard event used
    // by the native macOS Edit menu. Do not send a key directly to the target row.
    fireEvent.keyDown(document.activeElement!, { key: "Escape" })
    expect(useContextMenuStore.getState().request).toBeNull()
    const event = new Event("cut", { bubbles: true, cancelable: true })
    document.body.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(true)
    expect(useFileClipboardStore.getState().clipboard).toEqual({
        workspacePath: "/w", paths: [`/w/${target}`], mode: "cut",
    })
    await waitFor(() => expect(calls).toContainEqual({
        command: "clipboard_write_workspace_files",
        args: { workspaceCapabilityId: "tree109-unit", paths: [target] },
    }))
})
