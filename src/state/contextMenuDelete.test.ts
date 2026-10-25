import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { createElement } from "react"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { clearAll, documentGeneration, getDocument, updateBuffer } from "../editor/documentRegistry"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { markdownPreviewPath } from "../lib/markdownPreviewTab"
import { remoteFilePath } from "../lib/runtimeIdentity"
import * as ipc from "../lib/ipc"
import { WorkspaceResourcesBridge } from "../workbench/WorkspaceResourcesBridge"
import { executeLegacyContextMenuAction } from "./contextMenuStore"
import { useAppDialogStore } from "./appDialogStore"
import { useFileTreeStore } from "./fileTreeStore"
import { useWorkspaceStore } from "./workspaceStore"

const root = "/delete-fixture", folder = `${root}/folder`
const removed = `${folder}/removed.txt`, survivor = `${folder}/survivor.txt`, dirtyPath = `${folder}/dirty.txt`
const original = { confirm: useAppDialogStore.getState().confirm, message: useAppDialogStore.getState().message }
let calls: Array<{ command: string; args: Record<string, unknown> }>
let messages: string[]
let failure: string | undefined
let deleteGate: Promise<void> | undefined
let onProbe: ((path: string) => Promise<boolean>) | undefined
let releaseAll: Array<() => void>
let views: Array<{ path: string; view: EditorView }>
const absent = new Set<string>()

function deferred() {
    let resolve!: () => void
    const promise = new Promise<void>(done => { resolve = done })
    releaseAll.push(resolve)
    return { promise, resolve }
}
function remove(path = folder) {
    return executeLegacyContextMenuAction({ kind: "file", workspacePath: root, path, isDirectory: path === folder, sourceGroupIndex: 0 }, "cmDelete")
}
async function open(path: string, dirty = false) {
    useWorkspaceStore.getState().openTab(path)
    await getDocument(path)
    if (dirty) { updateBuffer(path, "UNSAVED", documentGeneration(path)); useWorkspaceStore.getState().markDirty(path, true) }
}
const paths = () => useWorkspaceStore.getState().groups.flatMap(group => group.tabs).map(tab => tab.path)
function switchAwayAndBack() {
    clearAll(); useWorkspaceStore.getState().setWorkspace("/elsewhere", "other")
    clearAll(); useWorkspaceStore.getState().setWorkspace(root, "new-owner")
}

beforeEach(() => {
    calls = []; messages = []; releaseAll = []; views = []; absent.clear()
    failure = "delete-cancelled-partial"; deleteGate = undefined; onProbe = undefined
    clearAll()
    useWorkspaceStore.setState({ workspacePath: root, workspaceCapabilityId: "delete-owner", groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0, treeRevision: 0 })
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useAppDialogStore.setState({ confirm: async () => true, message: async message => { messages.push(message.description) } })
    render(createElement(WorkspaceResourcesBridge))
    mockIPC(async (command, raw) => {
        const args = (raw ?? {}) as Record<string, unknown>; calls.push({ command, args })
        if (command === "open_file") {
            if (absent.has(String(args.path))) throw new Error("fixture file not found")
            return { kind: "full", content: "DISK", size: 4, lineEnding: "lf" }
        }
        if (command === "is_openable_file") return onProbe ? onProbe(String(args.path)) : !absent.has(String(args.path))
        if (command === "fs_delete") {
            await deleteGate
            absent.add(removed); absent.add(dirtyPath)
            if (failure) throw new Error(failure)
        }
        return null
    })
})
afterEach(() => {
    for (const release of releaseAll) release()
    cleanup()
    for (const { path, view } of views) { unregisterView(path, view); view.destroy() }
    clearAll(); clearMocks(); vi.restoreAllMocks()
    useAppDialogStore.setState(original)
})

