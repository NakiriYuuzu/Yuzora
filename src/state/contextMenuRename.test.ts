import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { createElement } from "react"
import { WorkspaceResourcesBridge } from "../workbench/WorkspaceResourcesBridge"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { executeLegacyContextMenuAction } from "./contextMenuStore"
import { useWorkspaceStore } from "./workspaceStore"
import { useFileTreeStore } from "./fileTreeStore"
import { useAppDialogStore } from "./appDialogStore"
import { useTextInputDialogStore } from "./textInputDialogStore"
import { cachedDocumentPaths, clearAll, documentGeneration, dropDocument, getDocument, reloadDocument, reserveDocumentRename, saveDocumentContent, updateBuffer } from "../editor/documentRegistry"
import { openFile, saveFile } from "../lib/ipc"
import { markdownPreviewPath } from "../lib/markdownPreviewTab"
import { remoteFilePath } from "../lib/runtimeIdentity"

const root = "/rename-fixture"
const source = `${root}/a.txt`
const target = `${root}/b.txt`
const disk = new Map<string, string>()
let calls: Array<{ command: string; args: Record<string, unknown> }>
let renameGate: Promise<void> | undefined
let readGate: Promise<void> | undefined
let saveGate: Promise<void> | undefined
let releases: Array<() => void>
const dialogMethods = { message: useAppDialogStore.getState().message, request: useTextInputDialogStore.getState().request }

function deferred() {
    let resolve!: () => void
    const promise = new Promise<void>(done => { resolve = done })
    releases.push(resolve)
    return { promise, resolve }
}

