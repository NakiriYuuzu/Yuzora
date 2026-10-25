import { afterEach, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"
import { createElement } from "react"
import { WorkspaceResourcesBridge } from "../workbench/WorkspaceResourcesBridge"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { clearAll, documentChangedOnDisk, documentGeneration, getDocument, updateBuffer } from "../editor/documentRegistry"
import { executeLegacyContextMenuAction } from "./contextMenuStore"
import { useWorkspaceStore } from "./workspaceStore"
import { useTextInputDialogStore } from "./textInputDialogStore"
import { useAppDialogStore } from "./appDialogStore"
import { useFileTreeStore } from "./fileTreeStore"

const original = { request: useTextInputDialogStore.getState().request, message: useAppDialogStore.getState().message }
afterEach(() => {
    cleanup()
    clearMocks(); clearAll()
    useTextInputDialogStore.setState({ request: original.request })
    useAppDialogStore.setState({ message: original.message })
})

it.each(["unchanged", "changed", "closed"])("compares a watcher event during rename against actual disk contents (%s)", async scenario => {
    clearAll()
    const root = "/rename-watch", from = `${root}/a.txt`, to = `${root}/b.txt`
    useWorkspaceStore.setState({ workspacePath: root, workspaceCapabilityId: "owned-fixture", groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useTextInputDialogStore.setState({ request: async () => "b.txt" })
    useAppDialogStore.setState({ message: async () => {} })
    render(createElement(WorkspaceResourcesBridge))
    let complete!: () => void, dispatched = false, renamed = false
    const gate = new Promise<void>(resolve => { complete = resolve })
    let completeRead!: () => void, readingTarget = false
    const readGate = new Promise<void>(resolve => { completeRead = resolve })
    mockIPC(async (command, raw) => {
        if (command === "open_file") {
            const path = (raw as { path: string }).path
            if (path !== (renamed ? to : from)) throw new Error("fixture file not found")
            if (renamed && scenario === "closed") { readingTarget = true; await readGate }
            const content = renamed && scenario === "changed" ? "EXTERNAL_DISK" : "UNCHANGED_DISK"
            return { kind: "full", content, size: content.length, lineEnding: "lf" }
        }
        if (command === "fs_rename") { dispatched = true; await gate; renamed = true }
        return null
    })
    useWorkspaceStore.getState().openTab(from)
    await getDocument(from)
    updateBuffer(from, "UNSAVED_EDITOR", documentGeneration(from))
    useWorkspaceStore.getState().markDirty(from, true)
    const operation = executeLegacyContextMenuAction({ kind: "file", workspacePath: root, path: from, isDirectory: false, sourceGroupIndex: 0 }, "cmRename")
    await vi.waitFor(() => expect(dispatched).toBe(true))
    const flagRenamed = vi.fn()
    const comparison = documentChangedOnDisk(from, flagRenamed).then(value => value, () => "error")
    // Allow an unblocked original-version read to finish before the rename.
    for (let turn = 0; turn < 10; turn++) await Promise.resolve()
    complete(); await operation
    if (scenario === "closed") {
        await vi.waitFor(() => expect(readingTarget).toBe(true))
        useWorkspaceStore.getState().closeTabsByPath([to])
        completeRead()
        expect(await comparison).toBe("error")
        expect(flagRenamed).not.toHaveBeenCalled()
        return
    }
    expect(await comparison).toBe(scenario === "changed")
    if (scenario === "changed") expect(flagRenamed).toHaveBeenCalledWith(to)
    else expect(flagRenamed).not.toHaveBeenCalled()
    expect((await getDocument(to)).result).toMatchObject({ content: "UNSAVED_EDITOR" })
})