it.each(["delete-cancelled-partial", "delete-limit-reached-partial", "permission error after one removal"])("reconciles only removed clean documents after %s", async reason => {
    failure = reason
    await open(removed); await open(survivor); await open(dirtyPath, true)
    await remove()
    expect(paths()).not.toContain(removed)
    expect(paths()).toContain(survivor)
    expect(paths()).toContain(dirtyPath)
    expect((await getDocument(dirtyPath)).result).toMatchObject({ content: "UNSAVED" })
    expect(useWorkspaceStore.getState().treeRevision).toBeGreaterThan(0)
    expect(messages.length).toBeGreaterThan(0)
    if (reason.startsWith("permission")) expect(messages[0]).toContain(reason)
})

it("preserves a source whose metadata probe fails while reconciling other removed files", async () => {
    await open(removed); await open(survivor)
    onProbe = async path => { if (path === survivor) throw new Error("stat permission denied"); return false }
    await remove()
    expect(paths()).toEqual([survivor])
    expect(messages).toHaveLength(1)
})

it("does not send a deletion confirmed for an older workspace lifecycle", async () => {
    let confirm!: (ok: boolean) => void
    useAppDialogStore.setState({ confirm: () => new Promise(resolve => { confirm = resolve }) })
    const operation = remove()
    switchAwayAndBack(); confirm(true); await operation
    expect(calls.some(call => call.command === "fs_delete")).toBe(false)
})

it("does not close a new workspace document when an old successful deletion completes", async () => {
    failure = undefined; await open(removed)
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    switchAwayAndBack(); await open(removed, true)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "UNSAVED" })
})

it("preserves a new edit made while a successful deletion was pending", async () => {
    failure = undefined; await open(removed)
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    updateBuffer(removed, "NEW_EDIT", documentGeneration(removed)); useWorkspaceStore.getState().markDirty(removed, true)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "NEW_EDIT" })
})

it("preserves further typing in an already-dirty live document after delete dispatch", async () => {
    failure = undefined; await open(removed, true)
    const view = new EditorView({ state: EditorState.create({ doc: "ORIGINAL_DIRTY" }) })
    views.push({ path: removed, view }); registerView(removed, view)
    const tab = useWorkspaceStore.getState().groups[0].tabs[0]
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "NEW_TYPING" } })
    useWorkspaceStore.getState().markDirty(removed, true)
    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toBe(tab)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect(view.state.doc.toString()).toBe("NEW_TYPING")
})

it("retains a newly opened linked Markdown preview when deletion completes", async () => {
    failure = undefined
    const source = `${folder}/note.md`; await open(source)
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    useWorkspaceStore.getState().openMarkdownPreviewInAdjacentGroup(source, 0)
    gate.resolve(); await operation
    expect(paths()).toContain(markdownPreviewPath(source))
    expect(paths()).toContain(source)
})

it("closes a confirmed unchanged dirty document on successful deletion", async () => {
    failure = undefined; await open(removed, true)
    await remove()
    expect(paths()).not.toContain(removed)
})

it("does not close a newly reopened tab after a delayed failed-delete metadata probe", async () => {
    await open(removed)
    const gate = deferred(); let probing = false
    onProbe = async () => { probing = true; await gate.promise; return false }
    const operation = remove()
    await vi.waitFor(() => expect(probing).toBe(true))
    useWorkspaceStore.getState().closeTabsByPath([removed])
    absent.delete(removed); await open(removed, true)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "UNSAVED" })
})

it("does not close old or new documents when workspace ABA occurs during a metadata probe", async () => {
    await open(removed)
    const gate = deferred(); let probing = false
    onProbe = async () => { probing = true; await gate.promise; return false }
    const operation = remove()
    await vi.waitFor(() => expect(probing).toBe(true))
    switchAwayAndBack(); absent.delete(removed); await open(removed, true)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "UNSAVED" })
})

it("preserves an edit-then-save tab even when dirty returns to false during deletion", async () => {
    failure = undefined; await open(removed)
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    useWorkspaceStore.getState().markDirty(removed, true)
    updateBuffer(removed, "NEW_SAVED_BUFFER", documentGeneration(removed))
    useWorkspaceStore.getState().markDirty(removed, false)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "NEW_SAVED_BUFFER" })
})

