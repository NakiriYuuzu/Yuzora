import { act, cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
// @ts-expect-error Node types are excluded from the browser tsconfig; Vitest still runs in Node.
import { readFileSync } from "node:fs"
import { afterEach, beforeEach, expect, test, vi } from "vitest"

const fs = vi.hoisted(() => ({ fsMovePaths: vi.fn(async () => [] as string[]) }))
vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    fsMovePaths: fs.fsMovePaths
}))
const dialog = vi.hoisted(() => ({ requestAppConfirmation: vi.fn(async () => true), showAppMessage: vi.fn(async () => undefined) }))
vi.mock("@/state/appDialogStore", () => dialog)
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }))

import { runtimeKey, LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { registerTerminalDropTarget } from "@/terminal/terminalDropTargets"
import { pointerDrag, stubElementFromPoint } from "@/test/pointerDrag"
import { FileTree } from "./FileTree"
import { useFileClipboardStore } from "../state/fileClipboardStore"
import { useFileTreeStore } from "../state/fileTreeStore"
import { useWorkspaceStore } from "../state/workspaceStore"

const TREE: Record<string, Array<{ name: string; path: string; isDir: boolean }>> = {
    "/w": [
        { name: "src", path: "/w/src", isDir: true },
        { name: "other", path: "/w/other", isDir: true },
        { name: "a.txt", path: "/w/a.txt", isDir: false }
    ],
    "/w/src": [
        { name: "inner", path: "/w/src/inner", isDir: true },
        { name: "b.txt", path: "/w/src/b.txt", isDir: false }
    ]
}

let restore: (() => void) | null = null
let hit: Element | null = null
const START = { x: 0, y: 0 }
const MOVE = [{ x: 10, y: 0 }, { x: 12, y: 0 }]

async function mountTree(extra?: React.ReactNode, expand = false) {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") return TREE[(args as { path: string }).path] ?? []
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }, { tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    render(<div data-file-tree-root><FileTree /></div>)
    if (extra) render(<>{extra}</>)
    await waitFor(() => expect(screen.getByText("a.txt")).toBeTruthy())
    if (expand) {
        fireEvent.click(screen.getByText("src"))
        await waitFor(() => expect(screen.getByText("b.txt")).toBeTruthy())
    }
}

// The drag label repeats the name, so rows are found by path rather than by text.
const row = (name: string) =>
    Array.from(document.querySelectorAll<HTMLElement>("button[data-tree-path]")).find((el) => el.dataset.treePath?.endsWith(`/${name}`)) as HTMLElement
const treeRoot = () => document.querySelector("[data-file-tree-root]")

beforeEach(() => {
    dialog.requestAppConfirmation.mockResolvedValue(true)
    fs.fsMovePaths.mockResolvedValue(["/w/src/a.txt"])
    restore = stubElementFromPoint(() => hit)
})
afterEach(() => {
    restore?.()
    hit = null
    cleanup()
    clearMocks()
    vi.clearAllMocks()
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useFileClipboardStore.setState({ clipboard: null, selection: null })
})

test("檔案樹列保留觸控的垂直捲動", async () => {
    await mountTree()
    // touch-action: none on every row would turn each swipe into a file drag.
    expect(row("a.txt")).toHaveAttribute("data-pointer-drag-handle", "pan-y")
    const styles = readFileSync("src/styles.css", "utf8")
    expect(styles).toMatch(/\[data-pointer-drag-handle="pan-y"\]\s*\{\s*touch-action:\s*pan-y;/)
})

test("拖到資料夾列並確認後移動檔案，且不碰 clipboard", async () => {
    await mountTree()
    hit = row("src")
    pointerDrag(row("a.txt"), MOVE, { from: START })
    await waitFor(() => expect(fs.fsMovePaths).toHaveBeenCalled())
    expect(dialog.requestAppConfirmation).toHaveBeenCalledWith(expect.objectContaining({ kind: "warning" }))
    expect(fs.fsMovePaths).toHaveBeenCalledWith("/w", ["/w/a.txt"], "/w/src")
    expect(useFileClipboardStore.getState().clipboard).toBeNull()
})

test("取消確認就不移動", async () => {
    dialog.requestAppConfirmation.mockResolvedValue(false)
    await mountTree()
    hit = row("src")
    pointerDrag(row("a.txt"), MOVE, { from: START })
    await waitFor(() => expect(dialog.requestAppConfirmation).toHaveBeenCalled())
    await act(async () => { await Promise.resolve() })
    expect(fs.fsMovePaths).not.toHaveBeenCalled()
})

test("拖到自己、子孫或目前的父層時沒有 target，也不移動", async () => {
    await mountTree(undefined, true)
    // folder onto itself
    hit = row("src")
    pointerDrag(row("src"), MOVE, { from: START, release: false })
    expect(row("src").getAttribute("data-pointer-drop-target")).toBeNull()
    fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId: 1, clientX: 12, clientY: 0 })
    // file onto its current parent folder
    hit = row("src")
    pointerDrag(row("b.txt"), MOVE, { from: START })
    // file in the root onto the root zone
    hit = treeRoot()
    pointerDrag(row("a.txt"), MOVE, { from: START })
    await act(async () => { await Promise.resolve() })
    expect(dialog.requestAppConfirmation).not.toHaveBeenCalled()
    expect(fs.fsMovePaths).not.toHaveBeenCalled()
})