function rename(path = source, name = "b.txt") {
    useTextInputDialogStore.setState({ request: async () => name })
    return executeLegacyContextMenuAction({ kind: "file", workspacePath: root, path, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
}

async function dirty(path: string, content: string) {
    useWorkspaceStore.getState().openTab(path)
    const entry = await getDocument(path)
    updateBuffer(path, content, documentGeneration(path))
    useWorkspaceStore.getState().markDirty(path, true)
    return entry
}

function switchAwayAndBack() {
    clearAll()
    useWorkspaceStore.getState().setWorkspace("/other-workspace", "other")
    clearAll()
    useWorkspaceStore.getState().setWorkspace(root, "new-owner")
}

beforeEach(() => {
    calls = []; releases = []; renameGate = undefined; readGate = undefined; saveGate = undefined
    disk.clear(); disk.set(source, "SOURCE_DISK")
    clearAll()
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    useWorkspaceStore.getState().setWorkspace(root, "rename-owner")
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useAppDialogStore.setState({ message: async () => {} })
    render(createElement(WorkspaceResourcesBridge))
    mockIPC(async (command, raw) => {
        const args = (raw ?? {}) as Record<string, unknown>
        calls.push({ command, args })
        if (command === "open_file") {
            await readGate
            const content = disk.get(String(args.path))
            if (content === undefined) throw new Error("fixture file not found")
            return { kind: "full", content, size: content.length, lineEnding: "lf" }
        }
        if (command === "save_file") {
            await saveGate
            disk.set(String(args.path), String(args.content))
            return String(args.content).length
        }
        if (command === "fs_rename") {
            await renameGate
            const from = `${root}/${args.from}`, to = `${root}/${args.to}`
            for (const [path, content] of [...disk]) {
                if (path === from || path.startsWith(from + "/")) {
                    disk.delete(path); disk.set(to + path.slice(from.length), content)
                }
            }
        }
        return null
    })
})

afterEach(() => {
    for (const release of releases) release()
    cleanup()
    vi.restoreAllMocks(); clearMocks(); clearAll()
    useAppDialogStore.setState({ message: dialogMethods.message })
    useTextInputDialogStore.setState({ request: dialogMethods.request })
})

describe("rename document ownership", () => {
    it("preserves a destination dirty buffer after its disk file was externally removed", async () => {
        disk.set(target, "DESTINATION_DISK")
        await dirty(source, "SOURCE_UNSAVED")
        const destination = await dirty(target, "DESTINATION_UNSAVED")
        disk.delete(target)
        await rename()
        expect(destination.result).toMatchObject({ content: "DESTINATION_UNSAVED" })
        expect((await getDocument(target)).result).toMatchObject({ content: "DESTINATION_UNSAVED" })
        expect(calls.filter(call => call.command === "fs_rename")).toHaveLength(0)
        expect(disk.get(source)).toBe("SOURCE_DISK")
    })

    it("protects destination descendants even when only their cached document remains", async () => {
        const from = `${root}/old`, to = `${root}/new`
        disk.set(`${from}/note.md`, "old disk"); disk.set(`${to}/note.md`, "new disk")
        await getDocument(`${from}/note.md`)
        await getDocument(`${to}/note.md`)
        updateBuffer(`${to}/note.md`, "UNSAVED_DESTINATION", documentGeneration(`${to}/note.md`))
        disk.delete(`${to}/note.md`)
        await rename(from, "new")
        expect((await getDocument(`${to}/note.md`)).result).toMatchObject({ content: "UNSAVED_DESTINATION" })
        expect(calls.filter(call => call.command === "fs_rename")).toHaveLength(0)
    })

    it("waits a late destination open and merges its placeholder with the source tab", async () => {
        disk.delete(target)
        await dirty(source, "SOURCE_UNSAVED")
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        useWorkspaceStore.getState().openTab(target)
        const pending = getDocument(target).then(entry => entry.result, error => ({ error: String(error) }))
        await Promise.resolve(); await Promise.resolve()
        const readsBeforeRename = calls.filter(call => call.command === "open_file" && call.args.path === target).length
        gate.resolve(); await operation
        expect(await pending).toMatchObject({ content: "SOURCE_UNSAVED" })
        expect(readsBeforeRename).toBe(0)
        expect(useWorkspaceStore.getState().groups[0].tabs).toMatchObject([{ path: target, dirty: true }])
        expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
    })

    it("does not dispatch a rename from a dialog belonging to an earlier workspace lifecycle", async () => {
        let respond!: (name: string) => void
        useTextInputDialogStore.setState({ request: () => new Promise(resolve => { respond = resolve }) })
        const operation = executeLegacyContextMenuAction({ kind: "file", workspacePath: root, path: source, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
        switchAwayAndBack(); respond("b.txt"); await operation
        expect(calls.filter(call => call.command === "fs_rename")).toHaveLength(0)
    })

    it("does not retarget newly reopened documents when an old rename completes after workspace ABA", async () => {
        await dirty(source, "OLD_OWNER")
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        switchAwayAndBack()
        useWorkspaceStore.getState().openTab(source)
        gate.resolve(); await operation
        expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toEqual([source])
    })

    it("rejects rename while a direct IPC save is still writing the source", async () => {
        const gate = deferred(); saveGate = gate.promise
        const saving = saveFile(source, "SAVED_SOURCE")
        await rename()
        const renamesBeforeSave = calls.filter(call => call.command === "fs_rename").length
        gate.resolve(); await saving
        expect(renamesBeforeSave).toBe(0)
        expect(disk.get(source)).toBe("SAVED_SOURCE")
    })

    it("rejects rename while a direct IPC read can still accept an old source snapshot", async () => {
        const gate = deferred(); readGate = gate.promise
        const reading = openFile(source).catch(() => null)
        await rename()
        const renamesBeforeRead = calls.filter(call => call.command === "fs_rename").length
        gate.resolve(); await reading
        expect(renamesBeforeRead).toBe(0)
    })

    it("does not let a new save recreate the old path during rename", async () => {
        await dirty(source, "SOURCE_UNSAVED")
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        const result = await saveFile(source, "LATE_SAVE").then(() => "saved", () => "blocked")
        gate.resolve(); await operation
        expect(result).toBe("blocked")
        expect(calls.filter(call => call.command === "save_file")).toHaveLength(0)
    })

    it("retargets a cached source used only by a Markdown preview", async () => {
        const from = `${root}/old.md`, to = `${root}/new.md`
        disk.set(from, "SOURCE_MARKDOWN")
        const entry = await getDocument(from)
        updateBuffer(from, "MARKDOWN_BUFFER", documentGeneration(from))
        useWorkspaceStore.setState({ groups: [{ tabs: [{ path: markdownPreviewPath(from), sourcePath: from, name: "Preview", kind: "markdown-preview", dirty: false, externallyModified: false }], activePath: markdownPreviewPath(from) }] })
        await rename(from, "new.md")
        expect(await getDocument(to)).toBe(entry)
        expect((await getDocument(to)).result).toMatchObject({ content: "MARKDOWN_BUFFER" })
    })

    it("releases failed renames before showing an error dialog and lets a queued read finish", async () => {
        const gate = deferred()
        renameGate = gate.promise.then(() => { throw new Error("fixture rename failure") })
        let shown = false
        useAppDialogStore.setState({ message: async () => { shown = true; await deferred().promise } })
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        const reading = getDocument(source)
        gate.resolve()
        expect((await reading).result).toMatchObject({ content: "SOURCE_DISK" })
        expect(shown).toBe(true)
        for (const release of releases) release()
        await operation
        await expect(saveFile(source, "AFTER_FAILURE")).resolves.toBe(13)
    })

    it("retires a queued read from a previous workspace but keeps the physical rename reserved", async () => {
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        const oldRead = getDocument(target).then(() => "accepted", () => "stale")
        switchAwayAndBack()
        const currentRead = getDocument(target)
        const attemptedSave = await saveFile(source, "LATE_AFTER_SWITCH").then(() => "saved", () => "blocked")
        gate.resolve(); await operation
        expect(await oldRead).toBe("stale")
        expect((await currentRead).result).toMatchObject({ content: "SOURCE_DISK" })
        expect(attemptedSave).toBe("blocked")
    })

    it("protects a cached Windows destination through verbatim/case aliases", async () => {
        const path = String.raw`\\?\C:\Repo\B\note.txt`
        useWorkspaceStore.getState().setWorkspace(String.raw`C:\Repo`, "windows-fixture")
        disk.set(path, "WINDOWS_UNSAVED")
        await getDocument(path)
        expect(() => reserveDocumentRename(String.raw`C:\Repo\A`, String.raw`c:\repo\b`)).toThrow()
        expect((await getDocument(path)).result).toMatchObject({ content: "WINDOWS_UNSAVED" })
    })

    it("allows case-only rename but rejects choosing between two cached Windows aliases", async () => {
        useWorkspaceStore.getState().setWorkspace(String.raw`C:\Repo`, "windows-fixture")
        const first = String.raw`C:\Repo\Note.txt`, second = String.raw`c:\repo\note.txt`
        disk.set(first, "FIRST"); disk.set(second, "SECOND")
        await getDocument(first)
        reserveDocumentRename(first, String.raw`C:\Repo\NOTE.txt`)()
        await getDocument(second)
        expect(() => reserveDocumentRename(first, String.raw`C:\Repo\NOTE.txt`)).toThrow()
    })

    it("does not discard source metadata when late destination placeholders exist in two splits", async () => {
        await dirty(source, "SOURCE_UNSAVED")
        await reloadDocument(source)
        updateBuffer(source, "SOURCE_UNSAVED", documentGeneration(source))
        useWorkspaceStore.getState().hydrateLineEnding(source, "crlf", documentGeneration(source))
        useWorkspaceStore.getState().toggleTabPinned(0, source)
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        useWorkspaceStore.getState().openTab(target)
        useWorkspaceStore.getState().openInRightSplit(target, 0)
        const queued = getDocument(target)
        gate.resolve(); await operation; await queued
        const groups = useWorkspaceStore.getState().groups
        expect(groups[0].tabs).toHaveLength(1)
        expect(groups[0].tabs[0]).toMatchObject({ path: target, dirty: true, pinned: true, lineEnding: "crlf" })
        expect(groups[1].tabs.map(tab => tab.path)).toEqual([target])
        expect(groups[1].tabs[0]).toMatchObject({ dirty: true, lineEnding: "crlf" })
        expect((await getDocument(target)).result).toMatchObject({ content: "SOURCE_UNSAVED" })
    })

    it("keeps queued local saves ahead of any overlapping rename admission", async () => {
        await getDocument(source)
        const gate = deferred(); saveGate = gate.promise
        const first = saveDocumentContent(source, "FIRST_SAVE")
        const second = saveDocumentContent(source, "SECOND_SAVE")
        expect(() => reserveDocumentRename(source, target)).toThrow()
        gate.resolve(); await first; await second
        expect(disk.get(source)).toBe("SECOND_SAVE")
        reserveDocumentRename(source, target)()
    })

    it("cancels a late destination read when its tab closes before rename settles", async () => {
        const gate = deferred(); renameGate = gate.promise
        const operation = rename()
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        useWorkspaceStore.getState().openTab(target)
        const reading = getDocument(target).then(() => "accepted", () => "cancelled")
        useWorkspaceStore.getState().closeTabsByPath([target]); dropDocument(target)
        gate.resolve(); await operation
        expect(await reading).toBe("cancelled")
        expect(calls.filter(call => call.command === "open_file")).toHaveLength(0)
        expect(cachedDocumentPaths(target)).toEqual([])
    })

    it("retires a Windows alias waiter instead of reading disk over the moved unsaved buffer", async () => {
        const workspace = String.raw`C:\Repo`, from = String.raw`C:\Repo\a.txt`, to = String.raw`C:\Repo\b.txt`, alias = String.raw`c:\repo\b.txt`
        useWorkspaceStore.getState().setWorkspace(workspace, "windows-owner")
        const gate = deferred()
        mockIPC(async (command, raw) => {
            calls.push({ command, args: (raw ?? {}) as Record<string, unknown> })
            if (command === "fs_rename") await gate.promise
            if (command === "open_file") return { kind: "full", content: "OLD_DISK", size: 8, lineEnding: "lf" }
            return null
        })
        const entry = await dirty(from, "WINDOWS_UNSAVED")
        useTextInputDialogStore.setState({ request: async () => "b.txt" })
        const operation = executeLegacyContextMenuAction({ kind: "file", workspacePath: workspace, path: from, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
        await vi.waitFor(() => expect(calls.some(call => call.command === "fs_rename")).toBe(true))
        useWorkspaceStore.getState().openTab(alias)
        const reading = getDocument(alias).then(() => "accepted", () => "cancelled")
        gate.resolve(); await operation
        expect(await reading).toBe("cancelled")
        expect(await getDocument(to)).toBe(entry)
        expect(calls.filter(call => call.command === "open_file")).toHaveLength(1)
        expect(cachedDocumentPaths(to)).toEqual([to])
    })

    it("coalesces moved Windows source aliases when only one has a cached buffer", async () => {
        const workspace = String.raw`C:\Repo`, from = String.raw`C:\Repo\a.txt`, alias = String.raw`c:\repo\a.txt`, to = String.raw`C:\Repo\b.txt`
        useWorkspaceStore.getState().setWorkspace(workspace, "windows-owner")
        disk.set(from, "OLD_DISK")
        useWorkspaceStore.getState().openTab(alias)
        await dirty(from, "WINDOWS_UNSAVED")
        useWorkspaceStore.getState().toggleTabPinned(0, from)
        useTextInputDialogStore.setState({ request: async () => "b.txt" })
        await executeLegacyContextMenuAction({ kind: "file", workspacePath: workspace, path: from, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
        expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({ path: to, dirty: true, pinned: true })
        expect((await getDocument(to)).result).toMatchObject({ content: "WINDOWS_UNSAVED" })
    })

    it("preserves both unrelated Windows alias buffers while renaming another document", async () => {
        const workspace = String.raw`C:\Repo`, from = String.raw`C:\Repo\a.txt`
        const first = String.raw`C:\Repo\unrelated.txt`, second = String.raw`c:\repo\unrelated.txt`
        useWorkspaceStore.getState().setWorkspace(workspace, "windows-owner")
        disk.set(from, "SOURCE"); disk.set(first, "DISK"); disk.set(second, "DISK")
        await dirty(from, "SOURCE_UNSAVED")
        await dirty(first, "FIRST_UNSAVED"); await dirty(second, "SECOND_UNSAVED")
        useTextInputDialogStore.setState({ request: async () => "b.txt" })
        await executeLegacyContextMenuAction({ kind: "file", workspacePath: workspace, path: from, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
        expect((await getDocument(first)).result).toMatchObject({ content: "FIRST_UNSAVED" })
        expect((await getDocument(second)).result).toMatchObject({ content: "SECOND_UNSAVED" })
        expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toContain(first)
        expect(useWorkspaceStore.getState().groups[0].tabs.map(tab => tab.path)).toContain(second)
    })

    it("merges a late remote Windows alias while preserving renamed source metadata", () => {
        const from = remoteFilePath("host", "C:/Repo/a.txt", "C:/Repo")
        const to = remoteFilePath("host", "C:/Repo/b.txt", "C:/Repo")
        const alias = remoteFilePath("host", "c:/repo/b.txt", "C:/Repo")
        useWorkspaceStore.getState().openTab(from)
        useWorkspaceStore.getState().markDirty(from, true)
        useWorkspaceStore.getState().openTab(alias)
        useWorkspaceStore.getState().updateTabPath(from, to, true)
        expect(useWorkspaceStore.getState().groups[0].tabs).toHaveLength(1)
        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({ path: to, dirty: true })
        expect(useWorkspaceStore.getState().groups[0].activePath).toBe(to)
    })
})