it("closes a stale standalone Markdown preview but retains its possibly dirty cached source", async () => {
    const source = `${folder}/note.md`
    await getDocument(source); updateBuffer(source, "UNOWNED_UNSAVED", documentGeneration(source))
    const preview = markdownPreviewPath(source)
    useWorkspaceStore.setState({ groups: [{ tabs: [{ path: preview, sourcePath: source, kind: "markdown-preview", name: "Preview", dirty: false, externallyModified: false }], activePath: preview }] })
    onProbe = async () => false
    await remove()
    expect(paths()).not.toContain(preview)
    expect((await getDocument(source)).result).toMatchObject({ content: "UNOWNED_UNSAVED" })
})

it("preserves a new linked preview added while a failed-delete probe is pending", async () => {
    const source = `${folder}/note.md`; await open(source)
    const gate = deferred(); let probing = false
    onProbe = async () => { probing = true; await gate.promise; return false }
    const operation = remove()
    await vi.waitFor(() => expect(probing).toBe(true))
    useWorkspaceStore.getState().openMarkdownPreviewInAdjacentGroup(source, 0)
    gate.resolve(); await operation
    expect(paths()).toContain(source)
    expect(paths()).toContain(markdownPreviewPath(source))
})

it("does not interpret the remote catch-all openability result as deletion evidence", async () => {
    const workspace = remoteFilePath("fixture-host", "/repo", "/repo")
    const source = remoteFilePath("fixture-host", "/repo/file.txt", "/repo")
    useWorkspaceStore.getState().setWorkspace(workspace, null)
    useWorkspaceStore.getState().openTab(source)
    vi.spyOn(ipc, "fsDelete").mockRejectedValue(new Error("remote partial failure"))
    const probe = vi.spyOn(ipc, "isOpenableFile").mockResolvedValue(false)
    await executeLegacyContextMenuAction({ kind: "file", workspacePath: workspace, path: source, isDirectory: false, sourceGroupIndex: 0 }, "cmDelete")
    expect(paths()).toContain(source)
    expect(probe).not.toHaveBeenCalled()
    expect(messages[0]).toContain("remote partial failure")
})

it("preserves a newer cached revision even when the tab was already dirty and stays the same object", async () => {
    failure = undefined; await open(removed, true)
    const tab = useWorkspaceStore.getState().groups[0].tabs[0]
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    updateBuffer(removed, "NEW_CACHED_REVISION", documentGeneration(removed))
    expect(useWorkspaceStore.getState().groups[0].tabs[0]).toBe(tab)
    gate.resolve(); await operation
    expect(paths()).toContain(removed)
    expect((await getDocument(removed)).result).toMatchObject({ content: "NEW_CACHED_REVISION" })
})

it("preserves ambiguous dirty split buffers when the registry can expose only one live view", async () => {
    failure = undefined; await open(removed, true)
    const tab = useWorkspaceStore.getState().groups[0].tabs[0]
    useWorkspaceStore.setState({ groups: [{ tabs: [tab], activePath: removed }, { tabs: [{ ...tab }], activePath: removed }] })
    const first = new EditorView({ state: EditorState.create({ doc: "FIRST_DIRTY" }) })
    const second = new EditorView({ state: EditorState.create({ doc: "SECOND_DIRTY" }) })
    views.push({ path: removed, view: first }, { path: removed, view: second })
    registerView(removed, first, { groupIndex: 0 }); registerView(removed, second, { groupIndex: 1 })
    const gate = deferred(); deleteGate = gate.promise
    const operation = remove()
    await vi.waitFor(() => expect(calls.some(call => call.command === "fs_delete")).toBe(true))
    first.dispatch({ changes: { from: 0, to: first.state.doc.length, insert: "NEW_FIRST_EDIT" } })
    gate.resolve(); await operation
    expect(paths()).toEqual([removed, removed])
    expect(first.state.doc.toString()).toBe("NEW_FIRST_EDIT")
})
