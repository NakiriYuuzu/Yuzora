import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { clearAll, documentGeneration, getDocument, updateBuffer } from "@/editor/documentRegistry"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useFileClipboardStore } from "@/state/fileClipboardStore"
import { useFileTreeStore } from "@/state/fileTreeStore"
import { useAppDialogStore } from "@/state/appDialogStore"
import { fsCopyPaths, fsMovePaths, fsPasteClipboardFiles } from "@/lib/ipc"
import * as ipc from "@/lib/ipc"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { pasteFiles } from "./fileClipboard"

const root = "/paste-owner-fixture", source = `${root}/a.txt`, targetDir = `${root}/dest`, target = `${targetDir}/a.txt`
const originalMessage = useAppDialogStore.getState().message
let calls: Array<{ command: string; args: Record<string, unknown> }>
let releases: Array<() => void>
let clipboardGate: Promise<void> | undefined
let mutationGate: Promise<void> | undefined
let failure: string | undefined
let messages: string[]
let osPaths: string[]

function deferred() {
    let resolve!: () => void
    const promise = new Promise<void>(done => { resolve = done })
    releases.push(resolve)
    return { promise, resolve }
}
function cut() {
    const clipboard = { workspacePath: root, paths: [source], mode: "cut" as const }
    useFileClipboardStore.getState().setClipboard(clipboard)
    return clipboard
}
function paste() { return pasteFiles(root, { path: targetDir, isDirectory: true }) }
function switchAwayAndBack() {
    clearAll(); useWorkspaceStore.getState().setWorkspace("/other", "other-owner")
    clearAll(); useWorkspaceStore.getState().setWorkspace(root, "new-owner")
}
const moves = () => calls.filter(call => call.command === "fs_move_paths")