test("資料夾不能移進自己的子孫", async () => {
    await mountTree(undefined, true)
    hit = row("inner")
    pointerDrag(row("src"), MOVE, { from: START, release: false })
    expect(row("inner").getAttribute("data-pointer-drop-target")).toBeNull()
    fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId: 1, clientX: 12, clientY: 0 })
    await act(async () => { await Promise.resolve() })
    expect(dialog.requestAppConfirmation).not.toHaveBeenCalled()
})

test("拖到樹根區域會移到 workspace 根目錄", async () => {
    await mountTree(undefined, true)
    hit = treeRoot()
    pointerDrag(row("b.txt"), MOVE, { from: START, release: false })
    expect(hit?.getAttribute("data-pointer-drop-target")).toBe("inside")
    fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId: 1, clientX: 12, clientY: 0 })
    await waitFor(() => expect(fs.fsMovePaths).toHaveBeenCalledWith("/w", ["/w/src/b.txt"], "/w"))
})

test("拖到資料夾內的檔案列時，目標是那個檔案所在的資料夾", async () => {
    fs.fsMovePaths.mockResolvedValue(["/w/src/a.txt"])
    await mountTree(undefined, true)
    hit = row("b.txt")
    pointerDrag(row("a.txt"), MOVE, { from: START, release: false })
    expect(row("b.txt").getAttribute("data-pointer-drop-target")).toBeNull()
    expect(row("src").getAttribute("data-pointer-drop-target")).toBe("inside")
    fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId: 1, clientX: 12, clientY: 0 })
    await waitFor(() => expect(fs.fsMovePaths).toHaveBeenCalledWith("/w", ["/w/a.txt"], "/w/src"))
})

test("拖到根層的檔案列時，目標是 workspace 根目錄", async () => {
    fs.fsMovePaths.mockResolvedValue(["/w/b.txt"])
    await mountTree(undefined, true)
    hit = row("a.txt")
    pointerDrag(row("b.txt"), MOVE, { from: START, release: false })
    expect(treeRoot()?.getAttribute("data-pointer-drop-target")).toBe("inside")
    fireEvent.pointerUp(window, { button: 0, buttons: 0, pointerId: 1, clientX: 12, clientY: 0 })
    await waitFor(() => expect(fs.fsMovePaths).toHaveBeenCalledWith("/w", ["/w/src/b.txt"], "/w"))
})

test("檔案拖到編輯器群組會開在該群組；資料夾不行", async () => {
    await mountTree(<div data-editor-group-index="1"><span id="body" /></div>)
    hit = document.getElementById("body")
    pointerDrag(row("src"), MOVE, { from: START })
    expect(useWorkspaceStore.getState().groups[1].tabs).toHaveLength(0)
    pointerDrag(row("a.txt"), MOVE, { from: START })
    expect(useWorkspaceStore.getState().groups[1].tabs.map((tab) => tab.path)).toEqual(["/w/a.txt"])
})

test("拖到終端 leaf 會貼上 quote 過的路徑", async () => {
    const paste = vi.fn(async () => undefined)
    const focus = vi.fn()
    const unregister = registerTerminalDropTarget("leaf-1", {
        scope: runtimeKey({ hostId: LOCAL_HOST_ID, sessionName: "main" }),
        canWrite: () => true,
        paste,
        focus
    })
    try {
        await mountTree(<div data-attachment-key="leaf-1"><span id="xterm" /></div>)
        hit = document.getElementById("xterm")
        pointerDrag(row("a.txt"), MOVE, { from: START })
        await waitFor(() => expect(paste).toHaveBeenCalledWith("/w/a.txt "))
        expect(focus).toHaveBeenCalled()
        expect(fs.fsMovePaths).not.toHaveBeenCalled()
    } finally {
        unregister()
    }
})

test("拖曳不會開 transient 分頁；單擊仍會", async () => {
    await mountTree()
    hit = null
    pointerDrag(row("a.txt"), MOVE, { from: START })
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(0)
    fireEvent.click(row("a.txt"))
    expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(0)
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 5)) })
    fireEvent.click(row("a.txt"))
    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({ path: "/w/a.txt", transient: true })
})