beforeEach(() => {
    calls = []; releases = []; messages = []; osPaths = []
    clipboardGate = undefined; mutationGate = undefined; failure = undefined
    clearAll()
    useWorkspaceStore.setState({ workspacePath: root, workspaceCapabilityId: "owner", groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useAppDialogStore.setState({ message: async options => { messages.push(options.description) } })
    cut()
    mockIPC(async (command, raw) => {
        const args = (raw ?? {}) as Record<string, unknown>; calls.push({ command, args })
        if (command === "clipboard_read_file_list") { await clipboardGate; return osPaths }
        if (command === "open_file") return { kind: "full", content: "DISK", size: 4, lineEnding: "lf" }
        if (command === "fs_move_paths" || command === "fs_copy_paths" || command === "fs_paste_clipboard_files") {
            await mutationGate
            if (failure) throw new Error(failure)
            return ["dest/a.txt"]
        }
        if (command === "list_dir") return []
        return null
    })
})
afterEach(() => {
    for (const release of releases) release()
    clearAll(); clearMocks(); vi.restoreAllMocks()
    useFileClipboardStore.setState({ clipboard: null, selection: null })
    useAppDialogStore.setState({ message: originalMessage })
})

it("does not dispatch a paste after the clipboard read crosses a workspace ABA", async () => {
    const gate = deferred(); clipboardGate = gate.promise
    const operation = paste()
    switchAwayAndBack(); gate.resolve(); await operation
    expect(moves()).toHaveLength(0)
})

it("does not dispatch the old cut after a different clipboard replaces it during its read", async () => {
    const gate = deferred(); clipboardGate = gate.promise
    const operation = paste()
    const replacement = { workspacePath: root, paths: [`${root}/b.txt`], mode: "cut" as const }
    useFileClipboardStore.getState().setClipboard(replacement)
    gate.resolve(); await operation
    expect(moves()).toHaveLength(0)
    expect(useFileClipboardStore.getState().clipboard).toBe(replacement)
})

it("does not retarget documents reopened by a later workspace owner", async () => {
    const gate = deferred(); mutationGate = gate.promise
    const operation = paste()
    await vi.waitFor(() => expect(moves()).toHaveLength(1))
    switchAwayAndBack()
    useWorkspaceStore.getState().openTab(source)
    const entry = await getDocument(source)
    updateBuffer(source, "NEW_OWNER_UNSAVED", documentGeneration(source))
    useWorkspaceStore.getState().markDirty(source, true)
    gate.resolve(); await operation
    expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual([source])
    expect(await getDocument(source)).toBe(entry)
    expect(entry.result).toMatchObject({ content: "NEW_OWNER_UNSAVED" })
})

it("keeps a newer clipboard when an earlier move finishes", async () => {
    const gate = deferred(); mutationGate = gate.promise
    const operation = paste()
    await vi.waitFor(() => expect(moves()).toHaveLength(1))
    const replacement = cut()
    gate.resolve(); await operation
    expect(useFileClipboardStore.getState().clipboard).toBe(replacement)
})

it("admits a cut only once while the first paste is in flight", async () => {
    const gate = deferred(); mutationGate = gate.promise
    const first = paste()
    await vi.waitFor(() => expect(moves()).toHaveLength(1))
    const second = paste()
    await new Promise(resolve => setTimeout(resolve, 20))
    const dispatched = moves().length
    gate.resolve(); await Promise.all([first, second])
    expect(dispatched).toBe(1)
})

it("does not dispatch a newly cut copy of the same physical source while its move is pending", async () => {
    const gate = deferred(); mutationGate = gate.promise
    const first = paste()
    await vi.waitFor(() => expect(moves()).toHaveLength(1))
    const replacement = cut()
    expect(await paste()).toEqual([])
    gate.resolve(); await first
    expect(moves()).toHaveLength(1)
    expect(useFileClipboardStore.getState().clipboard).toBe(replacement)
})

it.each(["copy", "move", "import"])("checks the %s owner immediately before native mutation dispatch", async method => {
    let current = true
    const operation = method === "import" ? fsPasteClipboardFiles(root, targetDir, () => current)
        : (method === "move" ? fsMovePaths : fsCopyPaths)(root, [source], targetDir, () => current)
    current = false
    expect(await operation).toEqual([])
    expect(calls.some(call => ["fs_move_paths", "fs_copy_paths", "fs_paste_clipboard_files"].includes(call.command))).toBe(false)
})

it.each(["copy", "move"])("checks the %s owner after loading the remote module", async method => {
    const remote = remoteFilePath("fixture-host", "/repo")
    let current = true
    const operation = (method === "move" ? fsMovePaths : fsCopyPaths)(remote, [remoteFilePath("fixture-host", "/repo/a.txt", "/repo")], remote, () => current)
    current = false
    expect(await operation).toEqual([])
    expect(calls).toHaveLength(0)
})

it.each(["copy", "import"])("does not publish successful %s UI or logs after predispatch cancellation", async method => {
    if (method === "copy") {
        useFileClipboardStore.getState().setClipboard({ workspacePath: root, paths: [source], mode: "copy" })
        const original = ipc.fsCopyPaths
        vi.spyOn(ipc, "fsCopyPaths").mockImplementationOnce((...args) => {
            const operation = original(...args)
            cut()
            return operation
        })
    } else {
        useFileClipboardStore.getState().setClipboard(null)
        osPaths = ["/external/fixture.txt"]
        const original = ipc.fsPasteClipboardFiles
        vi.spyOn(ipc, "fsPasteClipboardFiles").mockImplementationOnce((...args) => {
            const operation = original(...args)
            cut()
            return operation
        })
    }
    const revision = useWorkspaceStore.getState().treeRevision
    expect(await paste()).toEqual([])
    expect(calls.some(call => ["fs_copy_paths", "fs_paste_clipboard_files"].includes(call.command))).toBe(false)
    expect(useWorkspaceStore.getState().treeRevision).toBe(revision)
    expect(useFileTreeStore.getState().trees[root]?.expandedDirs.has(targetDir)).not.toBe(true)
    expect(calls.some(call => call.command === "log_event")).toBe(false)
})

it("attributes a completed move to its original workspace after the tree refresh awaits", async () => {
    const gate = deferred()
    const invalidate = vi.spyOn(useFileTreeStore.getState(), "invalidatePaths").mockImplementationOnce(async () => { await gate.promise })
    const operation = paste()
    await vi.waitFor(() => expect(invalidate).toHaveBeenCalledTimes(1))
    clearAll(); useWorkspaceStore.getState().setWorkspace("/other", "other-owner")
    gate.resolve(); await operation
    expect(calls.find(call => call.command === "log_event")?.args.event).toMatchObject({ event: "file_paste", workspace_path: root })
})

it("releases cut admission after failure so the same clipboard can be retried", async () => {
    const clipboard = useFileClipboardStore.getState().clipboard
    failure = "fixture move failed"
    expect(await paste()).toEqual([])
    expect(messages).toEqual([expect.stringContaining("fixture move failed")])
    expect(useFileClipboardStore.getState().clipboard).toBe(clipboard)
    failure = undefined
    expect(await paste()).toEqual([target])
    expect(moves()).toHaveLength(2)
    expect(useFileClipboardStore.getState().clipboard).toBeNull()
})

it("preserves ordinary successful move and dirty source retargeting", async () => {
    useWorkspaceStore.getState().openTab(source)
    const entry = await getDocument(source)
    updateBuffer(source, "UNSAVED_SOURCE", documentGeneration(source))
    useWorkspaceStore.getState().markDirty(source, true)
    expect(await paste()).toEqual([target])
    expect(useWorkspaceStore.getState().groups[0].tabs).toMatchObject([{ path: target, dirty: true }])
    expect(await getDocument(target)).toBe(entry)
    expect(entry.result).toMatchObject({ content: "UNSAVED_SOURCE" })
})

it("does not publish a late move error into a different workspace lifecycle", async () => {
    const gate = deferred(); mutationGate = gate.promise; failure = "OLD_OWNER_ERROR"
    const operation = paste()
    await vi.waitFor(() => expect(moves()).toHaveLength(1))
    switchAwayAndBack(); gate.resolve(); await operation
    expect(messages).toEqual([])
})
